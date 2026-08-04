//! Shared TCP proxy that fronts a real daemon and can drop or corrupt live
//! port-tunnel transports on demand, used by broker integration tests to
//! exercise broker reconnect behavior against real (Rust or C++) daemons.

use std::sync::Arc;
use std::time::Duration;

use remote_exec_proto::port_tunnel::FrameType;
use remote_exec_test_support::port_tunnel_io::{read_frame, write_frame};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, oneshot};
use tokio::task::JoinHandle;

/// Action to apply to a live port-tunnel transport on the next drop signal.
#[derive(Debug, Clone, Copy)]
pub enum ProxyAction {
    Drop,
    Corrupt,
}

/// Behavioral options for [`TunnelDropProxy`].
#[derive(Debug, Clone, Copy)]
pub struct TunnelDropProxyOptions {
    /// Bypass HTTP request sniffing entirely and just copy bytes both ways
    /// (used when the daemon speaks TLS directly to the broker).
    pub raw_stream: bool,
    /// Rewrite plain (non-port-tunnel) requests to `Connection: close` before
    /// forwarding, so proxied HTTP exchanges terminate cleanly.
    pub rewrite_plain_requests: bool,
    /// Prefix used in panic messages from proxy background tasks.
    pub panic_context: &'static str,
}

impl Default for TunnelDropProxyOptions {
    fn default() -> Self {
        Self {
            raw_stream: false,
            rewrite_plain_requests: true,
            panic_context: "tunnel-drop proxy",
        }
    }
}

/// A TCP proxy in front of a daemon that lets tests force-close or corrupt the
/// daemon side of broker port-tunnel transports while leaving the daemon alive.
pub struct TunnelDropProxy {
    pub listen_addr: std::net::SocketAddr,
    pub daemon_addr: Arc<Mutex<std::net::SocketAddr>>,
    pub active_port_tunnels: Arc<Mutex<Vec<oneshot::Sender<ProxyAction>>>>,
    pub background_tasks: Arc<Mutex<Vec<JoinHandle<()>>>>,
    pub shutdown: Option<oneshot::Sender<()>>,
    pub handle: Option<JoinHandle<()>>,
}

impl TunnelDropProxy {
    pub async fn spawn(daemon_addr: std::net::SocketAddr, options: TunnelDropProxyOptions) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind tunnel drop proxy");
        let listen_addr = listener.local_addr().expect("read tunnel drop proxy addr");
        let daemon_addr = Arc::new(Mutex::new(daemon_addr));
        let active_port_tunnels = Arc::new(Mutex::new(Vec::new()));
        let background_tasks = Arc::new(Mutex::new(Vec::new()));
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
        let daemon_addr_task = daemon_addr.clone();
        let active_port_tunnels_task = active_port_tunnels.clone();
        let background_tasks_accept = background_tasks.clone();
        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => {
                        break;
                    }
                    accepted = listener.accept() => {
                        let (stream, _) = match accepted {
                            Ok(value) => value,
                            Err(_) => break,
                        };
                        let daemon_addr = daemon_addr_task.clone();
                        let active_port_tunnels = active_port_tunnels_task.clone();
                        let connection_handle = tokio::spawn(async move {
                            let daemon_addr = *daemon_addr.lock().await;
                            if let Err(err) =
                                proxy_connection(stream, daemon_addr, active_port_tunnels, options).await
                            {
                                if is_expected_proxy_teardown_error(&err) {
                                    return;
                                }
                                panic!("{} connection failed: {err}", options.panic_context);
                            }
                        });
                        background_tasks_accept.lock().await.push(connection_handle);
                    }
                }
            }
        });

        Self {
            listen_addr,
            daemon_addr,
            active_port_tunnels,
            background_tasks,
            shutdown: Some(shutdown_tx),
            handle: Some(handle),
        }
    }

    pub async fn set_daemon_addr(&self, addr: std::net::SocketAddr) {
        *self.daemon_addr.lock().await = addr;
    }

    pub async fn drop_port_tunnels(&self) {
        let mut active = self.active_port_tunnels.lock().await;
        for shutdown in active.drain(..) {
            let _ = shutdown.send(ProxyAction::Drop);
        }
    }

    pub async fn corrupt_port_tunnels(&self) {
        let mut active = self.active_port_tunnels.lock().await;
        for shutdown in active.drain(..) {
            let _ = shutdown.send(ProxyAction::Corrupt);
        }
    }

    pub async fn assert_no_task_panics(&self) {
        assert_no_background_task_panics(&self.background_tasks, "tunnel-drop proxy task panicked")
            .await;
    }

    pub fn stop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
        if let Ok(mut tasks) = self.background_tasks.try_lock() {
            for handle in tasks.drain(..) {
                handle.abort();
            }
        }
    }
}

/// Awaits all finished background task handles, panicking with `context` if
/// any of them panicked, and leaves still-running tasks registered for later.
pub async fn assert_no_background_task_panics(
    tasks: &Mutex<Vec<JoinHandle<()>>>,
    context: &'static str,
) {
    let finished = {
        let mut tasks = tasks.lock().await;
        let mut finished = Vec::new();
        let mut pending = Vec::with_capacity(tasks.len());
        for handle in tasks.drain(..) {
            if handle.is_finished() {
                finished.push(handle);
            } else {
                pending.push(handle);
            }
        }
        *tasks = pending;
        finished
    };

    for handle in finished {
        handle.await.expect(context);
    }
}

async fn proxy_connection(
    mut client_stream: tokio::net::TcpStream,
    daemon_addr: std::net::SocketAddr,
    active_port_tunnels: Arc<Mutex<Vec<oneshot::Sender<ProxyAction>>>>,
    options: TunnelDropProxyOptions,
) -> std::io::Result<()> {
    let mut backend_stream = tokio::net::TcpStream::connect(daemon_addr).await?;
    if options.raw_stream {
        return proxy_plain_streams(client_stream, backend_stream).await;
    }
    let mut request = Vec::new();
    let mut byte = [0u8; 1];

    loop {
        let read = client_stream.read(&mut byte).await?;
        if read == 0 {
            return Ok(());
        }
        request.push(byte[0]);
        if request.ends_with(b"\r\n\r\n") {
            break;
        }
    }

    let request_text = String::from_utf8_lossy(&request);
    let is_port_tunnel = is_port_tunnel_upgrade_request(&request_text);

    if is_port_tunnel {
        backend_stream.write_all(&request).await?;
        let (drop_tx, drop_rx) = oneshot::channel();
        active_port_tunnels.lock().await.push(drop_tx);
        proxy_port_tunnel_streams(client_stream, backend_stream, drop_rx).await
    } else {
        let request = if options.rewrite_plain_requests {
            rewrite_request_connection_close(&request_text)
        } else {
            request
        };
        backend_stream.write_all(&request).await?;
        proxy_plain_streams(client_stream, backend_stream).await
    }
}

fn is_port_tunnel_upgrade_request(request: &str) -> bool {
    let lower = request.to_ascii_lowercase();
    let first_line = lower.lines().next().unwrap_or_default();
    first_line.starts_with("post /v1/port/tunnel ")
        && lower.contains("\r\nconnection: upgrade\r\n")
        && lower.contains("\r\nupgrade: remote-exec-port-tunnel\r\n")
}

fn rewrite_request_connection_close(request: &str) -> Vec<u8> {
    let (headers, _) = request.split_once("\r\n\r\n").unwrap_or((request, ""));
    let mut lines = headers.lines();
    let mut rewritten = String::new();
    if let Some(first_line) = lines.next() {
        rewritten.push_str(first_line);
        rewritten.push_str("\r\n");
    }
    for line in lines {
        if line.to_ascii_lowercase().starts_with("connection:") {
            continue;
        }
        rewritten.push_str(line);
        rewritten.push_str("\r\n");
    }
    rewritten.push_str("Connection: close\r\n\r\n");
    rewritten.into_bytes()
}

async fn proxy_plain_streams(
    mut client_stream: tokio::net::TcpStream,
    mut backend_stream: tokio::net::TcpStream,
) -> std::io::Result<()> {
    let _ = tokio::io::copy_bidirectional(&mut client_stream, &mut backend_stream).await?;
    Ok(())
}

async fn proxy_port_tunnel_streams(
    mut client_stream: tokio::net::TcpStream,
    mut backend_stream: tokio::net::TcpStream,
    mut drop_rx: oneshot::Receiver<ProxyAction>,
) -> std::io::Result<()> {
    tokio::select! {
        result = tokio::io::copy_bidirectional(&mut client_stream, &mut backend_stream) => {
            let _ = result?;
        }
        action = &mut drop_rx => {
            match action {
                Ok(ProxyAction::Drop) | Err(_) => {
                    let _ = client_stream.shutdown().await;
                    let _ = backend_stream.shutdown().await;
                }
                Ok(ProxyAction::Corrupt) => {
                    backend_stream
                        .write_all(&[
                            FrameType::TcpData as u8,
                            0,
                            1,
                            0,
                            0,
                            0,
                            0,
                            0,
                            0,
                            0,
                            0,
                            0,
                            0,
                            0,
                            0,
                            0,
                        ])
                        .await?;
                    if let Ok(Ok(frame)) = tokio::time::timeout(
                        Duration::from_secs(1),
                        read_frame(&mut backend_stream),
                    )
                    .await
                    {
                        write_frame(&mut client_stream, &frame).await?;
                    }
                    let _ = backend_stream.shutdown().await;
                    let _ = client_stream.shutdown().await;
                }
            }
        }
    }

    Ok(())
}

pub fn is_expected_proxy_teardown_error(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::ConnectionRefused
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::NotConnected
            | std::io::ErrorKind::UnexpectedEof
    ) || matches!(err.raw_os_error(), Some(10053 | 10054 | 10061))
}
