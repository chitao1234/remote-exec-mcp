use std::ffi::OsString;
use std::path::Path;
use std::process::Command as ProcessCommand;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use winptyrs::{AgentBuilder, AgentFlags, Child, EnvBlock, MouseMode, Pty, PtySize, SpawnConfig};

pub(crate) struct WinptySession {
    child: Arc<Mutex<Child>>,
    pid: u32,
    pty: Arc<Mutex<Pty>>,
}

fn map_winpty_error(err: winptyrs::Error) -> anyhow::Error {
    anyhow::anyhow!(err.to_string())
}

fn winpty_builder() -> AgentBuilder {
    AgentBuilder::new()
        .size(PtySize::new(120, 24).expect("default winpty size is valid"))
        .mouse_mode(MouseMode::None)
        .timeout_ms(10_000)
        .agent_flags(AgentFlags::COLOR_ESCAPES)
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
    winpty_builder()
        .open()
        .map(|_| ())
        .map_err(map_winpty_error)
}

pub(crate) fn spawn_winpty(
    cmd: &[String],
    cwd: &Path,
    environment: EnvBlock,
) -> anyhow::Result<(WinptySession, UnboundedReceiver<String>)> {
    let mut pty = winpty_builder().open().map_err(map_winpty_error)?;
    let mut spawn = SpawnConfig::new(&cmd[0]).cwd(cwd.as_os_str().to_os_string());
    if let Some(cmdline) = command_line(&cmd[1..]) {
        spawn = spawn.cmdline(cmdline);
    }
    let child = pty
        .spawn(spawn.env(environment))
        .map_err(map_winpty_error)?;

    let pid = child.id();
    let pty: Arc<Mutex<Pty>> = Arc::new(Mutex::new(pty));
    let reader: Arc<Mutex<Pty>> = Arc::clone(&pty);
    let child: Arc<Mutex<Child>> = Arc::new(Mutex::new(child));
    let reader_child = Arc::clone(&child);
    let (sender, receiver) = unbounded_channel();

    std::thread::spawn(move || {
        loop {
            let read_result = reader.lock().unwrap().read_nonblocking();
            match read_result {
                Ok(chunk) if !chunk.is_empty() => {
                    if sender.send(chunk).is_err() {
                        break;
                    }
                }
                Ok(_) => {
                    if !reader_child.lock().unwrap().is_alive().unwrap_or(false) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(winptyrs::Error::Eof) => break,
                Err(_) => break,
            }
        }
    });

    Ok((WinptySession { child, pid, pty }, receiver))
}

impl WinptySession {
    pub(crate) fn try_wait(&self) -> anyhow::Result<Option<i32>> {
        self.child
            .lock()
            .unwrap()
            .try_wait()
            .map(|status| status.map(|value| value as i32))
            .map_err(map_winpty_error)
    }

    pub(crate) fn write(&self, chars: &str) -> anyhow::Result<()> {
        self.pty
            .lock()
            .unwrap()
            .write(chars)
            .map(|_| ())
            .map_err(map_winpty_error)
    }

    pub(crate) fn terminate(&self) -> anyhow::Result<()> {
        if self.try_wait()?.is_some() {
            return Ok(());
        }

        let status = ProcessCommand::new("taskkill.exe")
            .args(["/PID", &self.pid.to_string(), "/T", "/F"])
            .status()
            .context("failed to run taskkill for winpty session")?;
        anyhow::ensure!(status.success(), "taskkill failed for winpty session");
        Ok(())
    }
}
