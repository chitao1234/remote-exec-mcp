use std::time::{Duration, Instant};

use tokio::sync::mpsc::UnboundedReceiver;

use super::super::transcript::TranscriptBuffer;
use super::child::{ChildStatus, SessionChild};

const TRANSCRIPT_LIMIT_BYTES: usize = 1024 * 1024;

pub struct LiveSession {
    pub tty: bool,
    pub started_at: Instant,
    pub transcript: TranscriptBuffer,
    pub(crate) child: SessionChild,
    pub(super) receiver: UnboundedReceiver<String>,
    pub(super) exit_code: Option<i32>,
    #[cfg(windows)]
    terminal_output_state: Option<super::windows::TerminalOutputState>,
}

pub(crate) enum OutputWait {
    Chunk(String),
    Closed,
    TimedOut,
}

pub(super) fn new_live_session(
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
        terminal_output_state: tty.then(super::windows::TerminalOutputState::default),
    }
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
        match self.child.try_wait_status()? {
            ChildStatus::Running => Ok(false),
            ChildStatus::Exited(exit_code) => {
                self.exit_code = exit_code;
                Ok(true)
            }
        }
    }

    pub async fn terminate(&mut self) -> anyhow::Result<()> {
        if self.has_exited().await? {
            return Ok(());
        }

        self.child.terminate()
    }

    pub async fn write(&mut self, chars: &str) -> anyhow::Result<()> {
        self.write_chars_internal(chars)
    }

    fn write_chars_internal(&mut self, chars: &str) -> anyhow::Result<()> {
        if chars.is_empty() {
            return Ok(());
        }

        #[cfg(windows)]
        let normalized_chars = super::windows::normalize_input(chars, self.tty);
        #[cfg(not(windows))]
        let normalized_chars = chars;

        match &mut self.child {
            SessionChild::Pty(pty) => {
                use std::io::Write;

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
            .map(super::windows::TerminalOutputState::drain_pending)
            .unwrap_or_default()
    }

    pub fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }

    pub fn record_output(&mut self, chunk: &str) {
        self.transcript.push(chunk.as_bytes());
    }
}
