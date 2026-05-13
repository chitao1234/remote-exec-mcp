use std::sync::Arc;

use remote_exec_proto::port_forward::normalize_endpoint;
use remote_exec_proto::port_tunnel::{
    EndpointMeta, ForwardDropKind, Frame, FrameType, TunnelForwardProtocol, UdpDatagramMeta,
};
use remote_exec_proto::rpc::RpcErrorCode;
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

use crate::HostRpcError;

use super::active::{
    ActiveProtocolAccess, ConnectContext, ListenContext, active_access, send_tunnel_error,
    send_tunnel_error_code,
};
use super::codec::decode_frame_meta;
use super::error::{is_recoverable_pressure_error, rpc_error};
use super::frames::{endpoint_ok_frame, frame as raw_frame, meta_frame};
use super::session::{AttachmentState, SessionState, reactivate_retained_udp_bind};
use super::{ConnectionLocalUdpBind, READ_BUF_SIZE, TunnelState, send_forward_drop_report};

enum UdpReadLoopTarget {
    Connect(ConnectContext),
    Listen(ListenContext),
}

impl UdpReadLoopTarget {
    async fn send_datagram(
        &self,
        stream_id: u32,
        meta: Vec<u8>,
        data: Vec<u8>,
    ) -> Result<(), HostRpcError> {
        let frame = raw_frame(FrameType::UdpDatagram, stream_id, meta, data);
        match self {
            Self::Connect(context) => context.tx().send(frame).await,
            Self::Listen(context) => context.tx().send(frame).await,
        }
    }

    async fn send_error_code(&self, stream_id: u32, code: String, message: String) {
        match self {
            Self::Connect(context) => {
                let _ = send_tunnel_error_code(
                    context.tx(),
                    Some(context.generation()),
                    stream_id,
                    code,
                    message,
                    false,
                )
                .await;
            }
            Self::Listen(context) => {
                let _ = send_tunnel_error_code(
                    context.tx(),
                    Some(context.generation()),
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
            Self::Connect(context) => {
                let _ = send_tunnel_error(
                    context.tx(),
                    Some(context.generation()),
                    stream_id,
                    RpcErrorCode::PortReadFailed,
                    message,
                    false,
                )
                .await;
            }
            Self::Listen(context) => {
                let _ = send_tunnel_error(
                    context.tx(),
                    Some(context.generation()),
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
            Self::Connect(context) => {
                let _ =
                    send_forward_drop_report(context.tx(), stream_id, kind, reason, message).await;
            }
            Self::Listen(context) => {
                let _ =
                    send_forward_drop_report(context.tx(), stream_id, kind, reason, message).await;
            }
        }
    }

    async fn close_on_terminal_send_failure(&self, stream_id: u32) {
        match self {
            Self::Connect(context) => {
                if let Some(bind) = context.udp_binds().lock().await.remove(&stream_id) {
                    bind.cancel.cancel();
                }
            }
            Self::Listen(context) => {
                if let Some(reader) = context.udp_readers().lock().await.remove(&stream_id) {
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
    match active_access(&tunnel)
        .await?
        .require_bind_target(TunnelForwardProtocol::Udp, "udp bind")?
    {
        ActiveProtocolAccess::Listen(listen) => {
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
            listen
                .session()
                .replace_udp_bind(
                    frame.stream_id,
                    socket.clone(),
                    &tunnel.state.port_forward_limiter,
                )
                .await?;
            tunnel
                .send(endpoint_ok_frame(
                    FrameType::UdpBindOk,
                    frame.stream_id,
                    bound_endpoint,
                )?)
                .await?;
            reactivate_retained_udp_bind(listen.session()).await
        }
        ActiveProtocolAccess::Connect(connect) => {
            tunnel_udp_bind_connection_local(tunnel, connect, frame).await
        }
    }
}

pub(super) async fn tunnel_udp_bind_connection_local(
    tunnel: Arc<TunnelState>,
    connect: ConnectContext,
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
    let stream_cancel = connect.cancel().child_token();
    connect.udp_binds().lock().await.insert(
        frame.stream_id,
        ConnectionLocalUdpBind {
            socket: socket.clone(),
            _permit: permit,
            cancel: stream_cancel.clone(),
        },
    );
    tunnel
        .send(endpoint_ok_frame(
            FrameType::UdpBindOk,
            frame.stream_id,
            bound_endpoint,
        )?)
        .await?;
    tokio::spawn(tunnel_udp_read_loop_connection_local(
        connect,
        frame.stream_id,
        socket,
        stream_cancel,
    ));
    Ok(())
}

pub(super) async fn tunnel_udp_read_loop_connection_local(
    connect: ConnectContext,
    stream_id: u32,
    socket: Arc<UdpSocket>,
    cancel: CancellationToken,
) {
    tunnel_udp_read_loop(
        UdpReadLoopTarget::Connect(connect),
        stream_id,
        socket,
        cancel,
    )
    .await;
}

pub(super) async fn tunnel_udp_read_loop_attached_session(
    session: Arc<SessionState>,
    attachment: Arc<AttachmentState>,
    stream_id: u32,
    socket: Arc<UdpSocket>,
    cancel: CancellationToken,
) {
    tunnel_udp_read_loop(
        UdpReadLoopTarget::Listen(ListenContext::new(session, attachment)),
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
        let frame = match meta_frame(
            FrameType::UdpDatagram,
            stream_id,
            &UdpDatagramMeta {
                peer: peer.to_string(),
            },
        ) {
            Ok(frame) => frame,
            Err(err) => {
                target
                    .send_error_code(stream_id, err.code.to_string(), err.message)
                    .await;
                return;
            }
        };
        if let Err(err) = target
            .send_datagram(stream_id, frame.meta, buf[..read].to_vec())
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
    let socket = match active_access(tunnel)
        .await?
        .require_protocol(TunnelForwardProtocol::Udp, "udp datagram")?
    {
        ActiveProtocolAccess::Listen(listen) => listen
            .session()
            .udp_socket(frame.stream_id)
            .await
            .ok_or_else(|| {
                rpc_error(
                    RpcErrorCode::UnknownPortBind,
                    format!("unknown tunnel udp stream `{}`", frame.stream_id),
                )
            })?,
        ActiveProtocolAccess::Connect(connect) => connect
            .udp_binds()
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
    };
    socket
        .send_to(&frame.data, &meta.peer)
        .await
        .map_err(|err| rpc_error(RpcErrorCode::PortWriteFailed, err.to_string()))?;
    Ok(())
}
