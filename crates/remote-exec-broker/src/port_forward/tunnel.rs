use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use remote_exec_proto::port_tunnel::{
    Frame, FrameType, HEADER_LEN, TunnelHeartbeatMeta, read_frame, write_frame, write_preface,
};
use remote_exec_proto::rpc::RpcErrorCode;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{Mutex, mpsc, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::daemon_client::DaemonClientError;

use super::events::{ForwardSideEvent, TunnelErrorMeta};
use super::timings::timings;

pub struct PortTunnel {
    tx: mpsc::Sender<QueuedFrame>,
    rx: Mutex<mpsc::Receiver<anyhow::Result<Frame>>>,
    cancel: CancellationToken,
    reader_task: Mutex<Option<JoinHandle<()>>>,
    writer_task: Mutex<Option<JoinHandle<()>>>,
    heartbeat_task: Mutex<Option<JoinHandle<()>>>,
    queued_bytes: Arc<AtomicUsize>,
    max_queued_bytes: usize,
}

struct QueuedFrame {
    frame: Frame,
    charge: usize,
}

impl PortTunnel {
    pub const DEFAULT_MAX_QUEUED_BYTES: usize =
        remote_exec_proto::port_forward::DEFAULT_TUNNEL_QUEUE_BYTES as usize;

    pub fn from_stream<S>(stream: S) -> Result<Self, DaemonClientError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        Self::from_stream_with_max_queued_bytes(stream, Self::DEFAULT_MAX_QUEUED_BYTES)
    }

    pub fn from_stream_with_max_queued_bytes<S>(
        stream: S,
        max_queued_bytes: usize,
    ) -> Result<Self, DaemonClientError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (mut reader, mut writer) = tokio::io::split(stream);
        let (tx, mut write_rx) = mpsc::channel::<QueuedFrame>(128);
        let (read_tx, read_rx) = mpsc::channel::<anyhow::Result<Frame>>(128);
        let (heartbeat_ack_tx, heartbeat_ack_rx) = watch::channel(0u64);
        let cancel = CancellationToken::new();
        let writer_cancel = cancel.clone();
        let queued_bytes = Arc::new(AtomicUsize::new(0));
        let writer_queued_bytes = queued_bytes.clone();
        let heartbeat_tx = tx.clone();
        let heartbeat_read_tx = read_tx.clone();
        let heartbeat_cancel = cancel.clone();
        let heartbeat_task = tokio::spawn(run_heartbeat_loop(
            heartbeat_tx,
            heartbeat_read_tx,
            heartbeat_ack_rx,
            heartbeat_cancel,
        ));
        let writer_task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    queued = write_rx.recv() => {
                        let Some(queued) = queued else {
                            return;
                        };
                        let QueuedFrame { frame, charge } = queued;
                        let result = write_frame(&mut writer, &frame).await;
                        release_queued_bytes(&writer_queued_bytes, charge);
                        if let Err(err) = result {
                            tracing::debug!(error = %err, "port tunnel writer stopped");
                            writer_cancel.cancel();
                            return;
                        }
                    }
                    _ = writer_cancel.cancelled() => return,
                }
            }
        });
        let reader_cancel = cancel.clone();
        let reader_tx = tx.clone();
        let reader_task = tokio::spawn(async move {
            let heartbeat_ack_tx = heartbeat_ack_tx;
            loop {
                tokio::select! {
                    _ = reader_cancel.cancelled() => return,
                    frame = read_frame(&mut reader) => {
                        match frame {
                            Ok(frame) => {
                                match frame.frame_type {
                                    FrameType::TunnelHeartbeatAck => {
                                        if let Ok(meta) =
                                            serde_json::from_slice::<TunnelHeartbeatMeta>(&frame.meta)
                                        {
                                            let _ = heartbeat_ack_tx.send(meta.nonce);
                                        }
                                        continue;
                                    }
                                    FrameType::TunnelHeartbeat => {
                                        let ack = Frame {
                                            frame_type: FrameType::TunnelHeartbeatAck,
                                            flags: 0,
                                            stream_id: 0,
                                            meta: frame.meta,
                                            data: Vec::new(),
                                        };
                                        if reader_tx.send(QueuedFrame { frame: ack, charge: 0 }).await.is_err() {
                                            return;
                                        }
                                        continue;
                                    }
                                    _ => {}
                                }
                                if read_tx.send(Ok(frame)).await.is_err() {
                                    return;
                                }
                            }
                            Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => {
                                let _ = read_tx
                                    .send(Err(std::io::Error::new(
                                        std::io::ErrorKind::UnexpectedEof,
                                        "port tunnel closed",
                                    )
                                    .into()))
                                    .await;
                                reader_cancel.cancel();
                                return;
                            }
                            Err(err) => {
                                let _ = read_tx.send(Err(err.into())).await;
                                reader_cancel.cancel();
                                return;
                            }
                        }
                    }
                }
            }
        });
        Ok(Self {
            tx,
            rx: Mutex::new(read_rx),
            cancel,
            reader_task: Mutex::new(Some(reader_task)),
            writer_task: Mutex::new(Some(writer_task)),
            heartbeat_task: Mutex::new(Some(heartbeat_task)),
            queued_bytes,
            max_queued_bytes,
        })
    }

    pub async fn local(
        state: Arc<remote_exec_host::HostRuntimeState>,
        max_queued_bytes: usize,
    ) -> Result<Self, DaemonClientError> {
        let (mut broker_side, daemon_side) = tokio::io::duplex(256 * 1024);
        tokio::spawn(remote_exec_host::port_forward::serve_tunnel(
            state,
            daemon_side,
        ));
        write_preface(&mut broker_side)
            .await
            .map_err(|err| DaemonClientError::Transport(err.into()))?;
        Self::from_stream_with_max_queued_bytes(broker_side, max_queued_bytes)
    }

    pub async fn send(&self, frame: Frame) -> anyhow::Result<()> {
        let charge = data_frame_charge(&frame);
        if charge > self.max_queued_bytes {
            return Err(backpressure_error());
        }
        if charge > 0 {
            reserve_queued_bytes(&self.queued_bytes, charge, self.max_queued_bytes)?;
        }
        let queued = QueuedFrame { frame, charge };
        if self.tx.send(queued).await.is_err() {
            release_queued_bytes(&self.queued_bytes, charge);
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "port tunnel writer is closed",
            )
            .into());
        }
        Ok(())
    }

    pub async fn recv(&self) -> anyhow::Result<Frame> {
        self.rx.lock().await.recv().await.ok_or_else(|| {
            anyhow::Error::from(std::io::Error::new(
                std::io::ErrorKind::NotConnected,
                "port tunnel reader is closed",
            ))
        })?
    }

    pub async fn close_stream(&self, stream_id: u32) -> anyhow::Result<()> {
        self.send(Frame {
            frame_type: FrameType::Close,
            flags: 0,
            stream_id,
            meta: Vec::new(),
            data: Vec::new(),
        })
        .await
    }

    pub async fn abort(&self) {
        self.cancel.cancel();
    }

    pub async fn wait_closed(&self, timeout: Duration) -> anyhow::Result<()> {
        if let Some(task) = self.reader_task.lock().await.take() {
            tokio::time::timeout(timeout, task)
                .await
                .map_err(|_| anyhow::anyhow!("timed out waiting for port tunnel reader task"))?
                .map_err(|err| anyhow::anyhow!("port tunnel reader task join failed: {err}"))?;
        }
        if let Some(task) = self.writer_task.lock().await.take() {
            tokio::time::timeout(timeout, task)
                .await
                .map_err(|_| anyhow::anyhow!("timed out waiting for port tunnel writer task"))?
                .map_err(|err| anyhow::anyhow!("port tunnel writer task join failed: {err}"))?;
        }
        if let Some(task) = self.heartbeat_task.lock().await.take() {
            tokio::time::timeout(timeout, task)
                .await
                .map_err(|_| anyhow::anyhow!("timed out waiting for port tunnel heartbeat task"))?
                .map_err(|err| anyhow::anyhow!("port tunnel heartbeat task join failed: {err}"))?;
        }
        Ok(())
    }
}

async fn run_heartbeat_loop(
    tx: mpsc::Sender<QueuedFrame>,
    read_tx: mpsc::Sender<anyhow::Result<Frame>>,
    mut ack_rx: watch::Receiver<u64>,
    cancel: CancellationToken,
) {
    let timing = timings();
    let heartbeat_interval = timing.heartbeat_interval;
    let heartbeat_timeout = timing.heartbeat_timeout;
    let mut next_nonce = 1u64;
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tokio::time::sleep(heartbeat_interval) => {}
        }

        let current_nonce = next_nonce;
        next_nonce = next_nonce.saturating_add(1);
        let frame = Frame {
            frame_type: FrameType::TunnelHeartbeat,
            flags: 0,
            stream_id: 0,
            meta: match serde_json::to_vec(&TunnelHeartbeatMeta {
                nonce: current_nonce,
            }) {
                Ok(meta) => meta,
                Err(err) => {
                    let _ = read_tx.send(Err(err.into())).await;
                    cancel.cancel();
                    return;
                }
            },
            data: Vec::new(),
        };
        let send_result =
            tokio::time::timeout(heartbeat_timeout, tx.send(QueuedFrame { frame, charge: 0 }))
                .await;
        if !matches!(send_result, Ok(Ok(()))) {
            let _ = read_tx
                .send(Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "port tunnel heartbeat could not be queued",
                )
                .into()))
                .await;
            cancel.cancel();
            return;
        }

        let acknowledged = tokio::time::timeout(heartbeat_timeout, async {
            loop {
                if *ack_rx.borrow_and_update() == current_nonce {
                    return true;
                }
                if ack_rx.changed().await.is_err() {
                    return false;
                }
            }
        })
        .await
        .unwrap_or(false);

        if !acknowledged {
            let _ = read_tx
                .send(Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "port tunnel heartbeat timed out",
                )
                .into()))
                .await;
            cancel.cancel();
            return;
        }
    }
}

pub(super) fn is_backpressure_error(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .to_string()
            .contains("port_forward_backpressure_exceeded")
    })
}

pub(super) fn is_recoverable_pressure_tunnel_error(meta: &TunnelErrorMeta) -> bool {
    meta.code.as_deref().and_then(RpcErrorCode::from_wire_value)
        == Some(RpcErrorCode::PortTunnelLimitExceeded)
}

fn data_frame_charge(frame: &Frame) -> usize {
    if frame.stream_id == 0 {
        0
    } else {
        HEADER_LEN
            .saturating_add(frame.meta.len())
            .saturating_add(frame.data.len())
    }
}

fn reserve_queued_bytes(
    queued_bytes: &AtomicUsize,
    charge: usize,
    max_queued_bytes: usize,
) -> anyhow::Result<()> {
    let mut current = queued_bytes.load(Ordering::Relaxed);
    loop {
        let Some(next) = current.checked_add(charge) else {
            return Err(backpressure_error());
        };
        if next > max_queued_bytes {
            return Err(backpressure_error());
        }
        match queued_bytes.compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Relaxed)
        {
            Ok(_) => return Ok(()),
            Err(observed) => current = observed,
        }
    }
}

fn release_queued_bytes(queued_bytes: &AtomicUsize, charge: usize) {
    if charge > 0 {
        queued_bytes.fetch_sub(charge, Ordering::AcqRel);
    }
}

fn backpressure_error() -> anyhow::Error {
    anyhow::anyhow!("port_forward_backpressure_exceeded: tunnel queue byte budget exceeded")
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct EndpointMeta {
    pub(super) endpoint: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct TcpAcceptMeta {
    pub(super) listener_stream_id: u32,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct UdpDatagramMeta {
    pub(super) peer: String,
}

pub(super) fn encode_tunnel_meta<T: Serialize>(meta: &T) -> anyhow::Result<Vec<u8>> {
    serde_json::to_vec(meta).map_err(anyhow::Error::from)
}

pub(super) fn decode_tunnel_meta<T: for<'de> Deserialize<'de>>(frame: &Frame) -> anyhow::Result<T> {
    serde_json::from_slice(&frame.meta).map_err(anyhow::Error::from)
}

pub(super) fn tunnel_error(frame: &Frame) -> anyhow::Error {
    format_terminal_tunnel_error(&decode_tunnel_error_frame(frame))
}

pub(super) fn decode_tunnel_error_frame(frame: &Frame) -> TunnelErrorMeta {
    let fallback = || TunnelErrorMeta {
        code: None,
        message: format!("port tunnel returned error on stream {}", frame.stream_id),
        fatal: true,
        stream_id: frame.stream_id,
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&frame.meta) else {
        return fallback();
    };
    TunnelErrorMeta {
        code: value
            .get("code")
            .and_then(|code| code.as_str())
            .map(ToOwned::to_owned),
        message: value
            .get("message")
            .and_then(|message| message.as_str())
            .unwrap_or("port tunnel error")
            .to_string(),
        fatal: value
            .get("fatal")
            .and_then(|fatal| fatal.as_bool())
            .unwrap_or(false),
        stream_id: frame.stream_id,
    }
}

pub(super) fn format_terminal_tunnel_error(meta: &TunnelErrorMeta) -> anyhow::Error {
    let _ = meta.fatal;
    match meta.code.as_deref() {
        Some(code) => anyhow::anyhow!("{code}: {}", meta.message),
        None if meta.message
            == format!("port tunnel returned error on stream {}", meta.stream_id) =>
        {
            anyhow::anyhow!("{}", meta.message)
        }
        None => anyhow::anyhow!("{}", meta.message),
    }
}

pub(super) fn classify_recoverable_tunnel_event(result: anyhow::Result<Frame>) -> ForwardSideEvent {
    match result {
        Ok(frame) if frame.frame_type == FrameType::Error => {
            let meta = decode_tunnel_error_frame(&frame);
            if meta.fatal {
                ForwardSideEvent::TerminalTunnelError(meta)
            } else {
                ForwardSideEvent::Frame(frame)
            }
        }
        Ok(frame) => ForwardSideEvent::Frame(frame),
        Err(err) if is_retryable_transport_error(&err) => ForwardSideEvent::RetryableTransportLoss,
        Err(err) => ForwardSideEvent::TerminalTransportError(err),
    }
}

pub(super) fn is_retryable_transport_error(err: &anyhow::Error) -> bool {
    for cause in err.chain() {
        if let Some(daemon_error) = cause.downcast_ref::<DaemonClientError>() {
            if daemon_error.is_transport() {
                return true;
            }
            if matches!(
                daemon_error.rpc_error_code(),
                Some(RpcErrorCode::InvalidPortTunnel | RpcErrorCode::PortTunnelUnavailable)
            ) {
                return true;
            }
        }
        if let Some(io_error) = cause.downcast_ref::<std::io::Error>() {
            if matches!(
                io_error.kind(),
                std::io::ErrorKind::UnexpectedEof
                    | std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::NotConnected
                    | std::io::ErrorKind::TimedOut
            ) {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use remote_exec_proto::port_tunnel::{
        Frame, FrameType, TunnelForwardProtocol, TunnelOpenMeta, TunnelRole,
    };

    use super::super::side::SideHandle;
    use super::*;

    #[tokio::test]
    async fn port_tunnel_close_stops_reader_and_writer_tasks() {
        let (broker_side, mut daemon_side) = tokio::io::duplex(4096);
        let tunnel = PortTunnel::from_stream(broker_side).unwrap();
        tunnel
            .send(Frame {
                frame_type: FrameType::TunnelClose,
                flags: 0,
                stream_id: 0,
                meta: serde_json::to_vec(&remote_exec_proto::port_tunnel::TunnelCloseMeta {
                    forward_id: "fwd_test".to_string(),
                    generation: 1,
                    reason: "operator_close".to_string(),
                })
                .unwrap(),
                data: Vec::new(),
            })
            .await
            .unwrap();

        let close = remote_exec_proto::port_tunnel::read_frame(&mut daemon_side)
            .await
            .unwrap();
        assert_eq!(close.frame_type, FrameType::TunnelClose);
        drop(daemon_side);
        tunnel.abort().await;
        tunnel
            .wait_closed(std::time::Duration::from_secs(1))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn local_port_tunnel_binds_tcp_listener() {
        let tunnel = SideHandle::local()
            .unwrap()
            .port_tunnel(PortTunnel::DEFAULT_MAX_QUEUED_BYTES)
            .await
            .unwrap();
        tunnel
            .send(Frame {
                frame_type: FrameType::TunnelOpen,
                flags: 0,
                stream_id: 0,
                meta: serde_json::to_vec(&TunnelOpenMeta {
                    forward_id: "fwd_test".to_string(),
                    role: TunnelRole::Listen,
                    side: "local".to_string(),
                    generation: 1,
                    protocol: TunnelForwardProtocol::Tcp,
                    resume_session_id: None,
                })
                .unwrap(),
                data: Vec::new(),
            })
            .await
            .unwrap();
        assert_eq!(
            tunnel.recv().await.unwrap().frame_type,
            FrameType::TunnelReady
        );
        tunnel
            .send(Frame {
                frame_type: FrameType::TcpListen,
                flags: 0,
                stream_id: 1,
                meta: serde_json::to_vec(&serde_json::json!({
                    "endpoint": "127.0.0.1:0"
                }))
                .unwrap(),
                data: Vec::new(),
            })
            .await
            .unwrap();

        let frame = tunnel.recv().await.unwrap();

        assert_eq!(frame.frame_type, FrameType::TcpListenOk);
    }

    #[tokio::test]
    async fn heartbeat_ack_frames_are_consumed_by_port_tunnel() {
        let (broker_side, mut daemon_side) = tokio::io::duplex(4096);
        let tunnel = PortTunnel::from_stream(broker_side).unwrap();
        let heartbeat_meta = serde_json::to_vec(&serde_json::json!({ "nonce": 1 })).unwrap();
        remote_exec_proto::port_tunnel::write_frame(
            &mut daemon_side,
            &Frame {
                frame_type: FrameType::TunnelHeartbeatAck,
                flags: 0,
                stream_id: 0,
                meta: heartbeat_meta,
                data: Vec::new(),
            },
        )
        .await
        .unwrap();
        remote_exec_proto::port_tunnel::write_frame(
            &mut daemon_side,
            &Frame {
                frame_type: FrameType::TunnelReady,
                flags: 0,
                stream_id: 0,
                meta: serde_json::to_vec(&remote_exec_proto::port_tunnel::TunnelReadyMeta {
                    generation: 1,
                    session_id: None,
                    resume_timeout_ms: None,
                    limits: remote_exec_proto::port_tunnel::TunnelLimitSummary::default(),
                })
                .unwrap(),
                data: Vec::new(),
            },
        )
        .await
        .unwrap();

        let frame = tokio::time::timeout(std::time::Duration::from_secs(1), tunnel.recv())
            .await
            .expect("heartbeat ack should not block data-plane frame")
            .unwrap();
        assert_eq!(frame.frame_type, FrameType::TunnelReady);
    }

    #[tokio::test]
    async fn heartbeat_timeout_surfaces_retryable_transport_error() {
        let (broker_side, mut daemon_side) = tokio::io::duplex(4096);
        let tunnel = PortTunnel::from_stream(broker_side).unwrap();

        let heartbeat = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                let frame = remote_exec_proto::port_tunnel::read_frame(&mut daemon_side)
                    .await
                    .unwrap();
                if frame.frame_type == FrameType::TunnelHeartbeat {
                    return frame;
                }
            }
        })
        .await
        .expect("broker should send heartbeat");
        assert_eq!(heartbeat.stream_id, 0);

        let err = tokio::time::timeout(std::time::Duration::from_secs(1), tunnel.recv())
            .await
            .expect("heartbeat timeout should wake tunnel receiver")
            .unwrap_err();
        assert!(is_retryable_transport_error(&err), "{err:#}");
    }
}
