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
use super::error::{bind_error, is_recoverable_pressure_error, request_error};
use super::frames::{endpoint_ok_frame, frame as raw_frame, meta_frame};
use super::session::{AttachmentState, SessionState, reactivate_retained_udp_bind};
use super::{
    ConnectionLocalUdpBind, READ_BUF_SIZE, TunnelSender, TunnelState, send_forward_drop_report,
};

enum UdpReadLoopTarget {
    Connect(ConnectContext),
    Listen(ListenContext),
}

impl UdpReadLoopTarget {
    fn tx(&self) -> &TunnelSender {
        match self {
            Self::Connect(context) => context.tx(),
            Self::Listen(context) => context.tx(),
        }
    }

    fn generation(&self) -> u64 {
        match self {
            Self::Connect(context) => context.generation(),
            Self::Listen(context) => context.generation(),
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
                .map_err(|err| request_error(RpcErrorCode::InvalidEndpoint, err.to_string()))?;
            let socket = Arc::new(
                UdpSocket::bind(&endpoint)
                    .await
                    .map_err(bind_error(RpcErrorCode::PortBindFailed))?,
            );
            let bound_endpoint = socket
                .local_addr()
                .map_err(bind_error(RpcErrorCode::PortBindFailed))?
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
        .map_err(|err| request_error(RpcErrorCode::InvalidEndpoint, err.to_string()))?;
    let socket = Arc::new(
        UdpSocket::bind(&endpoint)
            .await
            .map_err(bind_error(RpcErrorCode::PortBindFailed))?,
    );
    let bound_endpoint = socket
        .local_addr()
        .map_err(bind_error(RpcErrorCode::PortBindFailed))?
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
    let tx = target.tx();
    let generation = target.generation();
    let mut buf = vec![0; READ_BUF_SIZE];
    loop {
        let received = tokio::select! {
            _ = cancel.cancelled() => return,
            received = socket.recv_from(&mut buf) => received,
        };
        let (read, peer) = match received {
            Ok(received) => received,
            Err(err) => {
                let _ = send_tunnel_error(
                    tx,
                    Some(generation),
                    stream_id,
                    RpcErrorCode::PortReadFailed,
                    err.to_string(),
                    false,
                )
                .await;
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
                let _ = send_tunnel_error_code(
                    tx,
                    Some(generation),
                    stream_id,
                    err.wire_code(),
                    err.message,
                    false,
                )
                .await;
                return;
            }
        };
        if let Err(err) = tx
            .send(raw_frame(
                FrameType::UdpDatagram,
                stream_id,
                frame.meta,
                buf[..read].to_vec(),
            ))
            .await
        {
            if is_recoverable_pressure_error(&err) {
                let _ = send_forward_drop_report(
                    tx,
                    stream_id,
                    ForwardDropKind::UdpDatagram,
                    err.wire_code(),
                    err.message.clone(),
                )
                .await;
                tracing::debug!(
                    code = err.wire_code(),
                    message = %err.message,
                    "dropping udp datagram due to local port tunnel pressure"
                );
                continue;
            }
            let _ = send_tunnel_error_code(
                tx,
                Some(generation),
                stream_id,
                err.wire_code(),
                err.message,
                false,
            )
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
                request_error(
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
                request_error(
                    RpcErrorCode::UnknownPortBind,
                    format!("unknown tunnel udp stream `{}`", frame.stream_id),
                )
            })?,
    };
    socket
        .send_to(&frame.data, &meta.peer)
        .await
        .map_err(bind_error(RpcErrorCode::PortWriteFailed))?;
    Ok(())
}
