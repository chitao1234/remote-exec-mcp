use std::sync::Arc;
use std::sync::atomic::Ordering;

use remote_exec_proto::port_forward::{ensure_nonzero_connect_endpoint, normalize_endpoint};
use remote_exec_proto::port_tunnel::{Frame, FrameType};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::OwnedReadHalf;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::HostRpcError;

use super::codec::{decode_frame_meta, encode_frame_meta};
use super::error::{SessionCloseMode, rpc_error};
use super::session::{
    AttachmentState, SessionState, close_attached_session, listener_stream_id,
    send_tunnel_error_with_sender, udp_bind_stream_id, wait_for_session_attachment,
};
use super::tunnel::send_tunnel_error;
use super::tunnel::tunnel_mode;
use super::{EndpointMeta, EndpointOkMeta, READ_BUF_SIZE, TcpAcceptMeta, TunnelMode, TunnelState};

pub(super) async fn tunnel_tcp_listen(
    tunnel: Arc<TunnelState>,
    frame: Frame,
) -> Result<(), HostRpcError> {
    let TunnelMode::Session(session) = tunnel_mode(&tunnel).await else {
        return tunnel_tcp_listen_transport_owned(tunnel, frame).await;
    };
    let meta: EndpointMeta = decode_frame_meta(&frame)?;
    let endpoint = normalize_endpoint(&meta.endpoint)
        .map_err(|err| rpc_error("invalid_endpoint", err.to_string()))?;
    let listener = Arc::new(
        TcpListener::bind(&endpoint)
            .await
            .map_err(|err| rpc_error("port_bind_failed", err.to_string()))?,
    );
    let bound_endpoint = listener
        .local_addr()
        .map_err(|err| rpc_error("port_bind_failed", err.to_string()))?
        .to_string();
    session
        .replace_listener(frame.stream_id, listener.clone())
        .await;
    tunnel
        .send(Frame {
            frame_type: FrameType::TcpListenOk,
            flags: 0,
            stream_id: frame.stream_id,
            meta: encode_frame_meta(&EndpointOkMeta {
                endpoint: bound_endpoint.clone(),
            })?,
            data: Vec::new(),
        })
        .await?;

    tracing::info!(
        target = %tunnel.state.config.target,
        stream_id = frame.stream_id,
        endpoint = %bound_endpoint,
        "opened port tunnel tcp listener"
    );
    tokio::spawn(tunnel_tcp_accept_loop(session, listener));
    Ok(())
}

pub(super) async fn tunnel_tcp_listen_transport_owned(
    tunnel: Arc<TunnelState>,
    frame: Frame,
) -> Result<(), HostRpcError> {
    let meta: EndpointMeta = decode_frame_meta(&frame)?;
    let endpoint = normalize_endpoint(&meta.endpoint)
        .map_err(|err| rpc_error("invalid_endpoint", err.to_string()))?;
    let listener = TcpListener::bind(&endpoint)
        .await
        .map_err(|err| rpc_error("port_bind_failed", err.to_string()))?;
    let bound_endpoint = listener
        .local_addr()
        .map_err(|err| rpc_error("port_bind_failed", err.to_string()))?
        .to_string();
    let stream_cancel = tunnel.cancel.child_token();
    tunnel
        .stream_cancels
        .lock()
        .await
        .insert(frame.stream_id, stream_cancel.clone());
    tunnel
        .send(Frame {
            frame_type: FrameType::TcpListenOk,
            flags: 0,
            stream_id: frame.stream_id,
            meta: encode_frame_meta(&EndpointOkMeta {
                endpoint: bound_endpoint.clone(),
            })?,
            data: Vec::new(),
        })
        .await?;

    tracing::info!(
        target = %tunnel.state.config.target,
        stream_id = frame.stream_id,
        endpoint = %bound_endpoint,
        "opened port tunnel tcp listener"
    );
    tokio::spawn(tunnel_tcp_accept_loop_transport_owned(
        tunnel,
        frame.stream_id,
        listener,
        stream_cancel,
    ));
    Ok(())
}

pub(super) async fn tunnel_tcp_accept_loop(session: Arc<SessionState>, listener: Arc<TcpListener>) {
    loop {
        let Some(attachment) = wait_for_session_attachment(&session).await else {
            return;
        };
        let accepted = tokio::select! {
            _ = session.root_cancel.cancelled() => return,
            _ = attachment.cancel.cancelled() => continue,
            accepted = listener.accept() => accepted,
        };
        let (stream, peer) = match accepted {
            Ok(accepted) => accepted,
            Err(err) => {
                let _ = send_tunnel_error_with_sender(
                    &attachment.tx,
                    listener_stream_id(&session).await.unwrap_or(0),
                    "port_accept_failed",
                    err.to_string(),
                    false,
                )
                .await;
                return;
            }
        };
        if attachment.cancel.is_cancelled() {
            drop(stream);
            continue;
        }
        let stream_id = session
            .next_daemon_stream_id
            .fetch_add(2, Ordering::Relaxed);
        let listener_stream = listener_stream_id(&session).await.unwrap_or(0);
        let (reader, writer) = stream.into_split();
        attachment
            .tcp_writers
            .lock()
            .await
            .insert(stream_id, Arc::new(Mutex::new(writer)));
        let stream_cancel = attachment.cancel.child_token();
        attachment
            .stream_cancels
            .lock()
            .await
            .insert(stream_id, stream_cancel.clone());
        if attachment
            .tx
            .send(Frame {
                frame_type: FrameType::TcpAccept,
                flags: 0,
                stream_id,
                meta: match encode_frame_meta(&TcpAcceptMeta {
                    listener_stream_id: listener_stream,
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
                        continue;
                    }
                },
                data: Vec::new(),
            })
            .await
            .is_err()
        {
            return;
        }
        tokio::spawn(tunnel_tcp_read_loop_session_owned(
            attachment,
            stream_id,
            reader,
            stream_cancel,
        ));
    }
}

pub(super) async fn tunnel_tcp_accept_loop_transport_owned(
    tunnel: Arc<TunnelState>,
    listener_stream_id: u32,
    listener: TcpListener,
    cancel: CancellationToken,
) {
    loop {
        let accepted = tokio::select! {
            _ = cancel.cancelled() => return,
            accepted = listener.accept() => accepted,
        };
        let (stream, peer) = match accepted {
            Ok(accepted) => accepted,
            Err(err) => {
                let _ = send_tunnel_error(
                    &tunnel,
                    listener_stream_id,
                    "port_accept_failed",
                    err.to_string(),
                    false,
                )
                .await;
                return;
            }
        };
        let stream_id = tunnel.next_daemon_stream_id.fetch_add(2, Ordering::Relaxed);
        let (reader, writer) = stream.into_split();
        tunnel
            .tcp_writers
            .lock()
            .await
            .insert(stream_id, Arc::new(Mutex::new(writer)));
        let stream_cancel = tunnel.cancel.child_token();
        tunnel
            .stream_cancels
            .lock()
            .await
            .insert(stream_id, stream_cancel.clone());
        if tunnel
            .send(Frame {
                frame_type: FrameType::TcpAccept,
                flags: 0,
                stream_id,
                meta: match encode_frame_meta(&TcpAcceptMeta {
                    listener_stream_id,
                    peer: peer.to_string(),
                }) {
                    Ok(meta) => meta,
                    Err(err) => {
                        let _ = send_tunnel_error(&tunnel, stream_id, err.code, err.message, false)
                            .await;
                        continue;
                    }
                },
                data: Vec::new(),
            })
            .await
            .is_err()
        {
            return;
        }
        tokio::spawn(tunnel_tcp_read_loop_transport_owned(
            tunnel.clone(),
            stream_id,
            reader,
            stream_cancel,
        ));
    }
}

pub(super) async fn tunnel_tcp_connect(
    tunnel: Arc<TunnelState>,
    frame: Frame,
) -> Result<(), HostRpcError> {
    let TunnelMode::Session(session) = tunnel_mode(&tunnel).await else {
        return tunnel_tcp_connect_transport_owned(tunnel, frame).await;
    };
    let meta: EndpointMeta = decode_frame_meta(&frame)?;
    let endpoint = ensure_nonzero_connect_endpoint(&meta.endpoint)
        .map_err(|err| rpc_error("invalid_endpoint", err.to_string()))?;
    let stream = TcpStream::connect(endpoint.as_str())
        .await
        .map_err(|err| rpc_error("port_connect_failed", err.to_string()))?;
    let (reader, writer) = stream.into_split();
    let attachment = session
        .current_attachment()
        .await
        .ok_or_else(|| rpc_error("port_tunnel_closed", "port tunnel attachment is closed"))?;
    attachment
        .tcp_writers
        .lock()
        .await
        .insert(frame.stream_id, Arc::new(Mutex::new(writer)));
    let stream_cancel = attachment.cancel.child_token();
    attachment
        .stream_cancels
        .lock()
        .await
        .insert(frame.stream_id, stream_cancel.clone());
    tunnel
        .send(Frame {
            frame_type: FrameType::TcpConnectOk,
            flags: 0,
            stream_id: frame.stream_id,
            meta: Vec::new(),
            data: Vec::new(),
        })
        .await?;
    tokio::spawn(tunnel_tcp_read_loop_session_owned(
        attachment,
        frame.stream_id,
        reader,
        stream_cancel,
    ));
    Ok(())
}

pub(super) async fn tunnel_tcp_connect_transport_owned(
    tunnel: Arc<TunnelState>,
    frame: Frame,
) -> Result<(), HostRpcError> {
    let meta: EndpointMeta = decode_frame_meta(&frame)?;
    let endpoint = ensure_nonzero_connect_endpoint(&meta.endpoint)
        .map_err(|err| rpc_error("invalid_endpoint", err.to_string()))?;
    let stream = TcpStream::connect(endpoint.as_str())
        .await
        .map_err(|err| rpc_error("port_connect_failed", err.to_string()))?;
    let (reader, writer) = stream.into_split();
    tunnel
        .tcp_writers
        .lock()
        .await
        .insert(frame.stream_id, Arc::new(Mutex::new(writer)));
    let stream_cancel = tunnel.cancel.child_token();
    tunnel
        .stream_cancels
        .lock()
        .await
        .insert(frame.stream_id, stream_cancel.clone());
    tunnel
        .send(Frame {
            frame_type: FrameType::TcpConnectOk,
            flags: 0,
            stream_id: frame.stream_id,
            meta: Vec::new(),
            data: Vec::new(),
        })
        .await?;
    tokio::spawn(tunnel_tcp_read_loop_transport_owned(
        tunnel,
        frame.stream_id,
        reader,
        stream_cancel,
    ));
    Ok(())
}

pub(super) async fn tunnel_tcp_read_loop_transport_owned(
    tunnel: Arc<TunnelState>,
    stream_id: u32,
    mut reader: OwnedReadHalf,
    cancel: CancellationToken,
) {
    let mut buf = vec![0; READ_BUF_SIZE];
    loop {
        let read = tokio::select! {
            _ = cancel.cancelled() => return,
            read = reader.read(&mut buf) => read,
        };
        match read {
            Ok(0) => {
                let _ = tunnel
                    .send(Frame {
                        frame_type: FrameType::TcpEof,
                        flags: 0,
                        stream_id,
                        meta: Vec::new(),
                        data: Vec::new(),
                    })
                    .await;
                let _ = tunnel.stream_cancels.lock().await.remove(&stream_id);
                return;
            }
            Ok(read) => {
                if tunnel
                    .send(Frame {
                        frame_type: FrameType::TcpData,
                        flags: 0,
                        stream_id,
                        meta: Vec::new(),
                        data: buf[..read].to_vec(),
                    })
                    .await
                    .is_err()
                {
                    return;
                }
            }
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
        }
    }
}

pub(super) async fn tunnel_tcp_read_loop_session_owned(
    attachment: Arc<AttachmentState>,
    stream_id: u32,
    mut reader: OwnedReadHalf,
    cancel: CancellationToken,
) {
    let mut buf = vec![0; READ_BUF_SIZE];
    loop {
        let read = tokio::select! {
            _ = cancel.cancelled() => return,
            read = reader.read(&mut buf) => read,
        };
        match read {
            Ok(0) => {
                let _ = attachment
                    .tx
                    .send(Frame {
                        frame_type: FrameType::TcpEof,
                        flags: 0,
                        stream_id,
                        meta: Vec::new(),
                        data: Vec::new(),
                    })
                    .await;
                let _ = attachment.stream_cancels.lock().await.remove(&stream_id);
                return;
            }
            Ok(read) => {
                if attachment
                    .tx
                    .send(Frame {
                        frame_type: FrameType::TcpData,
                        flags: 0,
                        stream_id,
                        meta: Vec::new(),
                        data: buf[..read].to_vec(),
                    })
                    .await
                    .is_err()
                {
                    let _ = attachment.tcp_writers.lock().await.remove(&stream_id);
                    let _ = attachment.stream_cancels.lock().await.remove(&stream_id);
                    return;
                }
            }
            Err(err) => {
                let _ = send_tunnel_error_with_sender(
                    &attachment.tx,
                    stream_id,
                    "port_read_failed",
                    err.to_string(),
                    false,
                )
                .await;
                let _ = attachment.tcp_writers.lock().await.remove(&stream_id);
                let _ = attachment.stream_cancels.lock().await.remove(&stream_id);
                return;
            }
        }
    }
}

pub(super) async fn tunnel_tcp_data(
    tunnel: &Arc<TunnelState>,
    stream_id: u32,
    data: &[u8],
) -> Result<(), HostRpcError> {
    let writer = match tunnel_mode(tunnel).await {
        TunnelMode::Session(session) => {
            let attachment = session.current_attachment().await.ok_or_else(|| {
                rpc_error("port_tunnel_closed", "port tunnel attachment is closed")
            })?;
            attachment
                .tcp_writers
                .lock()
                .await
                .get(&stream_id)
                .cloned()
                .ok_or_else(|| {
                    rpc_error(
                        "unknown_port_connection",
                        format!("unknown tunnel tcp stream `{stream_id}`"),
                    )
                })?
        }
        TunnelMode::Transport => tunnel
            .tcp_writers
            .lock()
            .await
            .get(&stream_id)
            .cloned()
            .ok_or_else(|| {
                rpc_error(
                    "unknown_port_connection",
                    format!("unknown tunnel tcp stream `{stream_id}`"),
                )
            })?,
    };
    writer
        .lock()
        .await
        .write_all(data)
        .await
        .map_err(|err| rpc_error("port_write_failed", err.to_string()))
}

pub(super) async fn tunnel_tcp_eof(
    tunnel: &Arc<TunnelState>,
    stream_id: u32,
) -> Result<(), HostRpcError> {
    let writer = match tunnel_mode(tunnel).await {
        TunnelMode::Session(session) => {
            let Some(attachment) = session.current_attachment().await else {
                return Ok(());
            };
            attachment.tcp_writers.lock().await.get(&stream_id).cloned()
        }
        TunnelMode::Transport => tunnel.tcp_writers.lock().await.get(&stream_id).cloned(),
    };
    if let Some(writer) = writer {
        writer
            .lock()
            .await
            .shutdown()
            .await
            .map_err(|err| rpc_error("port_write_failed", err.to_string()))?;
    }
    Ok(())
}

pub(super) async fn tunnel_close_stream(
    tunnel: &Arc<TunnelState>,
    stream_id: u32,
) -> Result<(), HostRpcError> {
    match tunnel_mode(tunnel).await {
        TunnelMode::Session(session) => {
            if let Some(attachment) = session.current_attachment().await {
                if let Some(cancel) = attachment.stream_cancels.lock().await.remove(&stream_id) {
                    cancel.cancel();
                }
                attachment.tcp_writers.lock().await.remove(&stream_id);
            }
            if let Some(bind_stream_id) = udp_bind_stream_id(&session).await {
                if bind_stream_id == stream_id {
                    close_attached_session(tunnel, SessionCloseMode::GracefulClose).await;
                }
            }
            if let Some(listener_stream) = listener_stream_id(&session).await {
                if listener_stream == stream_id {
                    close_attached_session(tunnel, SessionCloseMode::GracefulClose).await;
                }
            }
        }
        TunnelMode::Transport => {
            if let Some(cancel) = tunnel.stream_cancels.lock().await.remove(&stream_id) {
                cancel.cancel();
            }
            tunnel.tcp_writers.lock().await.remove(&stream_id);
            tunnel.udp_sockets.lock().await.remove(&stream_id);
        }
    }
    Ok(())
}
