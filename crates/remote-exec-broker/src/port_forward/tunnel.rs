use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use remote_exec_proto::port_tunnel::{
    Frame, FrameType, TunnelHeartbeatMeta, decode_frame_meta as decode_port_tunnel_meta,
    encode_frame_meta as encode_port_tunnel_meta, read_frame, write_frame, write_preface,
};
use remote_exec_proto::rpc::RpcErrorCode;
use serde::Serialize;
use serde::de::DeserializeOwned;
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

#[derive(Debug, thiserror::Error)]
#[error("port_forward_backpressure_exceeded: tunnel queue byte budget exceeded")]
struct PortTunnelBackpressureError;

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
        let (reader, mut writer) = tokio::io::split(stream);
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
        let reader_task = tokio::spawn(run_reader_loop(
            reader,
            read_tx,
            reader_tx,
            heartbeat_ack_tx,
            reader_cancel,
        ));
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
        let charge = frame.data_plane_charge();
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
        abort_tunnel_task(&mut *self.reader_task.lock().await);
        abort_tunnel_task(&mut *self.writer_task.lock().await);
        abort_tunnel_task(&mut *self.heartbeat_task.lock().await);
    }

    pub async fn wait_closed(&self, timeout: Duration) -> anyhow::Result<()> {
        let reader_task = self.reader_task.lock().await.take();
        let writer_task = self.writer_task.lock().await.take();
        let heartbeat_task = self.heartbeat_task.lock().await.take();
        tokio::time::timeout(timeout, async move {
            let (reader_result, writer_result, heartbeat_result) = tokio::join!(
                wait_for_tunnel_task("reader", reader_task),
                wait_for_tunnel_task("writer", writer_task),
                wait_for_tunnel_task("heartbeat", heartbeat_task),
            );
            reader_result?;
            writer_result?;
            heartbeat_result?;
            Ok::<(), anyhow::Error>(())
        })
        .await
        .map_err(|_| anyhow::anyhow!("timed out waiting for port tunnel tasks to stop"))??;
        Ok(())
    }
}

async fn wait_for_tunnel_task(
    name: &'static str,
    task: Option<JoinHandle<()>>,
) -> anyhow::Result<()> {
    let Some(task) = task else {
        return Ok(());
    };
    match task.await {
        Ok(()) => Ok(()),
        Err(err) if err.is_cancelled() => Ok(()),
        Err(err) => Err(anyhow::anyhow!(
            "port tunnel {name} task join failed: {err}"
        )),
    }
}

fn abort_tunnel_task(task: &mut Option<JoinHandle<()>>) {
    if let Some(task) = task.as_ref() {
        task.abort();
    }
}

async fn run_reader_loop<R: AsyncRead + Unpin>(
    mut reader: R,
    read_tx: mpsc::Sender<anyhow::Result<Frame>>,
    write_tx: mpsc::Sender<QueuedFrame>,
    heartbeat_ack_tx: watch::Sender<u64>,
    cancel: CancellationToken,
) {
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return,
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
                                match write_tx.try_send(QueuedFrame {
                                    frame: ack,
                                    charge: 0,
                                }) {
                                    Ok(()) => {}
                                    Err(mpsc::error::TrySendError::Full(_)) => {
                                        tracing::debug!(
                                            "port tunnel writer queue is full; dropping heartbeat ack"
                                        );
                                    }
                                    Err(mpsc::error::TrySendError::Closed(_)) => return,
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
                        cancel.cancel();
                        return;
                    }
                    Err(err) => {
                        let _ = read_tx.send(Err(err.into())).await;
                        cancel.cancel();
                        return;
                    }
                }
            }
        }
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
            .downcast_ref::<PortTunnelBackpressureError>()
            .is_some()
    })
}

pub(super) fn is_recoverable_pressure_tunnel_error(meta: &TunnelErrorMeta) -> bool {
    meta.code().and_then(RpcErrorCode::from_wire_value)
        == Some(RpcErrorCode::PortTunnelLimitExceeded)
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
    anyhow::Error::new(PortTunnelBackpressureError)
}

pub(super) fn encode_tunnel_meta<T: Serialize>(meta: &T) -> anyhow::Result<Vec<u8>> {
    encode_port_tunnel_meta(meta).map_err(anyhow::Error::from)
}

pub(super) fn decode_tunnel_meta<T: DeserializeOwned>(frame: &Frame) -> anyhow::Result<T> {
    decode_port_tunnel_meta(frame).map_err(anyhow::Error::from)
}

pub(super) fn tunnel_error(frame: &Frame) -> anyhow::Error {
    format_terminal_tunnel_error(&decode_tunnel_error_frame(frame))
}

pub(super) fn decode_tunnel_error_frame(frame: &Frame) -> TunnelErrorMeta {
    decode_port_tunnel_meta::<remote_exec_proto::port_tunnel::TunnelErrorMeta>(frame)
        .map(|meta| TunnelErrorMeta::decoded(meta, frame.stream_id))
        .unwrap_or_else(|_| TunnelErrorMeta::fallback(frame.stream_id))
}

pub(super) fn format_terminal_tunnel_error(meta: &TunnelErrorMeta) -> anyhow::Error {
    let message = meta.message();
    tracing::debug!(
        code = ?meta.code(),
        generation = ?meta.generation(),
        stream_id = meta.stream_id,
        fatal = meta.fatal(),
        message = %message,
        used_fallback = meta.used_fallback(),
        "port tunnel reported terminal error"
    );
    match meta.code() {
        Some(code) => anyhow::anyhow!("{code}: {message}"),
        None => anyhow::anyhow!("{message}"),
    }
}

pub(super) fn classify_recoverable_tunnel_event(result: anyhow::Result<Frame>) -> ForwardSideEvent {
    match result {
        Ok(frame) if frame.frame_type == FrameType::Error => {
            let meta = decode_tunnel_error_frame(&frame);
            if meta.fatal() {
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
    use remote_exec_proto::port_forward::ForwardId;
    use remote_exec_proto::port_tunnel::{
        Frame, FrameType, TUNNEL_ERROR_CODE_LISTENER_OPEN_FAILED,
        TunnelErrorMeta as ProtoTunnelErrorMeta, TunnelForwardProtocol, TunnelOpenMeta, TunnelRole,
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
                meta: serde_json::to_vec(
                    &remote_exec_proto::port_tunnel::TunnelCloseMeta::operator_close("fwd_test", 1),
                )
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

    #[test]
    fn decode_tunnel_error_frame_uses_proto_defaults_for_partial_meta() {
        let frame = Frame {
            frame_type: FrameType::Error,
            flags: 0,
            stream_id: 7,
            meta: serde_json::to_vec(&serde_json::json!({
                "message": "listen refused",
            }))
            .unwrap(),
            data: Vec::new(),
        };

        let meta = decode_tunnel_error_frame(&frame);
        assert_eq!(meta.code(), None);
        assert_eq!(meta.message(), "listen refused");
        assert!(!meta.fatal());
        assert_eq!(meta.generation(), None);
        assert!(!meta.used_fallback());
    }

    #[test]
    fn decode_tunnel_error_frame_falls_back_for_malformed_meta() {
        let frame = Frame {
            frame_type: FrameType::Error,
            flags: 0,
            stream_id: 7,
            meta: b"{not-json".to_vec(),
            data: Vec::new(),
        };

        let meta = decode_tunnel_error_frame(&frame);
        assert_eq!(meta.code(), None);
        assert_eq!(meta.message(), "port tunnel returned error on stream 7");
        assert!(meta.fatal());
        assert_eq!(meta.generation(), None);
        assert!(meta.used_fallback());
    }

    #[test]
    fn format_terminal_tunnel_error_uses_shared_proto_code() {
        let meta = TunnelErrorMeta::decoded(
            ProtoTunnelErrorMeta::new(
                TUNNEL_ERROR_CODE_LISTENER_OPEN_FAILED,
                "listen refused",
                true,
                Some(3),
            ),
            11,
        );

        assert_eq!(
            format_terminal_tunnel_error(&meta).to_string(),
            format!("{TUNNEL_ERROR_CODE_LISTENER_OPEN_FAILED}: listen refused")
        );
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
                    forward_id: ForwardId::new("fwd_test"),
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

    #[tokio::test]
    async fn heartbeat_frames_do_not_block_reader_when_writer_queue_is_full() {
        let (broker_side, mut daemon_side) = tokio::io::duplex(1);
        let tunnel = PortTunnel::from_stream(broker_side).unwrap();

        for index in 0..128 {
            tunnel
                .send(Frame {
                    frame_type: FrameType::TcpData,
                    flags: 0,
                    stream_id: 1,
                    meta: Vec::new(),
                    data: vec![index as u8; 1024],
                })
                .await
                .unwrap();
        }
        tokio::task::yield_now().await;

        remote_exec_proto::port_tunnel::write_frame(
            &mut daemon_side,
            &Frame {
                frame_type: FrameType::TunnelHeartbeat,
                flags: 0,
                stream_id: 0,
                meta: serde_json::to_vec(&TunnelHeartbeatMeta { nonce: 1 }).unwrap(),
                data: Vec::new(),
            },
        )
        .await
        .unwrap();
        remote_exec_proto::port_tunnel::write_frame(
            &mut daemon_side,
            &Frame {
                frame_type: FrameType::TcpData,
                flags: 0,
                stream_id: 99,
                meta: Vec::new(),
                data: b"payload".to_vec(),
            },
        )
        .await
        .unwrap();

        let frame = tokio::time::timeout(std::time::Duration::from_secs(1), tunnel.recv())
            .await
            .expect("heartbeat echo should not block subsequent data frames")
            .unwrap();
        assert_eq!(frame.frame_type, FrameType::TcpData);
        assert_eq!(frame.stream_id, 99);
        assert_eq!(frame.data, b"payload");
    }

    #[tokio::test]
    async fn abort_stops_blocked_tunnel_tasks_within_one_total_timeout_budget() {
        let (broker_side, mut daemon_side) = tokio::io::duplex(1);
        let tunnel = PortTunnel::from_stream(broker_side).unwrap();

        for index in 0..128 {
            tunnel
                .send(Frame {
                    frame_type: FrameType::TcpData,
                    flags: 0,
                    stream_id: 1,
                    meta: Vec::new(),
                    data: vec![index as u8; 1024],
                })
                .await
                .unwrap();
        }

        let daemon_writer = tokio::spawn(async move {
            for index in 0..128 {
                if remote_exec_proto::port_tunnel::write_frame(
                    &mut daemon_side,
                    &Frame {
                        frame_type: FrameType::TcpData,
                        flags: 0,
                        stream_id: 10 + index,
                        meta: Vec::new(),
                        data: vec![index as u8; 16],
                    },
                )
                .await
                .is_err()
                {
                    return;
                }
            }
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        tunnel.abort().await;

        let result = tokio::time::timeout(
            std::time::Duration::from_millis(110),
            tunnel.wait_closed(std::time::Duration::from_millis(40)),
        )
        .await;
        assert!(
            result.is_ok(),
            "wait_closed should finish within one total timeout budget"
        );
        result
            .unwrap()
            .expect("abort should force blocked tunnel tasks to stop promptly");

        daemon_writer.abort();
        let writer_join = tokio::time::timeout(std::time::Duration::from_secs(1), daemon_writer)
            .await
            .expect("test helper writer should stop promptly");
        if let Err(err) = writer_join {
            assert!(err.is_cancelled(), "daemon writer join failed: {err}");
        }
    }
}
