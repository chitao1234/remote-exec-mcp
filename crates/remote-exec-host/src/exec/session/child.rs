use std::io::Write;
#[cfg(unix)]
use std::time::{Duration, Instant};

pub(super) enum ChildStatus {
    Running,
    Exited(Option<i32>),
}

pub(crate) enum SessionChild {
    Pty(PtySession),
    #[cfg(all(windows, feature = "winpty"))]
    Winpty(super::super::winpty::WinptySession),
    Pipe(Box<std::process::Child>),
}

pub struct PtySession {
    pub child: Box<dyn portable_pty::Child + Send>,
    pub master: Box<dyn portable_pty::MasterPty + Send>,
    pub writer: Box<dyn Write + Send>,
}

#[cfg(unix)]
fn terminate_unix_process_group(child: &mut std::process::Child) -> anyhow::Result<()> {
    use nix::sys::signal::{Signal, killpg};
    use nix::unistd::Pid;

    if child.try_wait()?.is_some() {
        return Ok(());
    }

    let pgid = Pid::from_raw(child.id() as i32);
    let _ = killpg(pgid, Signal::SIGTERM);
    let deadline = Instant::now() + Duration::from_millis(250);
    while Instant::now() < deadline {
        if child.try_wait()?.is_some() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    let _ = killpg(pgid, Signal::SIGKILL);
    let _ = child.wait()?;
    Ok(())
}

impl SessionChild {
    pub(super) fn try_wait_status(&mut self) -> anyhow::Result<ChildStatus> {
        match self {
            SessionChild::Pty(pty) => Ok(match pty.child.try_wait()? {
                Some(status) => ChildStatus::Exited(Some(status.exit_code() as i32)),
                None => ChildStatus::Running,
            }),
            #[cfg(all(windows, feature = "winpty"))]
            SessionChild::Winpty(pty) => Ok(match pty.try_wait()? {
                Some(status) => ChildStatus::Exited(Some(status)),
                None => ChildStatus::Running,
            }),
            SessionChild::Pipe(child) => Ok(match child.try_wait()? {
                Some(status) => ChildStatus::Exited(status.code()),
                None => ChildStatus::Running,
            }),
        }
    }

    pub(super) fn terminate(&mut self) -> anyhow::Result<()> {
        match self {
            SessionChild::Pty(pty) => {
                let _ = pty.child.kill();
                let _ = pty.child.try_wait()?;
            }
            #[cfg(all(windows, feature = "winpty"))]
            SessionChild::Winpty(pty) => {
                let _ = pty.terminate();
            }
            SessionChild::Pipe(child) => {
                #[cfg(unix)]
                {
                    terminate_unix_process_group(child)?;
                }
                #[cfg(not(unix))]
                {
                    let _ = child.kill();
                    let _ = child.try_wait()?;
                }
            }
        }

        Ok(())
    }

    pub(super) fn resize_pty(
        &mut self,
        size: remote_exec_proto::rpc::ExecPtySize,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            size.rows > 0 && size.cols > 0,
            "PTY rows and cols must be greater than zero"
        );
        match self {
            SessionChild::Pty(pty) => pty
                .master
                .resize(portable_pty::PtySize {
                    rows: size.rows,
                    cols: size.cols,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(Into::into),
            #[cfg(all(windows, feature = "winpty"))]
            SessionChild::Winpty(_) => {
                anyhow::bail!("PTY resize is not supported by the winpty backend")
            }
            SessionChild::Pipe(_) => anyhow::bail!("PTY resize requires a tty session"),
        }
    }
}
