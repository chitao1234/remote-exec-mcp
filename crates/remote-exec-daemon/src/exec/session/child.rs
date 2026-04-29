use std::io::Write;

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
                let _ = child.kill();
                let _ = child.try_wait()?;
            }
        }

        Ok(())
    }
}
