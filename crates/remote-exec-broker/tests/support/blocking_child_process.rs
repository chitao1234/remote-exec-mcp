use std::io::{self, BufRead, Write};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rmcp::{
    RoleClient,
    service::{RxJsonRpcMessage, TxJsonRpcMessage},
    transport::Transport,
};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

const CHILD_EXIT_TIMEOUT: Duration = Duration::from_secs(3);
const CHILD_EXIT_POLL: Duration = Duration::from_millis(10);

pub struct BlockingChildProcess {
    child: Arc<Mutex<Option<std::process::Child>>>,
    stdin: Arc<Mutex<Option<std::process::ChildStdin>>>,
    receiver: Arc<tokio::sync::Mutex<UnboundedReceiver<RxJsonRpcMessage<RoleClient>>>>,
}

impl BlockingChildProcess {
    pub fn spawn(command: tokio::process::Command) -> io::Result<Self> {
        let mut command = command.into_std();
        command.stdin(Stdio::piped()).stdout(Stdio::piped());

        let mut child = command.spawn()?;
        let stdout = child.stdout.take().ok_or_else(|| {
            io::Error::new(io::ErrorKind::BrokenPipe, "child stdout was not piped")
        })?;
        let stdin = child.stdin.take().ok_or_else(|| {
            io::Error::new(io::ErrorKind::BrokenPipe, "child stdin was not piped")
        })?;
        let (sender, receiver) = unbounded_channel();

        std::thread::Builder::new()
            .name("mcp-child-stdout".to_string())
            .spawn(move || read_child_stdout(stdout, sender))
            .map_err(io::Error::other)?;

        Ok(Self {
            child: Arc::new(Mutex::new(Some(child))),
            stdin: Arc::new(Mutex::new(Some(stdin))),
            receiver: Arc::new(tokio::sync::Mutex::new(receiver)),
        })
    }
}

impl Drop for BlockingChildProcess {
    fn drop(&mut self) {
        if let Ok(mut stdin) = self.stdin.lock() {
            stdin.take();
        }
        if let Ok(mut child) = self.child.lock() {
            if let Some(mut child) = child.take() {
                terminate_child(&mut child);
            }
        }
    }
}

impl Transport<RoleClient> for BlockingChildProcess {
    type Error = io::Error;

    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleClient>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        let stdin = Arc::clone(&self.stdin);
        async move {
            tokio::task::spawn_blocking(move || write_message(&stdin, item))
                .await
                .map_err(join_error_to_io)?
        }
    }

    fn receive(&mut self) -> impl Future<Output = Option<RxJsonRpcMessage<RoleClient>>> + Send {
        let receiver = Arc::clone(&self.receiver);
        async move { receiver.lock().await.recv().await }
    }

    fn close(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        let stdin = Arc::clone(&self.stdin);
        let child = Arc::clone(&self.child);
        async move {
            tokio::task::spawn_blocking(move || {
                if let Ok(mut stdin) = stdin.lock() {
                    stdin.take();
                }
                let mut child = child
                    .lock()
                    .map_err(|_| io::Error::other("child lock poisoned"))?;
                if let Some(mut child) = child.take() {
                    wait_then_terminate_child(&mut child)?;
                }
                Ok(())
            })
            .await
            .map_err(join_error_to_io)?
        }
    }
}

fn read_child_stdout(
    stdout: std::process::ChildStdout,
    sender: tokio::sync::mpsc::UnboundedSender<RxJsonRpcMessage<RoleClient>>,
) {
    let mut reader = std::io::BufReader::new(stdout);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => return,
            Ok(_) => {
                let message = line.trim_end_matches(['\r', '\n']);
                match serde_json::from_str::<RxJsonRpcMessage<RoleClient>>(message) {
                    Ok(message) => {
                        if sender.send(message).is_err() {
                            return;
                        }
                    }
                    Err(err) => {
                        eprintln!("failed to decode child MCP message: {err}; line={message}");
                        return;
                    }
                }
            }
            Err(err) => {
                eprintln!("failed to read child MCP stdout: {err}");
                return;
            }
        }
    }
}

fn write_message(
    stdin: &Arc<Mutex<Option<std::process::ChildStdin>>>,
    item: TxJsonRpcMessage<RoleClient>,
) -> io::Result<()> {
    let mut buffer = Vec::new();
    serde_json::to_writer(&mut buffer, &item)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    buffer.push(b'\n');

    let mut stdin = stdin
        .lock()
        .map_err(|_| io::Error::other("child stdin lock poisoned"))?;
    let stdin = stdin
        .as_mut()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "child stdin is closed"))?;
    stdin.write_all(&buffer)?;
    stdin.flush()
}

fn wait_then_terminate_child(child: &mut std::process::Child) -> io::Result<()> {
    let deadline = Instant::now() + CHILD_EXIT_TIMEOUT;
    loop {
        if child.try_wait()?.is_some() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            terminate_child(child);
            return Ok(());
        }
        std::thread::sleep(CHILD_EXIT_POLL);
    }
}

fn terminate_child(child: &mut std::process::Child) {
    match child.try_wait() {
        Ok(Some(_)) => {}
        Ok(None) => {
            let _ = child.kill();
            let _ = child.wait();
        }
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn join_error_to_io(err: tokio::task::JoinError) -> io::Error {
    io::Error::other(err)
}
