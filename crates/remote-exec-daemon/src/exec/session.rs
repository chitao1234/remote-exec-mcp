use std::io::{Read, Write};
use std::process::Stdio;
use std::time::Instant;

use anyhow::Context;
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use super::transcript::TranscriptBuffer;

const NORMALIZED_ENV: [(&str, &str); 10] = [
    ("NO_COLOR", "1"),
    ("TERM", "dumb"),
    ("LANG", "C.UTF-8"),
    ("LC_CTYPE", "C.UTF-8"),
    ("LC_ALL", "C.UTF-8"),
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
    pub child: SessionChild,
    receiver: UnboundedReceiver<String>,
    exit_code: Option<i32>,
}

pub enum SessionChild {
    Pty(PtySession),
    Pipe(tokio::process::Child),
}

pub struct PtySession {
    pub child: Box<dyn portable_pty::Child + Send>,
    pub writer: Box<dyn Write + Send>,
}

pub fn spawn(cmd: &[String], cwd: &std::path::Path, tty: bool) -> anyhow::Result<LiveSession> {
    if tty {
        spawn_pty(cmd, cwd)
    } else {
        spawn_pipe(cmd, cwd)
    }
}

fn apply_env_overlay_builder(builder: &mut CommandBuilder) {
    for (key, value) in NORMALIZED_ENV {
        builder.env(key, value);
    }
}

fn apply_env_overlay_command(command: &mut Command) {
    for (key, value) in NORMALIZED_ENV {
        command.env(key, value);
    }
}

fn spawn_pty(cmd: &[String], cwd: &std::path::Path) -> anyhow::Result<LiveSession> {
    let pty = NativePtySystem::default().openpty(PtySize {
        rows: 24,
        cols: 120,
        pixel_width: 0,
        pixel_height: 0,
    })?;
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
        child: SessionChild::Pipe(child),
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
            SessionChild::Pipe(child) => {
                if let Some(status) = child.try_wait()? {
                    self.exit_code = status.code();
                    return Ok(true);
                }
            }
        }

        Ok(false)
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
