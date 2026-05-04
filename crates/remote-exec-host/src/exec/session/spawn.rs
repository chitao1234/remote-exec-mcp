use std::io::Read;
use std::process::{Command, Stdio};

use portable_pty::{CommandBuilder, NativePtySystem, PtySystem};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

use crate::config::{ProcessEnvironment, WindowsPtyBackendOverride};

use super::capability::default_pty_size;
#[cfg(not(windows))]
use super::capability::supports_pty;
use super::child::{PtySession, SessionChild};
use super::live::{LiveSession, new_live_session};

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
            super::windows::spawn_tty_session(cmd, cwd, windows_pty_backend_override, environment)
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
    super::windows::debug_report(cmd, cwd).await
}

pub(super) fn spawn_pty(
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
    super::environment::apply_overlay_builder(&mut builder, environment);

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
    let (reader, writer) = os_pipe::pipe()?;
    let stderr = writer.try_clone()?;
    let mut command = Command::new(&cmd[0]);
    command
        .args(&cmd[1..])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::from(writer))
        .stderr(Stdio::from(stderr));
    super::environment::apply_overlay_std_command(&mut command, environment);
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
