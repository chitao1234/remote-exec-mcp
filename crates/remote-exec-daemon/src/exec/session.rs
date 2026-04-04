use std::io::{Read, Write};
use std::process::Stdio;
use std::time::Instant;

use anyhow::Context;
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use super::transcript::TranscriptBuffer;
use crate::config::{ProcessEnvironment, WindowsPtyBackendOverride};

#[cfg(windows)]
mod windows;

const NORMALIZED_ENV: [(&str, &str); 7] = [
    ("NO_COLOR", "1"),
    ("TERM", "dumb"),
    ("COLORTERM", ""),
    ("PAGER", "cat"),
    ("GIT_PAGER", "cat"),
    ("GH_PAGER", "cat"),
    ("CODEX_CI", "1"),
];
const TRANSCRIPT_LIMIT_BYTES: usize = 1024 * 1024;

pub struct LiveSession {
    pub tty: bool,
    pub started_at: Instant,
    pub transcript: TranscriptBuffer,
    pub(crate) child: SessionChild,
    receiver: UnboundedReceiver<String>,
    exit_code: Option<i32>,
    #[cfg(windows)]
    terminal_query_state: Option<windows::TerminalQueryState>,
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

fn new_live_session(
    tty: bool,
    child: SessionChild,
    receiver: UnboundedReceiver<String>,
) -> LiveSession {
    LiveSession {
        tty,
        started_at: Instant::now(),
        transcript: TranscriptBuffer::new(TRANSCRIPT_LIMIT_BYTES),
        child,
        receiver,
        exit_code: None,
        #[cfg(windows)]
        terminal_query_state: tty.then(windows::TerminalQueryState::default),
    }
}

fn default_pty_size() -> PtySize {
    PtySize {
        rows: 24,
        cols: 120,
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn portable_pty_probe() -> anyhow::Result<()> {
    NativePtySystem::default()
        .openpty(default_pty_size())
        .map(|_| ())
}

pub fn supports_pty_with_override(
    windows_pty_backend_override: Option<WindowsPtyBackendOverride>,
) -> bool {
    #[cfg(windows)]
    {
        windows::supports_pty_with_override(windows_pty_backend_override)
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
            windows::spawn_tty_session(cmd, cwd, windows_pty_backend_override, environment)
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
pub async fn windows_pty_debug_report(cmd: &[String], cwd: &std::path::Path) -> String {
    windows::debug_report(cmd, cwd).await
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

    Ok(new_live_session(
        true,
        SessionChild::Pty(PtySession {
            child,
            master: pty.master,
            writer,
        }),
        receiver,
    ))
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

    Ok(new_live_session(
        false,
        SessionChild::Pipe(Box::new(child)),
        receiver,
    ))
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
        let chars = windows::normalize_input(chars, self.tty);
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
            .map(windows::TerminalQueryState::drain_pending)
            .unwrap_or_default()
    }

    pub fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }

    pub fn record_output(&mut self, chunk: &str) {
        self.transcript.push(chunk.as_bytes());
    }
}
