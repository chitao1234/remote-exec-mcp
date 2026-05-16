use std::io::Read;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

use portable_pty::{CommandBuilder, NativePtySystem, PtySystem};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

use crate::config::{ProcessEnvironment, WindowsPtyBackendOverride};

use super::capability::default_pty_size;
#[cfg(not(windows))]
use super::capability::supports_pty;
use super::child::{PtySession, SessionChild};
use super::live::{LiveSession, new_live_session};

const PIPE_OUTPUT_READ_BUFFER_SIZE: usize = 8 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnCommand {
    pub program: String,
    pub argv0: Option<String>,
    pub args: Vec<String>,
}

impl SpawnCommand {
    pub fn from_argv(argv: &[String]) -> anyhow::Result<Self> {
        anyhow::ensure!(!argv.is_empty(), "spawn command argv must not be empty");
        Ok(Self {
            program: argv[0].clone(),
            argv0: None,
            args: argv[1..].to_vec(),
        })
    }

    pub fn argv(&self) -> Vec<String> {
        std::iter::once(self.program.clone())
            .chain(self.args.iter().cloned())
            .collect()
    }
}

pub fn spawn_with_windows_pty_backend_override(
    cmd: &SpawnCommand,
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
    let cmd = SpawnCommand::from_argv(cmd)?;
    spawn_with_windows_pty_backend_override(&cmd, cwd, tty, None, environment)
}

#[cfg(windows)]
pub async fn windows_pty_debug_report(cmd: &SpawnCommand, cwd: &std::path::Path) -> String {
    super::windows::debug_report(cmd, cwd).await
}

pub(super) fn spawn_pty(
    cmd: &SpawnCommand,
    cwd: &std::path::Path,
    environment: &ProcessEnvironment,
) -> anyhow::Result<LiveSession> {
    let pty = NativePtySystem::default().openpty(default_pty_size())?;
    let mut builder = CommandBuilder::new(&cmd.program);
    if let Some(argv0) = &cmd.argv0 {
        builder.arg0(argv0);
    }
    for arg in &cmd.args {
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
    cmd: &SpawnCommand,
    cwd: &std::path::Path,
    environment: &ProcessEnvironment,
) -> anyhow::Result<LiveSession> {
    let (reader, writer) = os_pipe::pipe()?;
    let stderr = writer.try_clone()?;
    let mut command = Command::new(&cmd.program);
    command
        .args(&cmd.args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::from(writer))
        .stderr(Stdio::from(stderr));
    #[cfg(unix)]
    if let Some(argv0) = &cmd.argv0 {
        command.arg0(argv0);
    }
    super::environment::apply_overlay_std_command(&mut command, environment);
    #[cfg(unix)]
    unsafe {
        command.pre_exec(|| {
            let result = nix::libc::setpgid(0, 0);
            if result == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }
    let child = command.spawn()?;
    let (sender, receiver) = unbounded_channel();
    let session = new_live_session(false, SessionChild::Pipe(Box::new(child)), receiver);

    spawn_output_reader(reader, sender);

    Ok(session)
}

fn spawn_output_reader<R>(mut reader: R, sender: UnboundedSender<String>)
where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || {
        let mut buffer = [0u8; PIPE_OUTPUT_READ_BUFFER_SIZE];
        let mut decoder = Utf8PipeDecoder::new();
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => {
                    if let Some(chunk) = decoder.finish() {
                        let _ = sender.send(chunk);
                    }
                    break;
                }
                Ok(read) => {
                    let Some(chunk) = decoder.push(&buffer[..read]) else {
                        continue;
                    };
                    if sender.send(chunk).is_err() {
                        break;
                    }
                }
                Err(_) => {
                    if let Some(chunk) = decoder.finish() {
                        let _ = sender.send(chunk);
                    }
                    break;
                }
            }
        }
    });
}

struct Utf8PipeDecoder {
    pending: Vec<u8>,
}

impl Utf8PipeDecoder {
    fn new() -> Self {
        Self {
            pending: Vec::new(),
        }
    }

    fn push(&mut self, bytes: &[u8]) -> Option<String> {
        self.pending.extend_from_slice(bytes);
        let complete_len = complete_utf8_lossy_prefix_len(&self.pending);
        if complete_len == 0 {
            return None;
        }
        let output = String::from_utf8_lossy(&self.pending[..complete_len]).into_owned();
        self.pending.drain(..complete_len);
        Some(output)
    }

    fn finish(&mut self) -> Option<String> {
        if self.pending.is_empty() {
            return None;
        }
        let output = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending.clear();
        Some(output)
    }
}

fn complete_utf8_lossy_prefix_len(bytes: &[u8]) -> usize {
    let mut offset = 0;
    loop {
        match std::str::from_utf8(&bytes[offset..]) {
            Ok(_) => return bytes.len(),
            Err(err) => {
                let invalid_at = offset + err.valid_up_to();
                match err.error_len() {
                    Some(error_len) => offset = invalid_at + error_len,
                    None => return invalid_at,
                }
            }
        }
    }
}

#[cfg(test)]
mod exec_pipe_decoder_tests {
    use super::Utf8PipeDecoder;

    #[test]
    fn split_multibyte_codepoint_is_emitted_once() {
        let mut decoder = Utf8PipeDecoder::new();

        assert_eq!(decoder.push(&[0xe4, 0xbd]), None);
        assert_eq!(decoder.push(&[0xa0]), Some("你".to_string()));
        assert_eq!(decoder.finish(), None);
    }

    #[test]
    fn invalid_complete_sequence_is_lossy_but_trailing_prefix_is_preserved() {
        let mut decoder = Utf8PipeDecoder::new();

        assert_eq!(
            decoder.push(&[0xff, b'a', 0xf0, 0x9f]),
            Some("\u{fffd}a".to_string())
        );
        assert_eq!(decoder.push(&[0x98, 0x80]), Some("😀".to_string()));
        assert_eq!(decoder.finish(), None);
    }

    #[test]
    fn unfinished_sequence_is_replaced_on_finish() {
        let mut decoder = Utf8PipeDecoder::new();

        assert_eq!(decoder.push(&[b'a', 0xe4, 0xbd]), Some("a".to_string()));
        assert_eq!(decoder.finish(), Some("\u{fffd}".to_string()));
    }
}
