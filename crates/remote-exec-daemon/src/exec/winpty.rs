use std::ffi::OsString;
use std::path::Path;
use std::process::Command as ProcessCommand;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use winptyrs::{AgentConfig, MouseMode, PTY, PTYArgs, PTYBackend};

pub(crate) struct WinptySession {
    pty: Arc<Mutex<PTY>>,
}

fn map_winpty_error(err: OsString) -> anyhow::Error {
    anyhow::anyhow!(err.to_string_lossy().into_owned())
}

fn winpty_args() -> PTYArgs {
    PTYArgs {
        cols: 120,
        rows: 24,
        mouse_mode: MouseMode::WINPTY_MOUSE_MODE_NONE,
        timeout: 10_000,
        agent_config: AgentConfig::WINPTY_FLAG_COLOR_ESCAPES,
    }
}

fn quote_windows_argument(arg: &str) -> String {
    if arg.is_empty() {
        return "\"\"".to_string();
    }
    if !arg.chars().any(|ch| matches!(ch, ' ' | '\t' | '"')) {
        return arg.to_string();
    }

    let mut quoted = String::from("\"");
    let mut backslashes = 0;

    for ch in arg.chars() {
        match ch {
            '\\' => backslashes += 1,
            '"' => {
                quoted.push_str(&"\\".repeat(backslashes * 2 + 1));
                quoted.push('"');
                backslashes = 0;
            }
            _ => {
                quoted.push_str(&"\\".repeat(backslashes));
                backslashes = 0;
                quoted.push(ch);
            }
        }
    }

    quoted.push_str(&"\\".repeat(backslashes * 2));
    quoted.push('"');
    quoted
}

fn command_line(args: &[String]) -> Option<OsString> {
    if args.is_empty() {
        return None;
    }

    Some(OsString::from(
        args.iter()
            .map(|arg| quote_windows_argument(arg))
            .collect::<Vec<_>>()
            .join(" "),
    ))
}

pub(crate) fn supports_winpty() -> anyhow::Result<()> {
    PTY::new_with_backend(&winpty_args(), PTYBackend::WinPTY)
        .map(|_| ())
        .map_err(map_winpty_error)
}

pub(crate) fn spawn_winpty(
    cmd: &[String],
    cwd: &Path,
    environment: OsString,
) -> anyhow::Result<(WinptySession, UnboundedReceiver<String>)> {
    let mut pty =
        PTY::new_with_backend(&winpty_args(), PTYBackend::WinPTY).map_err(map_winpty_error)?;
    pty.spawn(
        OsString::from(&cmd[0]),
        command_line(&cmd[1..]),
        Some(cwd.as_os_str().to_os_string()),
        Some(environment),
    )
    .map_err(map_winpty_error)?;

    let pty: Arc<Mutex<PTY>> = Arc::new(Mutex::new(pty));
    let reader: Arc<Mutex<PTY>> = Arc::clone(&pty);
    let (sender, receiver) = unbounded_channel();

    std::thread::spawn(move || {
        loop {
            let read_result: Result<OsString, OsString> = reader.lock().unwrap().read(false);
            match read_result {
                Ok(chunk) if !chunk.is_empty() => {
                    if sender.send(chunk.to_string_lossy().into_owned()).is_err() {
                        break;
                    }
                }
                Ok(_) => {
                    if !reader.lock().unwrap().is_alive().unwrap_or(false) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(_) => break,
            }
        }
    });

    Ok((WinptySession { pty }, receiver))
}

impl WinptySession {
    pub(crate) fn try_wait(&self) -> anyhow::Result<Option<i32>> {
        self.pty
            .lock()
            .unwrap()
            .get_exitstatus()
            .map(|status: Option<u32>| status.map(|value| value as i32))
            .map_err(map_winpty_error)
    }

    pub(crate) fn write(&self, chars: &str) -> anyhow::Result<()> {
        self.pty
            .lock()
            .unwrap()
            .write(OsString::from(chars))
            .map(|_| ())
            .map_err(map_winpty_error)
    }

    pub(crate) fn terminate(&self) -> anyhow::Result<()> {
        if self.try_wait()?.is_some() {
            return Ok(());
        }

        let pid = self.pty.lock().unwrap().get_pid();
        let _ = self.pty.lock().unwrap().cancel_io();
        let status = ProcessCommand::new("taskkill.exe")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .status()
            .context("failed to run taskkill for winpty session")?;
        anyhow::ensure!(status.success(), "taskkill failed for winpty session");
        Ok(())
    }
}
