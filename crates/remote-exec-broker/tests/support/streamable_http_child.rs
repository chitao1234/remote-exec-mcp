use std::net::SocketAddr;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cfg(any(target_os = "solaris", target_os = "illumos"))]
use std::io::Read;
#[cfg(not(any(target_os = "solaris", target_os = "illumos")))]
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::oneshot;

const STREAMABLE_HTTP_LOG_FILTER: &str = "warn,remote_exec_broker=info";
const STREAMABLE_HTTP_BOUND_ADDR_TIMEOUT: Duration = Duration::from_secs(5);

pub fn configure_streamable_http_broker_child(command: &mut tokio::process::Command) {
    command.env("REMOTE_EXEC_LOG", STREAMABLE_HTTP_LOG_FILTER);
    command.stderr(Stdio::piped());
}

pub async fn wait_for_streamable_http_bound_addr(
    child: &mut tokio::process::Child,
    resource: &str,
) -> SocketAddr {
    let stderr = child
        .stderr
        .take()
        .expect("streamable HTTP broker child should have piped stderr");
    let captured = Arc::new(Mutex::new(Vec::new()));
    let captured_for_task = Arc::clone(&captured);
    let (addr_tx, addr_rx) = oneshot::channel();

    spawn_stderr_reader(stderr, captured_for_task, addr_tx);

    let captured_lines = || captured.lock().unwrap().join("\n");

    match tokio::time::timeout(STREAMABLE_HTTP_BOUND_ADDR_TIMEOUT, addr_rx).await {
        Ok(Ok(addr)) => addr,
        Ok(Err(_)) => panic!(
            "{resource} exited before logging its bound streamable HTTP address; recent stderr:\n{}",
            captured_lines()
        ),
        Err(_) => panic!(
            "{resource} did not log its bound streamable HTTP address within {STREAMABLE_HTTP_BOUND_ADDR_TIMEOUT:?}; recent stderr:\n{}",
            captured_lines()
        ),
    }
}

#[cfg(not(any(target_os = "solaris", target_os = "illumos")))]
fn spawn_stderr_reader(
    stderr: tokio::process::ChildStderr,
    captured: Arc<Mutex<Vec<String>>>,
    addr_tx: oneshot::Sender<SocketAddr>,
) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        let mut addr_tx = Some(addr_tx);
        while let Some(line) = lines.next_line().await.unwrap() {
            handle_stderr_line(&captured, &mut addr_tx, line);
        }
    });
}

#[cfg(any(target_os = "solaris", target_os = "illumos"))]
fn spawn_stderr_reader(
    stderr: tokio::process::ChildStderr,
    captured: Arc<Mutex<Vec<String>>>,
    addr_tx: oneshot::Sender<SocketAddr>,
) {
    let stderr = std::fs::File::from(stderr.into_owned_fd().unwrap());
    std::thread::Builder::new()
        .name("mcp-http-child-stderr".to_string())
        .spawn(move || read_stderr_without_readiness(stderr, captured, addr_tx))
        .expect("spawn streamable HTTP broker stderr reader");
}

#[cfg(any(target_os = "solaris", target_os = "illumos"))]
fn read_stderr_without_readiness(
    mut stderr: std::fs::File,
    captured: Arc<Mutex<Vec<String>>>,
    addr_tx: oneshot::Sender<SocketAddr>,
) {
    let mut addr_tx = Some(addr_tx);
    let mut pending = Vec::new();
    let mut buffer = [0u8; 4096];

    loop {
        match stderr.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                pending.extend_from_slice(&buffer[..n]);
                while let Some(newline) = pending.iter().position(|byte| *byte == b'\n') {
                    let mut line = pending.drain(..=newline).collect::<Vec<_>>();
                    if line.last() == Some(&b'\n') {
                        line.pop();
                    }
                    if line.last() == Some(&b'\r') {
                        line.pop();
                    }
                    handle_stderr_line(
                        &captured,
                        &mut addr_tx,
                        String::from_utf8_lossy(&line).into_owned(),
                    );
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => break,
        }
    }

    if !pending.is_empty() {
        handle_stderr_line(
            &captured,
            &mut addr_tx,
            String::from_utf8_lossy(&pending).into_owned(),
        );
    }
}

fn handle_stderr_line(
    captured: &Arc<Mutex<Vec<String>>>,
    addr_tx: &mut Option<oneshot::Sender<SocketAddr>>,
    line: String,
) {
    let mut captured = captured.lock().unwrap();
    if captured.len() >= 32 {
        captured.remove(0);
    }
    captured.push(line.clone());
    drop(captured);

    if let Some(addr) = parse_bound_addr_from_log_line(&line) {
        if let Some(addr_tx) = addr_tx.take() {
            let _ = addr_tx.send(addr);
        }
    }
}

fn parse_bound_addr_from_log_line(line: &str) -> Option<SocketAddr> {
    if !line.contains("starting broker MCP streamable HTTP service") {
        return None;
    }
    let start = line.find("listen=")? + "listen=".len();
    let raw = line[start..].split_whitespace().next()?;
    raw.trim_matches('"').parse().ok()
}

#[cfg(test)]
mod tests {
    use super::parse_bound_addr_from_log_line;

    #[test]
    fn parses_compact_tracing_listen_field() {
        let line = "2026-05-13T12:34:56.000000Z  INFO remote_exec_broker::mcp_server: starting broker MCP streamable HTTP service listen=127.0.0.1:43123 path=/mcp stateful=false";
        assert_eq!(
            parse_bound_addr_from_log_line(line).unwrap().to_string(),
            "127.0.0.1:43123"
        );
    }

    #[test]
    fn ignores_non_startup_lines() {
        let line = "2026-05-13T12:34:56.000000Z  INFO remote_exec_broker::startup: starting broker";
        assert!(parse_bound_addr_from_log_line(line).is_none());
    }
}
