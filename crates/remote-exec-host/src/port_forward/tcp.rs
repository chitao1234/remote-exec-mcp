use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use remote_exec_proto::port_forward::{ensure_nonzero_connect_endpoint, normalize_endpoint};
use remote_exec_proto::port_tunnel::{ForwardDropKind, Frame, FrameType, TunnelForwardProtocol};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
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
use super::{
    EndpointMeta, EndpointOkMeta, READ_BUF_SIZE, TCP_WRITE_QUEUE_FRAMES, TcpAcceptMeta,
    TcpWriteCommand, TcpWriterHandle, TunnelMode, TunnelState, send_forward_drop_report,
};

fn spawn_tcp_writer_task(writer: OwnedWriteHalf, cancel: CancellationToken) -> TcpWriterHandle {
    let (tx, rx) = mpsc::channel(TCP_WRITE_QUEUE_FRAMES);
    tokio::spawn(tunnel_tcp_write_loop(writer, rx, cancel.clone()));
    TcpWriterHandle { tx, cancel }
}

async fn tcp_connect_with_timeout(
    tunnel: &TunnelState,
    endpoint: &str,
) -> Result<TcpStream, HostRpcError> {
    let connect_timeout =
        Duration::from_millis(tunnel.state.config.port_forward_limits.connect_timeout_ms);
    tokio::time::timeout(connect_timeout, TcpStream::connect(endpoint))
        .await
        .map_err(|_| rpc_error("port_connect_failed", "tcp connect timed out"))?
        .map_err(|err| rpc_error("port_connect_failed", err.to_string()))
}

pub(super) async fn tunnel_tcp_listen(
    tunnel: Arc<TunnelState>,
    frame: Frame,
) -> Result<(), HostRpcError> {
    let session = match tunnel_mode(&tunnel).await {
        TunnelMode::Listen {
            protocol: TunnelForwardProtocol::Tcp,
            session,
        } => session,
        TunnelMode::Listen { .. } => {
            return Err(rpc_error(
                "invalid_port_tunnel",
                "tcp listen requires an open tcp listen tunnel",
            ));
        }
        TunnelMode::Connect { .. } => {
            return Err(rpc_error(
                "invalid_port_tunnel",
                "tcp listen requires an open listen tunnel",
            ));
        }
        TunnelMode::Unopened => {
            return Err(rpc_error(
                "invalid_port_tunnel",
                "tcp listen requires tunnel open",
            ));
        }
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
        .replace_listener(
            frame.stream_id,
            listener.clone(),
            &tunnel.state.port_forward_limiter,
        )
        .await?;
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
        let stream_permit = match attachment.tx.limiter.try_acquire_active_tcp_stream() {
            Ok(permit) => permit,
            Err(err) => {
                drop(stream);
                let _ = send_forward_drop_report(
                    &attachment.tx,
                    listener_stream_id(&session).await.unwrap_or(0),
                    ForwardDropKind::TcpStream,
                    err.code,
                    err.message.clone(),
                )
                .await;
                tracing::debug!(
                    code = err.code,
                    message = %err.message,
                    "dropping accepted tcp stream due to local port tunnel pressure"
                );
                continue;
            }
        };
        let stream_id = session
            .next_daemon_stream_id
            .fetch_add(2, Ordering::Relaxed);
        let listener_stream = listener_stream_id(&session).await.unwrap_or(0);
        let (reader, writer) = stream.into_split();
        let stream_cancel = attachment.cancel.child_token();
        attachment.tcp_writers.lock().await.insert(
            stream_id,
            spawn_tcp_writer_task(writer, stream_cancel.clone()),
        );
        attachment
            .tcp_stream_permits
            .lock()
            .await
            .insert(stream_id, stream_permit);
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
                        let _ = attachment.tcp_writers.lock().await.remove(&stream_id);
                        let _ = attachment
                            .tcp_stream_permits
                            .lock()
                            .await
                            .remove(&stream_id);
                        let _ = attachment.stream_cancels.lock().await.remove(&stream_id);
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
            let _ = attachment.tcp_writers.lock().await.remove(&stream_id);
            let _ = attachment
                .tcp_stream_permits
                .lock()
                .await
                .remove(&stream_id);
            let _ = attachment.stream_cancels.lock().await.remove(&stream_id);
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

pub(super) async fn tunnel_tcp_connect(
    tunnel: Arc<TunnelState>,
    frame: Frame,
) -> Result<(), HostRpcError> {
    match tunnel_mode(&tunnel).await {
        TunnelMode::Connect {
            protocol: TunnelForwardProtocol::Tcp,
        } => tunnel_tcp_connect_transport_owned(tunnel, frame).await,
        TunnelMode::Connect { .. } => Err(rpc_error(
            "invalid_port_tunnel",
            "tcp connect requires an open tcp connect tunnel",
        )),
        TunnelMode::Listen { .. } => Err(rpc_error(
            "invalid_port_tunnel",
            "tcp connect requires an open connect tunnel",
        )),
        TunnelMode::Unopened => Err(rpc_error(
            "invalid_port_tunnel",
            "tcp connect requires tunnel open",
        )),
    }
}

pub(super) async fn tunnel_tcp_connect_transport_owned(
    tunnel: Arc<TunnelState>,
    frame: Frame,
) -> Result<(), HostRpcError> {
    let meta: EndpointMeta = decode_frame_meta(&frame)?;
    let endpoint = ensure_nonzero_connect_endpoint(&meta.endpoint)
        .map_err(|err| rpc_error("invalid_endpoint", err.to_string()))?;
    let stream = tcp_connect_with_timeout(&tunnel, endpoint.as_str()).await?;
    let stream_permit = tunnel
        .state
        .port_forward_limiter
        .try_acquire_active_tcp_stream()?;
    let (reader, writer) = stream.into_split();
    let stream_cancel = tunnel.cancel.child_token();
    tunnel.tcp_writers.lock().await.insert(
        frame.stream_id,
        spawn_tcp_writer_task(writer, stream_cancel.clone()),
    );
    tunnel
        .tcp_stream_permits
        .lock()
        .await
        .insert(frame.stream_id, stream_permit);
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
            _ = cancel.cancelled() => {
                cleanup_transport_tcp_stream(&tunnel, stream_id).await;
                return;
            }
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
                if let Err(err) = tunnel
                    .send(Frame {
                        frame_type: FrameType::TcpData,
                        flags: 0,
                        stream_id,
                        meta: Vec::new(),
                        data: buf[..read].to_vec(),
                    })
                    .await
                {
                    let _ =
                        send_tunnel_error(&tunnel, stream_id, err.code, err.message, false).await;
                    cancel_transport_tcp_stream(&tunnel, stream_id).await;
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
                cancel_transport_tcp_stream(&tunnel, stream_id).await;
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
            _ = cancel.cancelled() => {
                cleanup_session_tcp_stream(&attachment, stream_id).await;
                return;
            }
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
                if let Err(err) = attachment
                    .tx
                    .send(Frame {
                        frame_type: FrameType::TcpData,
                        flags: 0,
                        stream_id,
                        meta: Vec::new(),
                        data: buf[..read].to_vec(),
                    })
                    .await
                {
                    let _ = send_tunnel_error_with_sender(
                        &attachment.tx,
                        stream_id,
                        err.code,
                        err.message,
                        false,
                    )
                    .await;
                    cancel_session_tcp_stream(&attachment, stream_id).await;
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
                cancel_session_tcp_stream(&attachment, stream_id).await;
                return;
            }
        }
    }
}

async fn tunnel_tcp_write_loop(
    mut writer: OwnedWriteHalf,
    mut rx: mpsc::Receiver<TcpWriteCommand>,
    cancel: CancellationToken,
) {
    loop {
        let command = tokio::select! {
            _ = cancel.cancelled() => return,
            command = rx.recv() => {
                let Some(command) = command else {
                    return;
                };
                command
            }
        };
        match command {
            TcpWriteCommand::Data(data) => {
                let result = tokio::select! {
                    _ = cancel.cancelled() => return,
                    result = writer.write_all(&data) => result,
                };
                if result.is_err() {
                    cancel.cancel();
                    return;
                }
            }
            TcpWriteCommand::Shutdown => {
                let _ = tokio::select! {
                    _ = cancel.cancelled() => return,
                    result = writer.shutdown() => result,
                };
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
        TunnelMode::Listen {
            protocol: TunnelForwardProtocol::Tcp,
            session,
        } => {
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
        TunnelMode::Listen { .. }
        | TunnelMode::Connect {
            protocol: TunnelForwardProtocol::Udp,
        } => {
            return Err(rpc_error(
                "invalid_port_tunnel",
                "tcp data requires an open tcp tunnel",
            ));
        }
        TunnelMode::Connect {
            protocol: TunnelForwardProtocol::Tcp,
        } => tunnel
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
        TunnelMode::Unopened => {
            return Err(rpc_error(
                "invalid_port_tunnel",
                "tcp data requires tunnel open",
            ));
        }
    };
    writer
        .tx
        .try_send(TcpWriteCommand::Data(data.to_vec()))
        .map_err(|err| match err {
            mpsc::error::TrySendError::Full(_) => {
                writer.cancel.cancel();
                rpc_error(
                    "port_tunnel_limit_exceeded",
                    "tcp write queue limit reached",
                )
            }
            mpsc::error::TrySendError::Closed(_) => {
                rpc_error("port_connection_closed", "connection was closed")
            }
        })
}

async fn cleanup_transport_tcp_stream(tunnel: &TunnelState, stream_id: u32) {
    let _ = tunnel.stream_cancels.lock().await.remove(&stream_id);
    let _ = tunnel.tcp_writers.lock().await.remove(&stream_id);
    let _ = tunnel.tcp_stream_permits.lock().await.remove(&stream_id);
}

async fn cancel_transport_tcp_stream(tunnel: &TunnelState, stream_id: u32) {
    if let Some(cancel) = tunnel.stream_cancels.lock().await.remove(&stream_id) {
        cancel.cancel();
    }
    let _ = tunnel.tcp_writers.lock().await.remove(&stream_id);
    let _ = tunnel.tcp_stream_permits.lock().await.remove(&stream_id);
}

async fn cleanup_session_tcp_stream(attachment: &AttachmentState, stream_id: u32) {
    let _ = attachment.stream_cancels.lock().await.remove(&stream_id);
    let _ = attachment.tcp_writers.lock().await.remove(&stream_id);
    let _ = attachment
        .tcp_stream_permits
        .lock()
        .await
        .remove(&stream_id);
}

async fn cancel_session_tcp_stream(attachment: &AttachmentState, stream_id: u32) {
    if let Some(cancel) = attachment.stream_cancels.lock().await.remove(&stream_id) {
        cancel.cancel();
    }
    let _ = attachment.tcp_writers.lock().await.remove(&stream_id);
    let _ = attachment
        .tcp_stream_permits
        .lock()
        .await
        .remove(&stream_id);
}

pub(super) async fn tunnel_tcp_eof(
    tunnel: &Arc<TunnelState>,
    stream_id: u32,
) -> Result<(), HostRpcError> {
    let writer = match tunnel_mode(tunnel).await {
        TunnelMode::Listen {
            protocol: TunnelForwardProtocol::Tcp,
            session,
        } => {
            let Some(attachment) = session.current_attachment().await else {
                return Ok(());
            };
            attachment.tcp_writers.lock().await.get(&stream_id).cloned()
        }
        TunnelMode::Connect {
            protocol: TunnelForwardProtocol::Tcp,
        } => tunnel.tcp_writers.lock().await.get(&stream_id).cloned(),
        TunnelMode::Listen { .. } | TunnelMode::Connect { .. } => None,
        TunnelMode::Unopened => None,
    };
    if let Some(writer) = writer {
        let _ = writer.tx.try_send(TcpWriteCommand::Shutdown);
    }
    Ok(())
}

pub(super) async fn tunnel_close_stream(
    tunnel: &Arc<TunnelState>,
    stream_id: u32,
) -> Result<(), HostRpcError> {
    match tunnel_mode(tunnel).await {
        TunnelMode::Listen {
            protocol: TunnelForwardProtocol::Tcp,
            session,
        }
        | TunnelMode::Listen {
            protocol: TunnelForwardProtocol::Udp,
            session,
        } => {
            if let Some(attachment) = session.current_attachment().await {
                if let Some(cancel) = attachment.stream_cancels.lock().await.remove(&stream_id) {
                    cancel.cancel();
                }
                attachment.tcp_writers.lock().await.remove(&stream_id);
                attachment
                    .tcp_stream_permits
                    .lock()
                    .await
                    .remove(&stream_id);
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
        TunnelMode::Connect { .. } => {
            if let Some(cancel) = tunnel.stream_cancels.lock().await.remove(&stream_id) {
                cancel.cancel();
            }
            tunnel.tcp_writers.lock().await.remove(&stream_id);
            tunnel.tcp_stream_permits.lock().await.remove(&stream_id);
            tunnel.udp_sockets.lock().await.remove(&stream_id);
        }
        TunnelMode::Unopened => {}
    }
    tunnel
        .send(Frame {
            frame_type: FrameType::Close,
            flags: 0,
            stream_id,
            meta: Vec::new(),
            data: Vec::new(),
        })
        .await?;
    Ok(())
}
