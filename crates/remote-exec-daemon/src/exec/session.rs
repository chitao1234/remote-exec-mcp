use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use super::transcript::TranscriptBuffer;
use crate::config::{ProcessEnvironment, PtyMode, WindowsPtyBackendOverride};

mod environment;
#[cfg(windows)]
mod windows;
const TRANSCRIPT_LIMIT_BYTES: usize = 1024 * 1024;

pub struct LiveSession {
    pub tty: bool,
    pub started_at: Instant,
    pub transcript: TranscriptBuffer,
    pub(crate) child: SessionChild,
    receiver: UnboundedReceiver<String>,
    exit_code: Option<i32>,
    #[cfg(windows)]
    terminal_output_state: Option<windows::TerminalOutputState>,
}

pub(crate) enum OutputWait {
    Chunk(String),
    Closed,
    TimedOut,
}

pub(crate) enum SessionChild {
    Pty(PtySession),
    #[cfg(all(windows, feature = "winpty"))]
    Winpty(super::winpty::WinptySession),
    Pipe(Box<std::process::Child>),
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
        terminal_output_state: tty.then(windows::TerminalOutputState::default),
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

pub fn windows_pty_backend_override_for_mode(
    pty_mode: PtyMode,
) -> anyhow::Result<Option<WindowsPtyBackendOverride>> {
    match pty_mode {
        PtyMode::Auto | PtyMode::None => Ok(None),
        PtyMode::Conpty => {
            #[cfg(windows)]
            {
                Ok(Some(WindowsPtyBackendOverride::PortablePty))
            }
            #[cfg(not(windows))]
            {
                anyhow::bail!("configured PTY backend `conpty` is only supported on Windows");
            }
        }
        PtyMode::Winpty => {
            #[cfg(windows)]
            {
                Ok(Some(WindowsPtyBackendOverride::Winpty))
            }
            #[cfg(not(windows))]
            {
                anyhow::bail!("configured PTY backend `winpty` is only supported on Windows");
            }
        }
    }
}

pub fn supports_pty_for_mode(pty_mode: PtyMode) -> bool {
    if matches!(pty_mode, PtyMode::None) {
        return false;
    }

    let Ok(windows_pty_backend_override) = windows_pty_backend_override_for_mode(pty_mode) else {
        return false;
    };
    supports_pty_with_override(windows_pty_backend_override)
}

pub fn validate_pty_mode(pty_mode: PtyMode) -> anyhow::Result<()> {
    if matches!(pty_mode, PtyMode::Auto | PtyMode::None) {
        return Ok(());
    }

    anyhow::ensure!(
        supports_pty_for_mode(pty_mode),
        "configured PTY backend `{}` is not available on this host",
        match pty_mode {
            PtyMode::Conpty => "conpty",
            PtyMode::Winpty => "winpty",
            PtyMode::Auto | PtyMode::None => unreachable!("validated above"),
        }
    );
    Ok(())
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
    environment::apply_overlay_builder(&mut builder, environment);

    let child = pty.slave.spawn_command(builder)?;
    let writer = pty.master.take_writer()?;
    let reader = pty.master.try_clone_reader()?;
    let (sender, receiver) = unbounded_channel();
    spawn_output_reader(reader, sender);

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
    let (reader, writer) = std::io::pipe()?;
    let stderr = writer.try_clone()?;
    let mut command = Command::new(&cmd[0]);
    command
        .args(&cmd[1..])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::from(writer))
        .stderr(Stdio::from(stderr));
    environment::apply_overlay_std_command(&mut command, environment);
    let child = command.spawn()?;
    let (sender, receiver) = unbounded_channel();
    let session = new_live_session(false, SessionChild::Pipe(Box::new(child)), receiver);

    let _ = (cmd, cwd);
    spawn_output_reader(reader, sender);

    Ok(session)
}

fn spawn_output_reader<R>(mut reader: R, sender: UnboundedSender<String>)
where
    R: Read + Send + 'static,
{
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
}

impl LiveSession {
    pub async fn read_available(&mut self) -> anyhow::Result<String> {
        let mut output = String::new();
        while let Ok(chunk) = self.receiver.try_recv() {
            #[cfg(windows)]
            let chunk = self.filter_terminal_output(chunk)?;
            output.push_str(&chunk);
        }

        #[cfg(windows)]
        if self.exit_code.is_some() {
            output.push_str(&self.drain_terminal_output_buffer());
        }

        Ok(output)
    }

    pub(crate) async fn wait_for_output(
        &mut self,
        timeout: Duration,
    ) -> anyhow::Result<OutputWait> {
        match tokio::time::timeout(timeout, self.receiver.recv()).await {
            Ok(Some(chunk)) => {
                #[cfg(windows)]
                let chunk = self.filter_terminal_output(chunk)?;

                let mut output = chunk;
                output.push_str(&self.read_available().await?);
                Ok(OutputWait::Chunk(output))
            }
            Ok(None) => Ok(OutputWait::Closed),
            Err(_) => Ok(OutputWait::TimedOut),
        }
    }

    pub(crate) fn output_closed(&self) -> bool {
        self.receiver.is_closed()
    }

    pub async fn has_exited(&mut self) -> anyhow::Result<bool> {
        match &mut self.child {
            SessionChild::Pty(pty) => {
                if let Some(status) = pty.child.try_wait()? {
                    self.exit_code = Some(status.exit_code() as i32);
                    return Ok(true);
                }
            }
            #[cfg(all(windows, feature = "winpty"))]
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
            #[cfg(all(windows, feature = "winpty"))]
            SessionChild::Winpty(pty) => {
                let _ = pty.terminate();
            }
            SessionChild::Pipe(child) => {
                let _ = child.kill();
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
        let normalized_chars = windows::normalize_input(chars, self.tty);
        #[cfg(not(windows))]
        let normalized_chars = chars;

        match &mut self.child {
            SessionChild::Pty(pty) => {
                pty.writer.write_all(normalized_chars.as_bytes())?;
                pty.writer.flush()?;
                Ok(())
            }
            #[cfg(all(windows, feature = "winpty"))]
            SessionChild::Winpty(pty) => pty.write(normalized_chars.as_ref()),
            SessionChild::Pipe(_) => anyhow::bail!(
                "stdin is closed for this session; rerun exec_command with tty=true to keep stdin open"
            ),
        }
    }

    #[cfg(windows)]
    fn filter_terminal_output(&mut self, chunk: String) -> anyhow::Result<String> {
        let Some(result) = self
            .terminal_output_state
            .as_mut()
            .map(|state| state.filter_chunk(&chunk))
        else {
            return Ok(chunk);
        };

        self.write_chars_internal(&result.response)?;
        Ok(result.output)
    }

    #[cfg(windows)]
    fn drain_terminal_output_buffer(&mut self) -> String {
        self.terminal_output_state
            .as_mut()
            .map(windows::TerminalOutputState::drain_pending)
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
mod tests {
    use std::process::Command;
    use std::process::Stdio;
    use std::time::Duration;

    use tokio::sync::mpsc::unbounded_channel;

    use crate::config::PtyMode;
    use crate::exec::output;

    #[cfg(all(windows, not(feature = "winpty")))]
    use super::validate_pty_mode;
    use super::{
        LiveSession, SessionChild, new_live_session, supports_pty_for_mode,
        windows_pty_backend_override_for_mode,
    };

    #[cfg(unix)]
    const TEST_SHELL: &str = "/bin/sh";
    #[cfg(windows)]
    const TEST_SHELL: &str = "cmd.exe";

    #[test]
    fn pty_mode_none_disables_tty_support() {
        assert!(!supports_pty_for_mode(PtyMode::None));
    }

    #[test]
    fn pty_mode_auto_has_no_forced_windows_override() {
        assert_eq!(
            windows_pty_backend_override_for_mode(PtyMode::Auto).unwrap(),
            None
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn forcing_windows_pty_backend_is_rejected_on_non_windows_hosts() {
        assert!(windows_pty_backend_override_for_mode(PtyMode::Conpty).is_err());
        assert!(windows_pty_backend_override_for_mode(PtyMode::Winpty).is_err());
    }

    #[cfg(all(windows, not(feature = "winpty")))]
    #[test]
    fn winpty_mode_is_unavailable_when_the_feature_is_disabled() {
        assert!(!supports_pty_for_mode(PtyMode::Winpty));
        assert!(validate_pty_mode(PtyMode::Winpty).is_err());
    }

    async fn finished_pipe_session(
        receiver: tokio::sync::mpsc::UnboundedReceiver<String>,
    ) -> LiveSession {
        let mut command = Command::new(TEST_SHELL);
        #[cfg(unix)]
        command.args(["-c", "exit 0"]);
        #[cfg(windows)]
        command.args(["/D", "/C", "exit 0"]);
        command.stdin(Stdio::null());
        command.stdout(Stdio::null());
        command.stderr(Stdio::null());

        let child = command.spawn().expect("test child should spawn");
        let mut session = new_live_session(false, SessionChild::Pipe(Box::new(child)), receiver);
        session.exit_code = Some(0);
        session
    }

    #[tokio::test]
    async fn drain_after_exit_waits_for_delayed_pipe_output_until_channel_closes() {
        let (sender, receiver) = unbounded_channel();
        let mut session = finished_pipe_session(receiver).await;

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            sender
                .send("delayed ".to_string())
                .expect("first delayed chunk");
            tokio::time::sleep(Duration::from_millis(50)).await;
            sender
                .send("tail".to_string())
                .expect("second delayed chunk");
        });

        let output = output::drain_after_exit(&mut session)
            .await
            .expect("exit drain should succeed");

        assert_eq!(output, "delayed tail");
    }
}
