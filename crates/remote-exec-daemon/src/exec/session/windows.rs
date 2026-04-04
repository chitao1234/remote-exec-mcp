use std::borrow::Cow;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::bail;
use winptyrs::EnvBlock;

use crate::config::{ProcessEnvironment, WindowsPtyBackendOverride};

use super::{LiveSession, SessionChild, new_live_session, portable_pty_probe, spawn_pty};

#[cfg(any(test, windows))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PtyBackend {
    PortablePty,
    Winpty,
}

impl PtyBackend {
    fn debug_name(self) -> &'static str {
        match self {
            Self::PortablePty => "conpty_via_portable_pty",
            Self::Winpty => "winpty",
        }
    }
}

#[derive(Debug, Clone)]
struct PtyDiagnostics {
    selected_backend: Option<PtyBackend>,
    portable_pty_probe: Result<(), String>,
    winpty_probe: Result<(), String>,
}

#[derive(Debug, Default)]
pub(super) struct TerminalQueryState {
    pending: Vec<u8>,
}

#[derive(Debug, Default)]
pub(super) struct TerminalQueryResult {
    pub(super) output: String,
    pub(super) response: String,
}

impl TerminalQueryState {
    pub(super) fn filter_chunk(&mut self, chunk: &str) -> TerminalQueryResult {
        let mut input = Vec::with_capacity(self.pending.len() + chunk.len());
        input.extend_from_slice(&self.pending);
        input.extend_from_slice(chunk.as_bytes());
        self.pending.clear();

        let mut visible = Vec::with_capacity(input.len());
        let mut response = String::new();
        let mut index = 0;

        while index < input.len() {
            let remaining = &input[index..];
            if remaining.starts_with(b"\x1b[5n") {
                response.push_str("\x1b[0n");
                index += 4;
                continue;
            }
            if remaining.starts_with(b"\x1b[6n") {
                response.push_str("\x1b[1;1R");
                index += 4;
                continue;
            }

            if let Some(prefix_len) = terminal_query_prefix_len(remaining) {
                self.pending.extend_from_slice(&remaining[..prefix_len]);
                break;
            }

            visible.push(input[index]);
            index += 1;
        }

        TerminalQueryResult {
            output: String::from_utf8_lossy(&visible).into_owned(),
            response,
        }
    }

    pub(super) fn drain_pending(&mut self) -> String {
        let pending = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending.clear();
        pending
    }
}

fn terminal_query_prefix_len(bytes: &[u8]) -> Option<usize> {
    match bytes {
        [0x1b] => Some(1),
        [0x1b, b'['] => Some(2),
        [0x1b, b'[', b'5'] | [0x1b, b'[', b'6'] => Some(3),
        _ => None,
    }
}

pub(super) fn normalize_input(chars: &str, tty: bool) -> Cow<'_, str> {
    if !tty || !chars.contains('\n') {
        return Cow::Borrowed(chars);
    }

    let mut normalized = String::with_capacity(chars.len());
    let mut last_was_cr = false;

    for ch in chars.chars() {
        match ch {
            '\r' => {
                normalized.push('\r');
                last_was_cr = true;
            }
            '\n' => {
                if !last_was_cr {
                    normalized.push('\r');
                }
                last_was_cr = false;
            }
            _ => {
                normalized.push(ch);
                last_was_cr = false;
            }
        }
    }

    Cow::Owned(normalized)
}

#[cfg(any(test, windows))]
pub(super) fn select_pty_backend_with(
    portable_probe: impl FnOnce() -> anyhow::Result<()>,
    winpty_probe: impl FnOnce() -> anyhow::Result<()>,
) -> Option<PtyBackend> {
    if portable_probe().is_ok() {
        return Some(PtyBackend::PortablePty);
    }
    if winpty_probe().is_ok() {
        return Some(PtyBackend::Winpty);
    }
    None
}

pub(super) fn select_pty_backend_with_override(
    windows_pty_backend_override: Option<WindowsPtyBackendOverride>,
    portable_probe: impl FnOnce() -> anyhow::Result<()>,
    winpty_probe: impl FnOnce() -> anyhow::Result<()>,
) -> Option<PtyBackend> {
    match windows_pty_backend_override {
        Some(WindowsPtyBackendOverride::PortablePty) => {
            portable_probe().ok().map(|_| PtyBackend::PortablePty)
        }
        Some(WindowsPtyBackendOverride::Winpty) => winpty_probe().ok().map(|_| PtyBackend::Winpty),
        None => select_pty_backend_with(portable_probe, winpty_probe),
    }
}

fn collect_pty_diagnostics() -> PtyDiagnostics {
    let portable_pty_probe = portable_pty_probe().map_err(|err| err.to_string());
    let winpty_probe = super::super::winpty::supports_winpty().map_err(|err| err.to_string());
    let selected_backend = if portable_pty_probe.is_ok() {
        Some(PtyBackend::PortablePty)
    } else if winpty_probe.is_ok() {
        Some(PtyBackend::Winpty)
    } else {
        None
    };

    PtyDiagnostics {
        selected_backend,
        portable_pty_probe,
        winpty_probe,
    }
}

pub(super) fn supports_pty_with_override(
    windows_pty_backend_override: Option<WindowsPtyBackendOverride>,
) -> bool {
    select_pty_backend_with_override(
        windows_pty_backend_override,
        portable_pty_probe,
        super::super::winpty::supports_winpty,
    )
    .is_some()
}

pub(super) fn spawn_tty_session(
    cmd: &[String],
    cwd: &Path,
    windows_pty_backend_override: Option<WindowsPtyBackendOverride>,
    environment: &ProcessEnvironment,
) -> anyhow::Result<LiveSession> {
    match select_pty_backend_with_override(
        windows_pty_backend_override,
        portable_pty_probe,
        super::super::winpty::supports_winpty,
    ) {
        Some(PtyBackend::PortablePty) => spawn_pty(cmd, cwd, environment),
        Some(PtyBackend::Winpty) => {
            let (session, receiver) = super::super::winpty::spawn_winpty(
                cmd,
                cwd,
                winpty_environment_block(environment),
            )?;
            Ok(new_live_session(
                true,
                SessionChild::Winpty(session),
                receiver,
            ))
        }
        None => bail!("tty is not supported on this host"),
    }
}

fn build_metadata_line(label: &str, path: Option<&'static str>) -> String {
    match path {
        Some(path) => {
            let kind = if Path::new(path).is_file() {
                "file exists"
            } else if Path::new(path).is_dir() {
                "dir exists"
            } else {
                "missing on disk"
            };
            format!("{label}: {path} ({kind})")
        }
        None => format!("{label}: <not set>"),
    }
}

fn probe_line(label: &str, result: &Result<(), String>) -> String {
    match result {
        Ok(()) => format!("{label}: ok"),
        Err(err) => format!("{label}: {err}"),
    }
}

fn windows_status_name(code: u32) -> Option<&'static str> {
    match code {
        0xC000007B => Some("STATUS_INVALID_IMAGE_FORMAT"),
        0xC0000135 => Some("STATUS_DLL_NOT_FOUND"),
        0xC0000139 => Some("STATUS_ENTRYPOINT_NOT_FOUND"),
        0xC0000142 => Some("STATUS_DLL_INIT_FAILED"),
        _ => None,
    }
}

fn format_windows_exit_code(code: i32) -> String {
    let raw = code as u32;
    match windows_status_name(raw) {
        Some(name) => format!("{code} (0x{raw:08X}, {name})"),
        None => format!("{code} (0x{raw:08X})"),
    }
}

fn summarize_output_excerpt(output: &str) -> String {
    let normalized = output.replace('\r', "\\r").replace('\n', "\\n");
    if normalized.is_empty() {
        "<empty>".to_string()
    } else {
        let mut chars = normalized.chars();
        let excerpt = chars.by_ref().take(160).collect::<String>();
        if chars.next().is_some() {
            format!("{excerpt}...")
        } else {
            excerpt
        }
    }
}

fn winpty_environment_block(environment: &ProcessEnvironment) -> EnvBlock {
    let mut env_map = BTreeMap::<String, (String, OsString)>::new();

    for (key, value) in environment.vars() {
        let key_text = key.to_string_lossy().into_owned();
        env_map.insert(key_text.to_ascii_uppercase(), (key_text, value.clone()));
    }

    for key in ["LANG", "LC_CTYPE", "LC_ALL"] {
        env_map.remove(key);
    }

    for (key, value) in super::environment::normalized_pairs(environment) {
        env_map.insert(key.to_ascii_uppercase(), (key, OsString::from(value)));
    }

    EnvBlock::from_pairs(env_map.into_values())
}

async fn summarize_windows_backend_session(
    mut session: LiveSession,
    backend: PtyBackend,
) -> String {
    let deadline = Instant::now() + Duration::from_millis(300);
    let mut output = String::new();

    while Instant::now() < deadline {
        match session.read_available().await {
            Ok(chunk) => output.push_str(&chunk),
            Err(err) => {
                return format!(
                    "{} smoke test: failed to read output: {err}",
                    backend.debug_name()
                );
            }
        }

        match session.has_exited().await {
            Ok(true) => {
                if let Ok(tail) = session.read_available().await {
                    output.push_str(&tail);
                }
                let exit_code = session
                    .exit_code()
                    .map(format_windows_exit_code)
                    .unwrap_or_else(|| "<missing exit code>".to_string());
                return format!(
                    "{} smoke test: exited early with {exit_code}; output={}",
                    backend.debug_name(),
                    summarize_output_excerpt(&output)
                );
            }
            Ok(false) => {}
            Err(err) => {
                return format!(
                    "{} smoke test: failed to query exit status: {err}",
                    backend.debug_name()
                );
            }
        }

        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let _ = session.terminate().await;
    format!(
        "{} smoke test: still running after 300ms; output={}",
        backend.debug_name(),
        summarize_output_excerpt(&output)
    )
}

async fn smoke_test_windows_backend(
    backend: PtyBackend,
    cmd: &[String],
    cwd: &Path,
    environment: &ProcessEnvironment,
) -> String {
    match backend {
        PtyBackend::PortablePty => match spawn_pty(cmd, cwd, environment) {
            Ok(session) => summarize_windows_backend_session(session, backend).await,
            Err(err) => format!("{} smoke test: spawn failed: {err}", backend.debug_name()),
        },
        PtyBackend::Winpty => {
            match super::super::winpty::spawn_winpty(
                cmd,
                cwd,
                winpty_environment_block(environment),
            ) {
                Ok((session, receiver)) => {
                    let live_session =
                        new_live_session(true, SessionChild::Winpty(session), receiver);
                    summarize_windows_backend_session(live_session, backend).await
                }
                Err(err) => format!("{} smoke test: spawn failed: {err}", backend.debug_name()),
            }
        }
    }
}

pub(super) async fn debug_report(cmd: &[String], cwd: &Path) -> String {
    let diagnostics = collect_pty_diagnostics();
    let environment = ProcessEnvironment::capture_current();
    let mut lines = vec![
        "Windows PTY diagnostics".to_string(),
        format!("cwd: {}", cwd.display()),
        format!("argv: {cmd:?}"),
        format!(
            "selected backend: {}",
            diagnostics
                .selected_backend
                .map(PtyBackend::debug_name)
                .unwrap_or("none")
        ),
        probe_line("portable-pty ConPTY probe", &diagnostics.portable_pty_probe),
        probe_line("winpty probe", &diagnostics.winpty_probe),
        build_metadata_line("winpty link kind", option_env!("DEP_WINPTY_LINK_KIND")),
        build_metadata_line("winpty root", option_env!("DEP_WINPTY_WINPTY_ROOT")),
        build_metadata_line("winpty lib dir", option_env!("DEP_WINPTY_WINPTY_LIB_DIR")),
        build_metadata_line("winpty bin dir", option_env!("DEP_WINPTY_WINPTY_BIN_DIR")),
        build_metadata_line("winpty.dll", option_env!("DEP_WINPTY_WINPTY_DLL")),
        build_metadata_line("winpty-agent.exe", option_env!("DEP_WINPTY_WINPTY_RUNTIME")),
        build_metadata_line("conpty.dll", option_env!("DEP_WINPTY_CONPTY_DLL")),
        build_metadata_line("OpenConsole.exe", option_env!("DEP_WINPTY_CONPTY_RUNTIME")),
    ];

    lines.push(if diagnostics.portable_pty_probe.is_ok() {
        smoke_test_windows_backend(PtyBackend::PortablePty, cmd, cwd, &environment).await
    } else {
        format!(
            "conpty_via_portable_pty smoke test: skipped because probe failed: {}",
            diagnostics.portable_pty_probe.as_ref().err().unwrap()
        )
    });
    lines.push(if diagnostics.winpty_probe.is_ok() {
        smoke_test_windows_backend(PtyBackend::Winpty, cmd, cwd, &environment).await
    } else {
        format!(
            "winpty smoke test: skipped because probe failed: {}",
            diagnostics.winpty_probe.as_ref().err().unwrap()
        )
    });
    lines.push(
        "note: STATUS_DLL_INIT_FAILED / STATUS_DLL_NOT_FOUND identifies the failure class, not the exact missing DLL."
            .to_string(),
    );

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    use crate::config::WindowsPtyBackendOverride;

    #[cfg(windows)]
    use super::select_pty_backend_with_override;
    use super::{PtyBackend, TerminalQueryState, normalize_input, select_pty_backend_with};

    #[test]
    fn windows_pty_backend_prefers_portable_pty_when_both_backends_work() {
        assert_eq!(
            select_pty_backend_with(|| Ok(()), || Ok(())),
            Some(PtyBackend::PortablePty)
        );
    }

    #[test]
    fn windows_pty_backend_falls_back_to_winpty_when_portable_pty_is_unavailable() {
        assert_eq!(
            select_pty_backend_with(|| Err(anyhow::anyhow!("conpty unavailable")), || Ok(())),
            Some(PtyBackend::Winpty)
        );
    }

    #[test]
    fn windows_pty_backend_reports_no_support_when_both_backends_fail() {
        assert_eq!(
            select_pty_backend_with(
                || Err(anyhow::anyhow!("conpty unavailable")),
                || Err(anyhow::anyhow!("winpty unavailable"))
            ),
            None
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_pty_backend_override_forces_winpty_without_probing_portable_pty() {
        assert_eq!(
            select_pty_backend_with_override(
                Some(WindowsPtyBackendOverride::Winpty),
                || panic!("portable-pty probe should not run when winpty is forced"),
                || Ok(())
            ),
            Some(PtyBackend::Winpty)
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_pty_backend_override_reports_no_support_when_forced_backend_fails() {
        assert_eq!(
            select_pty_backend_with_override(
                Some(WindowsPtyBackendOverride::Winpty),
                || panic!("portable-pty probe should not run when winpty is forced"),
                || Err(anyhow::anyhow!("winpty unavailable"))
            ),
            None
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_terminal_query_state_replies_to_device_status_report() {
        let mut state = TerminalQueryState::default();
        let result = state.filter_chunk("before\x1b[5nafter");

        assert_eq!(result.output, "beforeafter");
        assert_eq!(result.response, "\x1b[0n");
        assert_eq!(state.drain_pending(), "");
    }

    #[cfg(windows)]
    #[test]
    fn windows_terminal_query_state_replies_to_cursor_position_report() {
        let mut state = TerminalQueryState::default();
        let result = state.filter_chunk("before\x1b[6nafter");

        assert_eq!(result.output, "beforeafter");
        assert_eq!(result.response, "\x1b[1;1R");
        assert_eq!(state.drain_pending(), "");
    }

    #[cfg(windows)]
    #[test]
    fn windows_terminal_query_state_handles_split_query_sequences() {
        let mut state = TerminalQueryState::default();

        let first = state.filter_chunk("before\x1b[");
        assert_eq!(first.output, "before");
        assert_eq!(first.response, "");

        let second = state.filter_chunk("6nafter");
        assert_eq!(second.output, "after");
        assert_eq!(second.response, "\x1b[1;1R");
        assert_eq!(state.drain_pending(), "");
    }

    #[cfg(windows)]
    #[test]
    fn windows_tty_input_normalization_converts_bare_lf_to_cr() {
        assert_eq!(normalize_input("ping\n", true).as_ref(), "ping\r");
    }

    #[cfg(windows)]
    #[test]
    fn windows_tty_input_normalization_coalesces_crlf_to_cr() {
        assert_eq!(
            normalize_input("ping\r\npong\n", true).as_ref(),
            "ping\rpong\r"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_tty_input_normalization_leaves_existing_cr_unchanged() {
        assert_eq!(normalize_input("ping\r", true).as_ref(), "ping\r");
    }
}
