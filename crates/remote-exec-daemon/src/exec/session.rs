#[cfg(windows)]
use std::collections::BTreeMap;
#[cfg(windows)]
use std::ffi::OsString;
use std::io::{Read, Write};
use std::process::Stdio;
use std::time::Instant;

use anyhow::Context;
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use super::transcript::TranscriptBuffer;

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
}

pub(crate) enum SessionChild {
    Pty(PtySession),
    #[cfg(windows)]
    Winpty(super::winpty::WinptySession),
    Pipe(Box<tokio::process::Child>),
}

pub struct PtySession {
    pub child: Box<dyn portable_pty::Child + Send>,
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
fn select_windows_pty_backend() -> Option<WindowsPtyBackend> {
    select_windows_pty_backend_with(portable_pty_probe, super::winpty::supports_winpty)
}

pub fn supports_pty() -> bool {
    #[cfg(windows)]
    {
        select_windows_pty_backend().is_some()
    }

    #[cfg(not(windows))]
    {
        portable_pty_probe().is_ok()
    }
}

pub fn spawn(cmd: &[String], cwd: &std::path::Path, tty: bool) -> anyhow::Result<LiveSession> {
    if tty {
        #[cfg(windows)]
        {
            match select_windows_pty_backend() {
                Some(WindowsPtyBackend::PortablePty) => spawn_pty(cmd, cwd),
                Some(WindowsPtyBackend::Winpty) => {
                    let (session, receiver) =
                        super::winpty::spawn_winpty(cmd, cwd, winpty_environment_block())?;
                    Ok(LiveSession {
                        tty: true,
                        started_at: Instant::now(),
                        transcript: TranscriptBuffer::new(1024 * 1024),
                        child: SessionChild::Winpty(session),
                        receiver,
                        exit_code: None,
                    })
                }
                None => anyhow::bail!("tty is not supported on this host"),
            }
        }

        #[cfg(not(windows))]
        {
            anyhow::ensure!(supports_pty(), "tty is not supported on this host");
            spawn_pty(cmd, cwd)
        }
    } else {
        spawn_pipe(cmd, cwd)
    }
}

fn normalized_env_pairs() -> Vec<(String, String)> {
    let mut pairs = NORMALIZED_ENV
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect::<Vec<_>>();
    pairs.extend(super::locale::LocaleEnvPlan::resolved().as_pairs());
    pairs
}

fn apply_env_overlay_builder(builder: &mut CommandBuilder) {
    builder.env_remove("LANG");
    builder.env_remove("LC_CTYPE");
    builder.env_remove("LC_ALL");
    for (key, value) in normalized_env_pairs() {
        builder.env(&key, &value);
    }
}

fn apply_env_overlay_command(command: &mut Command) {
    command.env_remove("LANG");
    command.env_remove("LC_CTYPE");
    command.env_remove("LC_ALL");
    for (key, value) in normalized_env_pairs() {
        command.env(&key, &value);
    }
}

#[cfg(windows)]
fn winpty_environment_block() -> OsString {
    let mut environment = BTreeMap::<String, (String, OsString)>::new();

    for (key, value) in std::env::vars_os() {
        let key_text = key.to_string_lossy().into_owned();
        environment.insert(key_text.to_ascii_uppercase(), (key_text, value));
    }

    for key in ["LANG", "LC_CTYPE", "LC_ALL"] {
        environment.remove(key);
    }

    for (key, value) in normalized_env_pairs() {
        environment.insert(key.to_ascii_uppercase(), (key, OsString::from(value)));
    }

    let mut block = OsString::new();
    for (_normalized, (key, value)) in environment {
        let mut entry = OsString::from(key);
        entry.push("=");
        entry.push(&value);
        block.push(entry);
        block.push("\0");
    }
    block
}

fn spawn_pty(cmd: &[String], cwd: &std::path::Path) -> anyhow::Result<LiveSession> {
    let pty = NativePtySystem::default().openpty(default_pty_size())?;
    let mut builder = CommandBuilder::new(&cmd[0]);
    for arg in &cmd[1..] {
        builder.arg(arg);
    }
    builder.cwd(cwd);
    apply_env_overlay_builder(&mut builder);

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
        child: SessionChild::Pty(PtySession { child, writer }),
        receiver,
        exit_code: None,
    })
}

fn spawn_pipe(cmd: &[String], cwd: &std::path::Path) -> anyhow::Result<LiveSession> {
    let mut command = Command::new(&cmd[0]);
    command
        .args(&cmd[1..])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    apply_env_overlay_command(&mut command);
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
            output.push_str(&chunk);
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
        if chars.is_empty() {
            return Ok(());
        }

        match &mut self.child {
            SessionChild::Pty(pty) => {
                pty.writer.write_all(chars.as_bytes())?;
                pty.writer.flush()?;
                Ok(())
            }
            #[cfg(windows)]
            SessionChild::Winpty(pty) => pty.write(chars),
            SessionChild::Pipe(_) => anyhow::bail!(
                "stdin is closed for this session; rerun exec_command with tty=true to keep stdin open"
            ),
        }
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
}
