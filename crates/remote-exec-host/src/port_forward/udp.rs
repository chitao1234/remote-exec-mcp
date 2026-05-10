use std::sync::Arc;

use remote_exec_proto::port_forward::normalize_endpoint;
use remote_exec_proto::port_tunnel::{ForwardDropKind, Frame, FrameType, TunnelForwardProtocol};
use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

use crate::HostRpcError;

use super::codec::{decode_frame_meta, encode_frame_meta};
use super::error::{is_recoverable_pressure_error, rpc_error};
use super::session::send_tunnel_error_with_sender;
use super::session::{AttachmentState, reactivate_retained_udp_bind};
use super::tunnel::send_tunnel_error;
use super::tunnel::tunnel_mode;
use super::{
    EndpointMeta, EndpointOkMeta, READ_BUF_SIZE, TransportUdpBind, TunnelMode, TunnelState,
    UdpDatagramMeta, send_forward_drop_report,
};

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
                .map_err(|err| rpc_error("invalid_endpoint", err.to_string()))?;
            let socket = Arc::new(
                UdpSocket::bind(&endpoint)
                    .await
                    .map_err(|err| rpc_error("port_bind_failed", err.to_string()))?,
            );
            let bound_endpoint = socket
                .local_addr()
                .map_err(|err| rpc_error("port_bind_failed", err.to_string()))?
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
            "invalid_port_tunnel",
            "udp bind requires an open udp listen tunnel",
        )),
        TunnelMode::Connect {
            protocol: TunnelForwardProtocol::Udp,
        } => tunnel_udp_bind_transport_owned(tunnel, frame).await,
        TunnelMode::Connect { .. } => Err(rpc_error(
            "invalid_port_tunnel",
            "udp bind requires an open udp connect tunnel",
        )),
        TunnelMode::Unopened => Err(rpc_error(
            "invalid_port_tunnel",
            "udp bind requires tunnel open",
        )),
    }
}

pub(super) async fn tunnel_udp_bind_transport_owned(
    tunnel: Arc<TunnelState>,
    frame: Frame,
) -> Result<(), HostRpcError> {
    let meta: EndpointMeta = decode_frame_meta(&frame)?;
    let endpoint = normalize_endpoint(&meta.endpoint)
        .map_err(|err| rpc_error("invalid_endpoint", err.to_string()))?;
    let socket = Arc::new(
        UdpSocket::bind(&endpoint)
            .await
            .map_err(|err| rpc_error("port_bind_failed", err.to_string()))?,
    );
    let bound_endpoint = socket
        .local_addr()
        .map_err(|err| rpc_error("port_bind_failed", err.to_string()))?
        .to_string();
    let permit = tunnel.state.port_forward_limiter.try_acquire_udp_bind()?;
    let stream_cancel = tunnel.cancel.child_token();
    tunnel.udp_binds.lock().await.insert(
        frame.stream_id,
        TransportUdpBind {
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
    tokio::spawn(tunnel_udp_read_loop_transport_owned(
        tunnel,
        frame.stream_id,
        socket,
        stream_cancel,
    ));
    Ok(())
}

pub(super) async fn tunnel_udp_read_loop_transport_owned(
    tunnel: Arc<TunnelState>,
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
                let _ = send_tunnel_error(
                    &tunnel,
                    stream_id,
                    "port_read_failed",
                    err.to_string(),
                    false,
                )
                .await;
                return;
            }
        };
        let meta = match encode_frame_meta(&UdpDatagramMeta {
            peer: peer.to_string(),
        }) {
            Ok(meta) => meta,
            Err(err) => {
                let _ = send_tunnel_error(&tunnel, stream_id, err.code, err.message, false).await;
                return;
            }
        };
        if let Err(err) = tunnel
            .send(Frame {
                frame_type: FrameType::UdpDatagram,
                flags: 0,
                stream_id,
                meta,
                data: buf[..read].to_vec(),
            })
            .await
        {
            if is_recoverable_pressure_error(&err) {
                let _ = send_forward_drop_report(
                    &tunnel.tx,
                    stream_id,
                    ForwardDropKind::UdpDatagram,
                    err.code,
                    err.message.clone(),
                )
                .await;
                tracing::debug!(
                    code = err.code,
                    message = %err.message,
                    "dropping udp datagram due to local port tunnel pressure"
                );
                continue;
            }
            let _ = send_tunnel_error(&tunnel, stream_id, err.code, err.message, false).await;
            if let Some(bind) = tunnel.udp_binds.lock().await.remove(&stream_id) {
                bind.cancel.cancel();
            }
            return;
        }
    }
}

pub(super) async fn tunnel_udp_read_loop_session_owned(
    attachment: Arc<AttachmentState>,
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
                let _ = send_tunnel_error_with_sender(
                    &attachment.tx,
                    stream_id,
                    "port_read_failed",
                    err.to_string(),
                    false,
                )
                .await;
                return;
            }
        };
        let meta = match encode_frame_meta(&UdpDatagramMeta {
            peer: peer.to_string(),
        }) {
            Ok(meta) => meta,
            Err(err) => {
                let _ = send_tunnel_error_with_sender(
                    &attachment.tx,
                    stream_id,
                    err.code,
                    err.message,
                    false,
                )
                .await;
                return;
            }
        };
        if let Err(err) = attachment
            .tx
            .send(Frame {
                frame_type: FrameType::UdpDatagram,
                flags: 0,
                stream_id,
                meta,
                data: buf[..read].to_vec(),
            })
            .await
        {
            if is_recoverable_pressure_error(&err) {
                let _ = send_forward_drop_report(
                    &attachment.tx,
                    stream_id,
                    ForwardDropKind::UdpDatagram,
                    err.code,
                    err.message.clone(),
                )
                .await;
                tracing::debug!(
                    code = err.code,
                    message = %err.message,
                    "dropping udp datagram due to local port tunnel pressure"
                );
                continue;
            }
            let _ = send_tunnel_error_with_sender(
                &attachment.tx,
                stream_id,
                err.code,
                err.message,
                false,
            )
            .await;
            if let Some(reader) = attachment.udp_readers.lock().await.remove(&stream_id) {
                reader.cancel.cancel();
            }
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
                "unknown_port_bind",
                format!("unknown tunnel udp stream `{}`", frame.stream_id),
            )
        })?,
        TunnelMode::Listen { .. } => {
            return Err(rpc_error(
                "invalid_port_tunnel",
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
                    "unknown_port_bind",
                    format!("unknown tunnel udp stream `{}`", frame.stream_id),
                )
            })?,
        TunnelMode::Connect { .. } => {
            return Err(rpc_error(
                "invalid_port_tunnel",
                "udp datagram requires an open udp tunnel",
            ));
        }
        TunnelMode::Unopened => {
            return Err(rpc_error(
                "invalid_port_tunnel",
                "udp datagram requires tunnel open",
            ));
        }
    };
    socket
        .send_to(&frame.data, &meta.peer)
        .await
        .map_err(|err| rpc_error("port_write_failed", err.to_string()))?;
    Ok(())
}
