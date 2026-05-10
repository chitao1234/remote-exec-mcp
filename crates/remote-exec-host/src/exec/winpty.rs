use std::ffi::OsString;
use std::path::Path;
use std::process::Command as ProcessCommand;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

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

fn lock_winpty<'a, T>(
    mutex: &'a Mutex<T>,
    name: &'static str,
) -> anyhow::Result<MutexGuard<'a, T>> {
    mutex
        .lock()
        .map_err(|_| anyhow::anyhow!("winpty {name} mutex poisoned"))
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

fn resolve_executable_for_winpty(program: &str) -> OsString {
    let path = Path::new(program);
    if path.is_absolute() || program.contains(['\\', '/']) {
        return OsString::from(program);
    }

    let candidates = executable_candidates(program);
    if let Some(path_env) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_env) {
            for candidate in &candidates {
                let resolved = dir.join(candidate);
                if std::fs::metadata(&resolved)
                    .map(|metadata| metadata.is_file())
                    .unwrap_or(false)
                {
                    return resolved.into_os_string();
                }
            }
        }
    }

    OsString::from(program)
}

fn executable_candidates(program: &str) -> Vec<OsString> {
    let lower = program.to_ascii_lowercase();
    if lower.ends_with(".exe")
        || lower.ends_with(".com")
        || lower.ends_with(".bat")
        || lower.ends_with(".cmd")
    {
        return vec![OsString::from(program)];
    }

    let mut candidates = vec![OsString::from(program)];
    for extension in [".exe", ".com", ".bat", ".cmd"] {
        let mut candidate = OsString::from(program);
        candidate.push(extension);
        candidates.push(candidate);
    }
    candidates
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
    let mut spawn = SpawnConfig::new(resolve_executable_for_winpty(&cmd[0]))
        .cwd(cwd.as_os_str().to_os_string());
    // Winpty forwards this string to CreateProcessW as lpCommandLine. Include argv[0]
    // so programs that rely on CRT-style argv parsing, such as Git Bash, still see
    // switches like `-c` and `-l` in argv[1..] instead of losing them into argv[0].
    if let Some(cmdline) = command_line(cmd) {
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
        let mut exit_deadline = None;

        loop {
            let read_result = match lock_winpty(&reader, "pty reader") {
                Ok(pty) => pty.read_nonblocking(),
                Err(_) => break,
            };
            match read_result {
                Ok(chunk) if !chunk.is_empty() => {
                    exit_deadline = None;
                    if sender.send(chunk).is_err() {
                        break;
                    }
                }
                Ok(_) => {
                    let child_alive = match lock_winpty(&reader_child, "child reader") {
                        Ok(child) => child.is_alive().unwrap_or(false),
                        Err(_) => break,
                    };
                    if child_alive {
                        exit_deadline = None;
                        std::thread::sleep(Duration::from_millis(25));
                        continue;
                    }

                    let deadline = exit_deadline.get_or_insert_with(|| {
                        Instant::now() + super::output::EXIT_OUTPUT_IDLE_GRACE
                    });
                    if Instant::now() >= *deadline {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(10));
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
        lock_winpty(&self.child, "child")?
            .try_wait()
            .map(|status| status.map(|value| value as i32))
            .map_err(map_winpty_error)
    }

    pub(crate) fn write(&self, chars: &str) -> anyhow::Result<()> {
        lock_winpty(&self.pty, "pty")?
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

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::{command_line, quote_windows_argument};

    #[test]
    fn quote_windows_argument_leaves_simple_arguments_unchanged() {
        assert_eq!(quote_windows_argument("plain"), "plain");
        assert_eq!(quote_windows_argument(r#"C:\Tools\bin"#), r#"C:\Tools\bin"#);
    }

    #[test]
    fn quote_windows_argument_quotes_whitespace_and_embedded_quotes() {
        assert_eq!(quote_windows_argument("two words"), r#""two words""#);
        assert_eq!(
            quote_windows_argument(r#"quote "mark""#),
            r#""quote \"mark\"""#
        );
    }

    #[test]
    fn quote_windows_argument_doubles_trailing_backslashes_before_closing_quote() {
        assert_eq!(
            quote_windows_argument(r#"C:\Program Files\Test Folder\"#),
            r#""C:\Program Files\Test Folder\\""#,
        );
    }

    #[test]
    fn command_line_quotes_each_argument_for_winpty_spawn() {
        assert_eq!(
            command_line(&[
                "bash.exe".to_string(),
                "-c".to_string(),
                "printf ok".to_string(),
            ]),
            Some(OsString::from(r#"bash.exe -c "printf ok""#,))
        );
    }

    #[test]
    fn command_line_quotes_whole_argv_for_winpty_spawn() {
        assert_eq!(
            command_line(&[
                "pwsh.exe".to_string(),
                "plain".to_string(),
                "two words".to_string(),
                r#"quote "mark""#.to_string(),
                r#"C:\Program Files\Test Folder\"#.to_string(),
            ]),
            Some(OsString::from(
                r#"pwsh.exe plain "two words" "quote \"mark\"" "C:\Program Files\Test Folder\\""#,
            ))
        );
    }

    #[test]
    fn command_line_returns_none_for_empty_argv() {
        assert_eq!(command_line(&[]), None);
    }
}
