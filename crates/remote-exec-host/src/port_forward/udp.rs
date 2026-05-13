use std::sync::Arc;

use remote_exec_proto::port_forward::normalize_endpoint;
use remote_exec_proto::port_tunnel::{
    EndpointMeta, ForwardDropKind, Frame, FrameType, TunnelForwardProtocol, UdpDatagramMeta,
};
use remote_exec_proto::rpc::RpcErrorCode;
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

use crate::HostRpcError;

use super::codec::{decode_frame_meta, encode_frame_meta};
use super::error::{is_recoverable_pressure_error, rpc_error};
use super::session::{AttachmentState, reactivate_retained_udp_bind};
use super::session::{send_tunnel_error_code_with_sender, send_tunnel_error_with_sender};
use super::tunnel::tunnel_mode;
use super::tunnel::{send_tunnel_error, send_tunnel_error_code};
use super::{
    ConnectionLocalUdpBind, EndpointOkMeta, READ_BUF_SIZE, TunnelMode, TunnelState,
    send_forward_drop_report,
};

enum UdpReadLoopTarget {
    Connection(Arc<TunnelState>),
    AttachedSession(Arc<AttachmentState>),
}

impl UdpReadLoopTarget {
    async fn send_datagram(
        &self,
        stream_id: u32,
        meta: Vec<u8>,
        data: Vec<u8>,
    ) -> Result<(), HostRpcError> {
        let frame = Frame {
            frame_type: FrameType::UdpDatagram,
            flags: 0,
            stream_id,
            meta,
            data,
        };
        match self {
            Self::Connection(tunnel) => tunnel.send(frame).await,
            Self::AttachedSession(attachment) => attachment.tx.send(frame).await,
        }
    }

    async fn send_error_code(&self, stream_id: u32, code: String, message: String) {
        match self {
            Self::Connection(tunnel) => {
                let _ = send_tunnel_error_code(tunnel, stream_id, code, message, false).await;
            }
            Self::AttachedSession(attachment) => {
                let _ = send_tunnel_error_code_with_sender(
                    &attachment.tx,
                    stream_id,
                    code,
                    message,
                    false,
                )
                .await;
            }
        }
    }

    async fn send_read_failed(&self, stream_id: u32, message: String) {
        match self {
            Self::Connection(tunnel) => {
                let _ = send_tunnel_error(
                    tunnel,
                    stream_id,
                    RpcErrorCode::PortReadFailed,
                    message,
                    false,
                )
                .await;
            }
            Self::AttachedSession(attachment) => {
                let _ = send_tunnel_error_with_sender(
                    &attachment.tx,
                    stream_id,
                    RpcErrorCode::PortReadFailed,
                    message,
                    false,
                )
                .await;
            }
        }
    }

    async fn send_forward_drop(
        &self,
        stream_id: u32,
        kind: ForwardDropKind,
        reason: String,
        message: String,
    ) {
        match self {
            Self::Connection(tunnel) => {
                let _ =
                    send_forward_drop_report(&tunnel.tx, stream_id, kind, reason, message).await;
            }
            Self::AttachedSession(attachment) => {
                let _ = send_forward_drop_report(&attachment.tx, stream_id, kind, reason, message)
                    .await;
            }
        }
    }

    async fn close_on_terminal_send_failure(&self, stream_id: u32) {
        match self {
            Self::Connection(tunnel) => {
                if let Some(bind) = tunnel.udp_binds.lock().await.remove(&stream_id) {
                    bind.cancel.cancel();
                }
            }
            Self::AttachedSession(attachment) => {
                if let Some(reader) = attachment.udp_readers.lock().await.remove(&stream_id) {
                    reader.cancel.cancel();
                }
            }
        }
    }
}

pub(super) async fn tunnel_udp_bind(
    tunnel: Arc<TunnelState>,
    frame: Frame,
) -> Result<(), HostRpcError> {
    match tunnel_mode(&tunnel).await {
        TunnelMode::Listen {
            protocol: TunnelForwardProtocol::Udp,
            session,
        } => {
            let meta: EndpointMeta = decode_frame_meta(&frame)?;
            let endpoint = normalize_endpoint(&meta.endpoint)
                .map_err(|err| rpc_error(RpcErrorCode::InvalidEndpoint, err.to_string()))?;
            let socket = Arc::new(
                UdpSocket::bind(&endpoint)
                    .await
                    .map_err(|err| rpc_error(RpcErrorCode::PortBindFailed, err.to_string()))?,
            );
            let bound_endpoint = socket
                .local_addr()
                .map_err(|err| rpc_error(RpcErrorCode::PortBindFailed, err.to_string()))?
                .to_string();
            session
                .replace_udp_bind(
                    frame.stream_id,
                    socket.clone(),
                    &tunnel.state.port_forward_limiter,
                )
                .await?;
            tunnel
                .send(Frame {
                    frame_type: FrameType::UdpBindOk,
                    flags: 0,
                    stream_id: frame.stream_id,
                    meta: encode_frame_meta(&EndpointOkMeta {
                        endpoint: bound_endpoint,
                    })?,
                    data: Vec::new(),
                })
                .await?;
            reactivate_retained_udp_bind(&session).await
        }
        TunnelMode::Listen { .. } => Err(rpc_error(
            RpcErrorCode::InvalidPortTunnel,
            "udp bind requires an open udp listen tunnel",
        )),
        TunnelMode::Connect {
            protocol: TunnelForwardProtocol::Udp,
        } => tunnel_udp_bind_connection_local(tunnel, frame).await,
        TunnelMode::Connect { .. } => Err(rpc_error(
            RpcErrorCode::InvalidPortTunnel,
            "udp bind requires an open udp connect tunnel",
        )),
        TunnelMode::Unopened => Err(rpc_error(
            RpcErrorCode::InvalidPortTunnel,
            "udp bind requires tunnel open",
        )),
    }
}

pub(super) async fn tunnel_udp_bind_connection_local(
    tunnel: Arc<TunnelState>,
    frame: Frame,
) -> Result<(), HostRpcError> {
    let meta: EndpointMeta = decode_frame_meta(&frame)?;
    let endpoint = normalize_endpoint(&meta.endpoint)
        .map_err(|err| rpc_error(RpcErrorCode::InvalidEndpoint, err.to_string()))?;
    let socket = Arc::new(
        UdpSocket::bind(&endpoint)
            .await
            .map_err(|err| rpc_error(RpcErrorCode::PortBindFailed, err.to_string()))?,
    );
    let bound_endpoint = socket
        .local_addr()
        .map_err(|err| rpc_error(RpcErrorCode::PortBindFailed, err.to_string()))?
        .to_string();
    let permit = tunnel.state.port_forward_limiter.try_acquire_udp_bind()?;
    let stream_cancel = tunnel.cancel.child_token();
    tunnel.udp_binds.lock().await.insert(
        frame.stream_id,
        ConnectionLocalUdpBind {
            socket: socket.clone(),
            _permit: permit,
            cancel: stream_cancel.clone(),
        },
    );
    tunnel
        .send(Frame {
            frame_type: FrameType::UdpBindOk,
            flags: 0,
            stream_id: frame.stream_id,
            meta: encode_frame_meta(&EndpointOkMeta {
                endpoint: bound_endpoint,
            })?,
            data: Vec::new(),
        })
        .await?;
    tokio::spawn(tunnel_udp_read_loop_connection_local(
        tunnel,
        frame.stream_id,
        socket,
        stream_cancel,
    ));
    Ok(())
}

pub(super) async fn tunnel_udp_read_loop_connection_local(
    tunnel: Arc<TunnelState>,
    stream_id: u32,
    socket: Arc<UdpSocket>,
    cancel: CancellationToken,
) {
    tunnel_udp_read_loop(
        UdpReadLoopTarget::Connection(tunnel),
        stream_id,
        socket,
        cancel,
    )
    .await;
}

pub(super) async fn tunnel_udp_read_loop_attached_session(
    attachment: Arc<AttachmentState>,
    stream_id: u32,
    socket: Arc<UdpSocket>,
    cancel: CancellationToken,
) {
    tunnel_udp_read_loop(
        UdpReadLoopTarget::AttachedSession(attachment),
        stream_id,
        socket,
        cancel,
    )
    .await;
}

async fn tunnel_udp_read_loop(
    target: UdpReadLoopTarget,
    stream_id: u32,
    socket: Arc<UdpSocket>,
    cancel: CancellationToken,
) {
    let mut buf = vec![0; READ_BUF_SIZE];
    loop {
        let received = tokio::select! {
            _ = cancel.cancelled() => return,
            received = socket.recv_from(&mut buf) => received,
        };
        let (read, peer) = match received {
            Ok(received) => received,
            Err(err) => {
                target.send_read_failed(stream_id, err.to_string()).await;
                return;
            }
        };
        let meta = match encode_frame_meta(&UdpDatagramMeta {
            peer: peer.to_string(),
        }) {
            Ok(meta) => meta,
            Err(err) => {
                target
                    .send_error_code(stream_id, err.code.to_string(), err.message)
                    .await;
                return;
            }
        };
        if let Err(err) = target
            .send_datagram(stream_id, meta, buf[..read].to_vec())
            .await
        {
            if is_recoverable_pressure_error(&err) {
                target
                    .send_forward_drop(
                        stream_id,
                        ForwardDropKind::UdpDatagram,
                        err.code.to_string(),
                        err.message.clone(),
                    )
                    .await;
                tracing::debug!(
                    code = %err.code,
                    message = %err.message,
                    "dropping udp datagram due to local port tunnel pressure"
                );
                continue;
            }
            target
                .send_error_code(stream_id, err.code.to_string(), err.message)
                .await;
            target.close_on_terminal_send_failure(stream_id).await;
            return;
        }
    }
}

pub(super) async fn tunnel_udp_datagram(
    tunnel: &Arc<TunnelState>,
    frame: Frame,
) -> Result<(), HostRpcError> {
    let meta: UdpDatagramMeta = decode_frame_meta(&frame)?;
    let socket = match tunnel_mode(tunnel).await {
        TunnelMode::Listen {
            protocol: TunnelForwardProtocol::Udp,
            session,
        } => session.udp_socket(frame.stream_id).await.ok_or_else(|| {
            rpc_error(
                RpcErrorCode::UnknownPortBind,
                format!("unknown tunnel udp stream `{}`", frame.stream_id),
            )
        })?,
        TunnelMode::Listen { .. } => {
            return Err(rpc_error(
                RpcErrorCode::InvalidPortTunnel,
                "udp datagram requires an open udp tunnel",
            ));
        }
        TunnelMode::Connect {
            protocol: TunnelForwardProtocol::Udp,
        } => tunnel
            .udp_binds
            .lock()
            .await
            .get(&frame.stream_id)
            .map(|bind| bind.socket.clone())
            .ok_or_else(|| {
                rpc_error(
                    RpcErrorCode::UnknownPortBind,
                    format!("unknown tunnel udp stream `{}`", frame.stream_id),
                )
            })?,
        TunnelMode::Connect { .. } => {
            return Err(rpc_error(
                RpcErrorCode::InvalidPortTunnel,
                "udp datagram requires an open udp tunnel",
            ));
        }
        TunnelMode::Unopened => {
            return Err(rpc_error(
                RpcErrorCode::InvalidPortTunnel,
                "udp datagram requires tunnel open",
            ));
        }
    };
    socket
        .send_to(&frame.data, &meta.peer)
        .await
        .map_err(|err| rpc_error(RpcErrorCode::PortWriteFailed, err.to_string()))?;
    Ok(())
}
