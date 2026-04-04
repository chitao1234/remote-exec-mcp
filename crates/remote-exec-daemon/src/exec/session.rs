#[cfg(windows)]
use std::borrow::Cow;
#[cfg(windows)]
use std::collections::BTreeMap;
#[cfg(windows)]
use std::ffi::OsString;
use std::io::{Read, Write};
#[cfg(windows)]
use std::path::Path;
use std::process::Stdio;
#[cfg(windows)]
use std::time::Duration;
use std::time::Instant;

use anyhow::Context;
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
#[cfg(windows)]
use winptyrs::EnvBlock;

use super::transcript::TranscriptBuffer;
#[cfg(not(windows))]
use crate::config::ProcessEnvironment;
#[cfg(windows)]
use crate::config::{ProcessEnvironment, WindowsPtyBackendOverride};

const NORMALIZED_ENV: [(&str, &str); 7] = [
    ("NO_COLOR", "1"),
    ("TERM", "dumb"),
    ("COLORTERM", ""),
    ("PAGER", "cat"),
    ("GIT_PAGER", "cat"),
    ("GH_PAGER", "cat"),
    ("CODEX_CI", "1"),
];

pub struct LiveSession {
    pub tty: bool,
    pub started_at: Instant,
    pub transcript: TranscriptBuffer,
    pub(crate) child: SessionChild,
    receiver: UnboundedReceiver<String>,
    exit_code: Option<i32>,
    #[cfg(windows)]
    terminal_query_state: Option<WindowsTerminalQueryState>,
}

pub(crate) enum SessionChild {
    Pty(PtySession),
    #[cfg(windows)]
    Winpty(super::winpty::WinptySession),
    Pipe(Box<tokio::process::Child>),
}

pub struct PtySession {
    pub child: Box<dyn portable_pty::Child + Send>,
    pub master: Box<dyn portable_pty::MasterPty + Send>,
    pub writer: Box<dyn Write + Send>,
}

fn default_pty_size() -> PtySize {
    PtySize {
        rows: 24,
        cols: 120,
        pixel_width: 0,
        pixel_height: 0,
    }
}

#[cfg(any(test, windows))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowsPtyBackend {
    PortablePty,
    Winpty,
}

#[cfg(windows)]
impl WindowsPtyBackend {
    fn debug_name(self) -> &'static str {
        match self {
            Self::PortablePty => "conpty_via_portable_pty",
            Self::Winpty => "winpty",
        }
    }
}

#[cfg(windows)]
#[derive(Debug, Clone)]
struct WindowsPtyDiagnostics {
    selected_backend: Option<WindowsPtyBackend>,
    portable_pty_probe: Result<(), String>,
    winpty_probe: Result<(), String>,
}

#[cfg(windows)]
#[derive(Debug, Default)]
struct WindowsTerminalQueryState {
    pending: Vec<u8>,
}

#[cfg(windows)]
#[derive(Debug, Default)]
struct WindowsTerminalQueryResult {
    output: String,
    response: String,
}

#[cfg(windows)]
impl WindowsTerminalQueryState {
    fn filter_chunk(&mut self, chunk: &str) -> WindowsTerminalQueryResult {
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

        WindowsTerminalQueryResult {
            output: String::from_utf8_lossy(&visible).into_owned(),
            response,
        }
    }

    fn drain_pending(&mut self) -> String {
        let pending = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending.clear();
        pending
    }
}

#[cfg(windows)]
fn terminal_query_prefix_len(bytes: &[u8]) -> Option<usize> {
    match bytes {
        [0x1b] => Some(1),
        [0x1b, b'['] => Some(2),
        [0x1b, b'[', b'5'] | [0x1b, b'[', b'6'] => Some(3),
        _ => None,
    }
}

#[cfg(windows)]
fn normalize_windows_tty_input(chars: &str) -> Cow<'_, str> {
    if !chars.contains('\n') {
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

fn portable_pty_probe() -> anyhow::Result<()> {
    NativePtySystem::default()
        .openpty(default_pty_size())
        .map(|_| ())
}

#[cfg(any(test, windows))]
fn select_windows_pty_backend_with(
    portable_probe: impl FnOnce() -> anyhow::Result<()>,
    winpty_probe: impl FnOnce() -> anyhow::Result<()>,
) -> Option<WindowsPtyBackend> {
    if portable_probe().is_ok() {
        return Some(WindowsPtyBackend::PortablePty);
    }
    if winpty_probe().is_ok() {
        return Some(WindowsPtyBackend::Winpty);
    }
    None
}

#[cfg(windows)]
fn select_windows_pty_backend_with_override(
    windows_pty_backend_override: Option<WindowsPtyBackendOverride>,
    portable_probe: impl FnOnce() -> anyhow::Result<()>,
    winpty_probe: impl FnOnce() -> anyhow::Result<()>,
) -> Option<WindowsPtyBackend> {
    match windows_pty_backend_override {
        Some(WindowsPtyBackendOverride::PortablePty) => portable_probe()
            .ok()
            .map(|_| WindowsPtyBackend::PortablePty),
        Some(WindowsPtyBackendOverride::Winpty) => {
            winpty_probe().ok().map(|_| WindowsPtyBackend::Winpty)
        }
        None => select_windows_pty_backend_with(portable_probe, winpty_probe),
    }
}

#[cfg(windows)]
fn collect_windows_pty_diagnostics() -> WindowsPtyDiagnostics {
    let portable_pty_probe = portable_pty_probe().map_err(|err| err.to_string());
    let winpty_probe = super::winpty::supports_winpty().map_err(|err| err.to_string());
    let selected_backend = if portable_pty_probe.is_ok() {
        Some(WindowsPtyBackend::PortablePty)
    } else if winpty_probe.is_ok() {
        Some(WindowsPtyBackend::Winpty)
    } else {
        None
    };

    WindowsPtyDiagnostics {
        selected_backend,
        portable_pty_probe,
        winpty_probe,
    }
}

pub fn supports_pty_with_override(
    windows_pty_backend_override: Option<WindowsPtyBackendOverride>,
) -> bool {
    #[cfg(windows)]
    {
        select_windows_pty_backend_with_override(
            windows_pty_backend_override,
            portable_pty_probe,
            super::winpty::supports_winpty,
        )
        .is_some()
    }

    #[cfg(not(windows))]
    {
        let _ = windows_pty_backend_override;
        portable_pty_probe().is_ok()
    }
}

pub fn supports_pty() -> bool {
    supports_pty_with_override(None)
}

#[cfg(windows)]
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

#[cfg(windows)]
fn probe_line(label: &str, result: &Result<(), String>) -> String {
    match result {
        Ok(()) => format!("{label}: ok"),
        Err(err) => format!("{label}: {err}"),
    }
}

#[cfg(windows)]
fn windows_status_name(code: u32) -> Option<&'static str> {
    match code {
        0xC000007B => Some("STATUS_INVALID_IMAGE_FORMAT"),
        0xC0000135 => Some("STATUS_DLL_NOT_FOUND"),
        0xC0000139 => Some("STATUS_ENTRYPOINT_NOT_FOUND"),
        0xC0000142 => Some("STATUS_DLL_INIT_FAILED"),
        _ => None,
    }
}

#[cfg(windows)]
fn format_windows_exit_code(code: i32) -> String {
    let raw = code as u32;
    match windows_status_name(raw) {
        Some(name) => format!("{code} (0x{raw:08X}, {name})"),
        None => format!("{code} (0x{raw:08X})"),
    }
}

#[cfg(windows)]
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

pub fn spawn_with_windows_pty_backend_override(
    cmd: &[String],
    cwd: &std::path::Path,
    tty: bool,
    windows_pty_backend_override: Option<WindowsPtyBackendOverride>,
    environment: &ProcessEnvironment,
) -> anyhow::Result<LiveSession> {
    if tty {
        #[cfg(windows)]
        {
            match select_windows_pty_backend_with_override(
                windows_pty_backend_override,
                portable_pty_probe,
                super::winpty::supports_winpty,
            ) {
                Some(WindowsPtyBackend::PortablePty) => spawn_pty(cmd, cwd, environment),
                Some(WindowsPtyBackend::Winpty) => {
                    let (session, receiver) = super::winpty::spawn_winpty(
                        cmd,
                        cwd,
                        winpty_environment_block(environment),
                    )?;
                    Ok(LiveSession {
                        tty: true,
                        started_at: Instant::now(),
                        transcript: TranscriptBuffer::new(1024 * 1024),
                        child: SessionChild::Winpty(session),
                        receiver,
                        exit_code: None,
                        terminal_query_state: Some(WindowsTerminalQueryState::default()),
                    })
                }
                None => anyhow::bail!("tty is not supported on this host"),
            }
        }

        #[cfg(not(windows))]
        {
            let _ = windows_pty_backend_override;
            anyhow::ensure!(supports_pty(), "tty is not supported on this host");
            spawn_pty(cmd, cwd, environment)
        }
    } else {
        spawn_pipe(cmd, cwd, environment)
    }
}

pub fn spawn(
    cmd: &[String],
    cwd: &std::path::Path,
    tty: bool,
    environment: &ProcessEnvironment,
) -> anyhow::Result<LiveSession> {
    spawn_with_windows_pty_backend_override(cmd, cwd, tty, None, environment)
}

fn normalized_env_pairs(environment: &ProcessEnvironment) -> Vec<(String, String)> {
    let mut pairs = NORMALIZED_ENV
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect::<Vec<_>>();
    pairs.extend(super::locale::LocaleEnvPlan::resolved(environment).as_pairs());
    pairs
}

fn apply_base_environment_builder(builder: &mut CommandBuilder, environment: &ProcessEnvironment) {
    builder.env_clear();
    for (key, value) in environment.vars() {
        builder.env(key, value);
    }
}

fn apply_env_overlay_builder(builder: &mut CommandBuilder, environment: &ProcessEnvironment) {
    apply_base_environment_builder(builder, environment);
    builder.env_remove("LANG");
    builder.env_remove("LC_CTYPE");
    builder.env_remove("LC_ALL");
    for (key, value) in normalized_env_pairs(environment) {
        builder.env(&key, &value);
    }
}

fn apply_base_environment_command(command: &mut Command, environment: &ProcessEnvironment) {
    command.env_clear();
    for (key, value) in environment.vars() {
        command.env(key, value);
    }
}

fn apply_env_overlay_command(command: &mut Command, environment: &ProcessEnvironment) {
    apply_base_environment_command(command, environment);
    command.env_remove("LANG");
    command.env_remove("LC_CTYPE");
    command.env_remove("LC_ALL");
    for (key, value) in normalized_env_pairs(environment) {
        command.env(&key, &value);
    }
}

#[cfg(windows)]
fn winpty_environment_block(environment: &ProcessEnvironment) -> EnvBlock {
    let mut env_map = BTreeMap::<String, (String, OsString)>::new();

    for (key, value) in environment.vars() {
        let key_text = key.to_string_lossy().into_owned();
        env_map.insert(key_text.to_ascii_uppercase(), (key_text, value.clone()));
    }

    for key in ["LANG", "LC_CTYPE", "LC_ALL"] {
        env_map.remove(key);
    }

    for (key, value) in normalized_env_pairs(environment) {
        env_map.insert(key.to_ascii_uppercase(), (key, OsString::from(value)));
    }

    EnvBlock::from_pairs(env_map.into_values())
}

#[cfg(windows)]
async fn summarize_windows_backend_session(
    mut session: LiveSession,
    backend: WindowsPtyBackend,
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

#[cfg(windows)]
async fn smoke_test_windows_backend(
    backend: WindowsPtyBackend,
    cmd: &[String],
    cwd: &Path,
    environment: &ProcessEnvironment,
) -> String {
    match backend {
        WindowsPtyBackend::PortablePty => match spawn_pty(cmd, cwd, environment) {
            Ok(session) => summarize_windows_backend_session(session, backend).await,
            Err(err) => format!("{} smoke test: spawn failed: {err}", backend.debug_name()),
        },
        WindowsPtyBackend::Winpty => {
            match super::winpty::spawn_winpty(cmd, cwd, winpty_environment_block(environment)) {
                Ok((session, receiver)) => {
                    let live_session = LiveSession {
                        tty: true,
                        started_at: Instant::now(),
                        transcript: TranscriptBuffer::new(1024 * 1024),
                        child: SessionChild::Winpty(session),
                        receiver,
                        exit_code: None,
                        terminal_query_state: Some(WindowsTerminalQueryState::default()),
                    };
                    summarize_windows_backend_session(live_session, backend).await
                }
                Err(err) => format!("{} smoke test: spawn failed: {err}", backend.debug_name()),
            }
        }
    }
}

#[cfg(windows)]
pub async fn windows_pty_debug_report(cmd: &[String], cwd: &Path) -> String {
    let diagnostics = collect_windows_pty_diagnostics();
    let environment = ProcessEnvironment::capture_current();
    let mut lines = vec![
        "Windows PTY diagnostics".to_string(),
        format!("cwd: {}", cwd.display()),
        format!("argv: {cmd:?}"),
        format!(
            "selected backend: {}",
            diagnostics
                .selected_backend
                .map(WindowsPtyBackend::debug_name)
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
        smoke_test_windows_backend(WindowsPtyBackend::PortablePty, cmd, cwd, &environment).await
    } else {
        format!(
            "conpty_via_portable_pty smoke test: skipped because probe failed: {}",
            diagnostics.portable_pty_probe.as_ref().err().unwrap()
        )
    });
    lines.push(if diagnostics.winpty_probe.is_ok() {
        smoke_test_windows_backend(WindowsPtyBackend::Winpty, cmd, cwd, &environment).await
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

fn spawn_pty(
    cmd: &[String],
    cwd: &std::path::Path,
    environment: &ProcessEnvironment,
) -> anyhow::Result<LiveSession> {
    let pty = NativePtySystem::default().openpty(default_pty_size())?;
    let mut builder = CommandBuilder::new(&cmd[0]);
    for arg in &cmd[1..] {
        builder.arg(arg);
    }
    builder.cwd(cwd);
    apply_env_overlay_builder(&mut builder, environment);

    let child = pty.slave.spawn_command(builder)?;
    let writer = pty.master.take_writer()?;
    let mut reader = pty.master.try_clone_reader()?;
    let (sender, receiver) = unbounded_channel();

    std::thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    if sender
                        .send(String::from_utf8_lossy(&buffer[..read]).into_owned())
                        .is_err()
                    {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    Ok(LiveSession {
        tty: true,
        started_at: Instant::now(),
        transcript: TranscriptBuffer::new(1024 * 1024),
        child: SessionChild::Pty(PtySession {
            child,
            master: pty.master,
            writer,
        }),
        receiver,
        exit_code: None,
        #[cfg(windows)]
        terminal_query_state: Some(WindowsTerminalQueryState::default()),
    })
}

fn spawn_pipe(
    cmd: &[String],
    cwd: &std::path::Path,
    environment: &ProcessEnvironment,
) -> anyhow::Result<LiveSession> {
    let mut command = Command::new(&cmd[0]);
    command
        .args(&cmd[1..])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    apply_env_overlay_command(&mut command, environment);
    let mut child = command.spawn()?;
    let stdout = child.stdout.take().context("missing stdout pipe")?;
    let stderr = child.stderr.take().context("missing stderr pipe")?;
    let (sender, receiver) = unbounded_channel();

    spawn_pipe_reader(stdout, sender.clone());
    spawn_pipe_reader(stderr, sender);

    Ok(LiveSession {
        tty: false,
        started_at: Instant::now(),
        transcript: TranscriptBuffer::new(1024 * 1024),
        child: SessionChild::Pipe(Box::new(child)),
        receiver,
        exit_code: None,
        #[cfg(windows)]
        terminal_query_state: None,
    })
}

fn spawn_pipe_reader<R>(mut reader: R, sender: UnboundedSender<String>)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut buffer = [0u8; 8192];
        loop {
            match reader.read(&mut buffer).await {
                Ok(0) => break,
                Ok(read) => {
                    if sender
                        .send(String::from_utf8_lossy(&buffer[..read]).into_owned())
                        .is_err()
                    {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
}

impl LiveSession {
    pub async fn read_available(&mut self) -> anyhow::Result<String> {
        let mut output = String::new();
        while let Ok(chunk) = self.receiver.try_recv() {
            #[cfg(windows)]
            let chunk = self.filter_terminal_queries(chunk)?;
            output.push_str(&chunk);
        }

        #[cfg(windows)]
        if self.exit_code.is_some() {
            output.push_str(&self.drain_terminal_query_buffer());
        }

        Ok(output)
    }

    pub async fn has_exited(&mut self) -> anyhow::Result<bool> {
        match &mut self.child {
            SessionChild::Pty(pty) => {
                if let Some(status) = pty.child.try_wait()? {
                    self.exit_code = Some(status.exit_code() as i32);
                    return Ok(true);
                }
            }
            #[cfg(windows)]
            SessionChild::Winpty(pty) => {
                if let Some(status) = pty.try_wait()? {
                    self.exit_code = Some(status);
                    return Ok(true);
                }
            }
            SessionChild::Pipe(child) => {
                if let Some(status) = child.try_wait()? {
                    self.exit_code = status.code();
                    return Ok(true);
                }
            }
        }

        Ok(false)
    }

    pub async fn terminate(&mut self) -> anyhow::Result<()> {
        if self.has_exited().await? {
            return Ok(());
        }

        match &mut self.child {
            SessionChild::Pty(pty) => {
                let _ = pty.child.kill();
                let _ = pty.child.try_wait()?;
            }
            #[cfg(windows)]
            SessionChild::Winpty(pty) => {
                let _ = pty.terminate();
            }
            SessionChild::Pipe(child) => {
                let _ = child.start_kill();
                let _ = child.try_wait()?;
            }
        }

        Ok(())
    }

    pub async fn write(&mut self, chars: &str) -> anyhow::Result<()> {
        self.write_chars_internal(chars)
    }

    fn write_chars_internal(&mut self, chars: &str) -> anyhow::Result<()> {
        if chars.is_empty() {
            return Ok(());
        }

        #[cfg(windows)]
        let chars = if self.tty {
            normalize_windows_tty_input(chars)
        } else {
            Cow::Borrowed(chars)
        };
        #[cfg(not(windows))]
        let chars = chars;

        match &mut self.child {
            SessionChild::Pty(pty) => {
                pty.writer.write_all(chars.as_bytes())?;
                pty.writer.flush()?;
                Ok(())
            }
            #[cfg(windows)]
            SessionChild::Winpty(pty) => pty.write(chars.as_ref()),
            SessionChild::Pipe(_) => anyhow::bail!(
                "stdin is closed for this session; rerun exec_command with tty=true to keep stdin open"
            ),
        }
    }

    #[cfg(windows)]
    fn filter_terminal_queries(&mut self, chunk: String) -> anyhow::Result<String> {
        let Some(result) = self
            .terminal_query_state
            .as_mut()
            .map(|state| state.filter_chunk(&chunk))
        else {
            return Ok(chunk);
        };

        self.write_chars_internal(&result.response)?;
        Ok(result.output)
    }

    #[cfg(windows)]
    fn drain_terminal_query_buffer(&mut self) -> String {
        self.terminal_query_state
            .as_mut()
            .map(WindowsTerminalQueryState::drain_pending)
            .unwrap_or_default()
    }

    pub fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }

    pub fn record_output(&mut self, chunk: &str) {
        self.transcript.push(chunk.as_bytes());
    }
}

#[cfg(test)]
mod windows_pty_backend_tests {
    #[cfg(windows)]
    use crate::config::WindowsPtyBackendOverride;

    #[cfg(windows)]
    use super::WindowsTerminalQueryState;
    #[cfg(windows)]
    use super::select_windows_pty_backend_with_override;
    use super::{WindowsPtyBackend, select_windows_pty_backend_with};

    #[test]
    fn windows_pty_backend_prefers_portable_pty_when_both_backends_work() {
        assert_eq!(
            select_windows_pty_backend_with(|| Ok(()), || Ok(())),
            Some(WindowsPtyBackend::PortablePty)
        );
    }

    #[test]
    fn windows_pty_backend_falls_back_to_winpty_when_portable_pty_is_unavailable() {
        assert_eq!(
            select_windows_pty_backend_with(
                || Err(anyhow::anyhow!("conpty unavailable")),
                || Ok(())
            ),
            Some(WindowsPtyBackend::Winpty)
        );
    }

    #[test]
    fn windows_pty_backend_reports_no_support_when_both_backends_fail() {
        assert_eq!(
            select_windows_pty_backend_with(
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
            select_windows_pty_backend_with_override(
                Some(WindowsPtyBackendOverride::Winpty),
                || panic!("portable-pty probe should not run when winpty is forced"),
                || Ok(())
            ),
            Some(WindowsPtyBackend::Winpty)
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_pty_backend_override_reports_no_support_when_forced_backend_fails() {
        assert_eq!(
            select_windows_pty_backend_with_override(
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
        let mut state = WindowsTerminalQueryState::default();
        let result = state.filter_chunk("before\x1b[5nafter");

        assert_eq!(result.output, "beforeafter");
        assert_eq!(result.response, "\x1b[0n");
        assert_eq!(state.drain_pending(), "");
    }

    #[cfg(windows)]
    #[test]
    fn windows_terminal_query_state_replies_to_cursor_position_report() {
        let mut state = WindowsTerminalQueryState::default();
        let result = state.filter_chunk("before\x1b[6nafter");

        assert_eq!(result.output, "beforeafter");
        assert_eq!(result.response, "\x1b[1;1R");
        assert_eq!(state.drain_pending(), "");
    }

    #[cfg(windows)]
    #[test]
    fn windows_terminal_query_state_handles_split_query_sequences() {
        let mut state = WindowsTerminalQueryState::default();

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
        assert_eq!(
            super::normalize_windows_tty_input("ping\n").as_ref(),
            "ping\r"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_tty_input_normalization_coalesces_crlf_to_cr() {
        assert_eq!(
            super::normalize_windows_tty_input("ping\r\npong\n").as_ref(),
            "ping\rpong\r"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_tty_input_normalization_leaves_existing_cr_unchanged() {
        assert_eq!(
            super::normalize_windows_tty_input("ping\r").as_ref(),
            "ping\r"
        );
    }
}
