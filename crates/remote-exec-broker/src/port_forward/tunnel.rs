use std::sync::Arc;
use std::time::Duration;

use remote_exec_proto::port_tunnel::{Frame, FrameType, read_frame, write_frame, write_preface};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::daemon_client::DaemonClientError;

use super::events::{ForwardSideEvent, TunnelErrorMeta};

pub struct PortTunnel {
    tx: mpsc::Sender<Frame>,
    rx: Mutex<mpsc::Receiver<anyhow::Result<Frame>>>,
    cancel: CancellationToken,
    reader_task: Mutex<Option<JoinHandle<()>>>,
    writer_task: Mutex<Option<JoinHandle<()>>>,
}

impl PortTunnel {
    pub fn from_stream<S>(stream: S) -> Result<Self, DaemonClientError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (mut reader, mut writer) = tokio::io::split(stream);
        let (tx, mut write_rx) = mpsc::channel::<Frame>(128);
        let (read_tx, read_rx) = mpsc::channel::<anyhow::Result<Frame>>(128);
        let cancel = CancellationToken::new();
        let writer_cancel = cancel.clone();
        let writer_task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    frame = write_rx.recv() => {
                        let Some(frame) = frame else {
                            return;
                        };
                        if let Err(err) = write_frame(&mut writer, &frame).await {
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
        let reader_task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = reader_cancel.cancelled() => return,
                    frame = read_frame(&mut reader) => {
                        match frame {
                            Ok(frame) => {
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
        })
    }

    pub async fn local(
        state: Arc<remote_exec_host::HostRuntimeState>,
    ) -> Result<Self, DaemonClientError> {
        let (mut broker_side, daemon_side) = tokio::io::duplex(256 * 1024);
        tokio::spawn(remote_exec_host::port_forward::serve_tunnel(
            state,
            daemon_side,
        ));
        write_preface(&mut broker_side)
            .await
            .map_err(|err| DaemonClientError::Transport(err.into()))?;
        Self::from_stream(broker_side)
    }

    pub async fn send(&self, frame: Frame) -> anyhow::Result<()> {
        self.tx.send(frame).await.map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "port tunnel writer is closed",
            )
            .into()
        })
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
        Ok(())
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct EndpointMeta {
    pub(super) endpoint: String,
}

#[derive(Debug, Serialize)]
pub(super) struct SessionResumeMeta {
    pub(super) session_id: String,
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
    use remote_exec_proto::port_tunnel::{Frame, FrameType};

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
        let tunnel = SideHandle::local().port_tunnel().await.unwrap();
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
}
