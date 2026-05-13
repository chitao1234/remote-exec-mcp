#[cfg(feature = "winpty")]
use std::collections::BTreeMap;
#[cfg(feature = "winpty")]
use std::ffi::OsString;
use std::path::Path;
use std::time::Instant;

use anyhow::bail;
#[cfg(feature = "winpty")]
use winptyrs::EnvBlock;

use crate::config::{ProcessEnvironment, WindowsPtyBackendOverride};

use super::{LiveSession, portable_pty_probe, spawn_pty};
#[cfg(feature = "winpty")]
use super::{SessionChild, new_live_session};

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

#[cfg(not(feature = "winpty"))]
fn winpty_feature_disabled_message() -> &'static str {
    "winpty backend is not compiled in; enable the `winpty` cargo feature"
}

#[cfg(feature = "winpty")]
fn compiled_winpty_probe() -> anyhow::Result<()> {
    super::super::winpty::supports_winpty()
}

#[cfg(not(feature = "winpty"))]
fn compiled_winpty_probe() -> anyhow::Result<()> {
    bail!("{}", winpty_feature_disabled_message())
}

#[cfg(feature = "winpty")]
fn spawn_compiled_winpty_session(
    cmd: &[String],
    cwd: &Path,
    environment: &ProcessEnvironment,
) -> anyhow::Result<LiveSession> {
    let (session, receiver) =
        super::super::winpty::spawn_winpty(cmd, cwd, winpty_environment_block(environment))?;
    Ok(new_live_session(
        true,
        SessionChild::Winpty(session),
        receiver,
    ))
}

#[cfg(not(feature = "winpty"))]
fn spawn_compiled_winpty_session(
    cmd: &[String],
    cwd: &Path,
    environment: &ProcessEnvironment,
) -> anyhow::Result<LiveSession> {
    let _ = (cmd, cwd, environment);
    bail!("{}", winpty_feature_disabled_message())
}

fn collect_pty_diagnostics() -> PtyDiagnostics {
    let portable_pty_probe = portable_pty_probe().map_err(|err| err.to_string());
    let winpty_probe = compiled_winpty_probe().map_err(|err| err.to_string());
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
        compiled_winpty_probe,
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
        compiled_winpty_probe,
    ) {
        Some(PtyBackend::PortablePty) => spawn_pty(cmd, cwd, environment),
        Some(PtyBackend::Winpty) => spawn_compiled_winpty_session(cmd, cwd, environment),
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

fn skipped_smoke_test_line(backend: PtyBackend, probe: &Result<(), String>) -> String {
    let reason = probe
        .as_ref()
        .err()
        .map(String::as_str)
        .unwrap_or("unknown probe failure");
    format!(
        "{} smoke test: skipped because probe failed: {reason}",
        backend.debug_name()
    )
}

#[cfg(feature = "winpty")]
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
    let deadline = Instant::now() + super::super::timing::WINDOWS_BACKEND_SMOKE_TIMEOUT;
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

        tokio::time::sleep(super::super::timing::EXEC_POLL_INTERVAL).await;
    }

    let _ = session.terminate().await;
    let timeout_ms = super::super::timing::WINDOWS_BACKEND_SMOKE_TIMEOUT.as_millis();
    format!(
        "{} smoke test: still running after {timeout_ms}ms; output={}",
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
        PtyBackend::Winpty => match spawn_compiled_winpty_session(cmd, cwd, environment) {
            Ok(session) => summarize_windows_backend_session(session, backend).await,
            Err(err) => format!("{} smoke test: spawn failed: {err}", backend.debug_name()),
        },
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
        skipped_smoke_test_line(PtyBackend::PortablePty, &diagnostics.portable_pty_probe)
    });
    lines.push(if diagnostics.winpty_probe.is_ok() {
        smoke_test_windows_backend(PtyBackend::Winpty, cmd, cwd, &environment).await
    } else {
        skipped_smoke_test_line(PtyBackend::Winpty, &diagnostics.winpty_probe)
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
    use super::{PtyBackend, select_pty_backend_with};

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
}
