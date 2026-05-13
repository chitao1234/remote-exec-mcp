use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use remote_exec_proto::port_forward::{ensure_nonzero_connect_endpoint, normalize_endpoint};
use remote_exec_proto::port_tunnel::{
    EndpointMeta, ForwardDropKind, Frame, FrameType, TcpAcceptMeta, TunnelForwardProtocol,
};
use remote_exec_proto::rpc::RpcErrorCode;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::HostRpcError;

use super::access::{OpenProtocolAccess, OpenTunnelRole, tunnel_access};
use super::codec::decode_frame_meta;
use super::error::{SessionCloseMode, rpc_error};
use super::frames::{data_frame, empty_frame, endpoint_ok_frame, meta_frame};
use super::session::{
    AttachmentState, SessionState, close_attached_session, listener_stream_id,
    send_tunnel_error_code_with_sender, send_tunnel_error_with_sender, udp_bind_stream_id,
    wait_for_session_attachment,
};
use super::tunnel::{send_tunnel_error, send_tunnel_error_code};
use super::{
    READ_BUF_SIZE, TCP_WRITE_QUEUE_FRAMES, TcpStreamEntry, TcpWriteCommand, TcpWriterHandle,
    TunnelState, send_forward_drop_report,
};

const TCP_CONTROL_SEND_TIMEOUT: Duration = Duration::from_secs(1);
type TcpStreamMap = tokio::sync::Mutex<std::collections::HashMap<u32, TcpStreamEntry>>;

enum TcpReadLoopTarget {
    Connection(Arc<TunnelState>),
    AttachedSession(Arc<AttachmentState>),
}

impl TcpReadLoopTarget {
    async fn send_frame(&self, frame: Frame) -> Result<(), HostRpcError> {
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

    async fn cleanup_stream(&self, stream_id: u32) {
        match self {
            Self::Connection(tunnel) => cleanup_tcp_stream(&tunnel.tcp_streams, stream_id).await,
            Self::AttachedSession(attachment) => {
                cleanup_tcp_stream(&attachment.tcp_streams, stream_id).await
            }
        }
    }

    async fn cancel_stream(&self, stream_id: u32) {
        match self {
            Self::Connection(tunnel) => cancel_tcp_stream(&tunnel.tcp_streams, stream_id).await,
            Self::AttachedSession(attachment) => {
                cancel_tcp_stream(&attachment.tcp_streams, stream_id).await
            }
        }
    }

    async fn clear_cancel(&self, stream_id: u32) {
        match self {
            Self::Connection(tunnel) => clear_tcp_cancel(&tunnel.tcp_streams, stream_id).await,
            Self::AttachedSession(attachment) => {
                clear_tcp_cancel(&attachment.tcp_streams, stream_id).await
            }
        }
    }
}

fn spawn_tcp_writer_task(writer: OwnedWriteHalf, cancel: CancellationToken) -> TcpWriterHandle {
    let (tx, rx) = mpsc::channel(TCP_WRITE_QUEUE_FRAMES);
    tokio::spawn(tunnel_tcp_write_loop(writer, rx, cancel.clone()));
    TcpWriterHandle { tx, cancel }
}

async fn tcp_connect_with_timeout(
    tunnel: &TunnelState,
    endpoint: &str,
) -> Result<TcpStream, HostRpcError> {
    let connect_timeout = tunnel
        .state
        .config
        .port_forward_limits
        .timeouts()
        .connect_timeout();
    tokio::time::timeout(connect_timeout, TcpStream::connect(endpoint))
        .await
        .map_err(|_| rpc_error(RpcErrorCode::PortConnectFailed, "tcp connect timed out"))?
        .map_err(|err| rpc_error(RpcErrorCode::PortConnectFailed, err.to_string()))
}

pub(super) async fn tunnel_tcp_listen(
    tunnel: Arc<TunnelState>,
    frame: Frame,
) -> Result<(), HostRpcError> {
    let session = tunnel_access(&tunnel)
        .await?
        .require_listen_session(TunnelForwardProtocol::Tcp, "tcp listen")?;
    let meta: EndpointMeta = decode_frame_meta(&frame)?;
    let endpoint = normalize_endpoint(&meta.endpoint)
        .map_err(|err| rpc_error(RpcErrorCode::InvalidEndpoint, err.to_string()))?;
    let listener = Arc::new(
        TcpListener::bind(&endpoint)
            .await
            .map_err(|err| rpc_error(RpcErrorCode::PortBindFailed, err.to_string()))?,
    );
    let bound_endpoint = listener
        .local_addr()
        .map_err(|err| rpc_error(RpcErrorCode::PortBindFailed, err.to_string()))?
        .to_string();
    session
        .replace_listener(
            frame.stream_id,
            listener.clone(),
            &tunnel.state.port_forward_limiter,
        )
        .await?;
    tunnel
        .send(endpoint_ok_frame(
            FrameType::TcpListenOk,
            frame.stream_id,
            bound_endpoint.clone(),
        )?)
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
                    RpcErrorCode::PortAcceptFailed,
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
                    err.code.as_str(),
                    err.message.clone(),
                )
                .await;
                tracing::debug!(
                    code = %err.code,
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
        attachment.tcp_streams.lock().await.insert(
            stream_id,
            TcpStreamEntry {
                writer: spawn_tcp_writer_task(writer, stream_cancel.clone()),
                _permit: stream_permit,
                cancel: Some(stream_cancel.clone()),
            },
        );
        if attachment
            .tx
            .send(
                match meta_frame(
                    FrameType::TcpAccept,
                    stream_id,
                    &TcpAcceptMeta {
                        listener_stream_id: listener_stream,
                        peer: peer.to_string(),
                    },
                ) {
                    Ok(frame) => frame,
                    Err(err) => {
                        cancel_tcp_stream(&attachment.tcp_streams, stream_id).await;
                        let _ = send_tunnel_error_code_with_sender(
                            &attachment.tx,
                            stream_id,
                            err.code.as_str(),
                            err.message,
                            false,
                        )
                        .await;
                        continue;
                    }
                },
            )
            .await
            .is_err()
        {
            cancel_tcp_stream(&attachment.tcp_streams, stream_id).await;
            return;
        }
        tokio::spawn(tunnel_tcp_read_loop_attached_session(
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
    tunnel_access(&tunnel)
        .await?
        .require_connect_tunnel(TunnelForwardProtocol::Tcp, "tcp connect")?;
    tunnel_tcp_connect_connection_local(tunnel, frame).await
}

pub(super) async fn tunnel_tcp_connect_connection_local(
    tunnel: Arc<TunnelState>,
    frame: Frame,
) -> Result<(), HostRpcError> {
    let meta: EndpointMeta = decode_frame_meta(&frame)?;
    let endpoint = ensure_nonzero_connect_endpoint(&meta.endpoint)
        .map_err(|err| rpc_error(RpcErrorCode::InvalidEndpoint, err.to_string()))?;
    let stream = tcp_connect_with_timeout(&tunnel, endpoint.as_str()).await?;
    let stream_permit = tunnel
        .state
        .port_forward_limiter
        .try_acquire_active_tcp_stream()?;
    let (reader, writer) = stream.into_split();
    let stream_cancel = tunnel.cancel.child_token();
    tunnel.tcp_streams.lock().await.insert(
        frame.stream_id,
        TcpStreamEntry {
            writer: spawn_tcp_writer_task(writer, stream_cancel.clone()),
            _permit: stream_permit,
            cancel: Some(stream_cancel.clone()),
        },
    );
    tunnel
        .send(empty_frame(FrameType::TcpConnectOk, frame.stream_id))
        .await?;
    tokio::spawn(tunnel_tcp_read_loop_connection_local(
        tunnel,
        frame.stream_id,
        reader,
        stream_cancel,
    ));
    Ok(())
}

pub(super) async fn tunnel_tcp_read_loop_connection_local(
    tunnel: Arc<TunnelState>,
    stream_id: u32,
    mut reader: OwnedReadHalf,
    cancel: CancellationToken,
) {
    tunnel_tcp_read_loop(
        TcpReadLoopTarget::Connection(tunnel),
        stream_id,
        &mut reader,
        cancel,
    )
    .await;
}

pub(super) async fn tunnel_tcp_read_loop_attached_session(
    attachment: Arc<AttachmentState>,
    stream_id: u32,
    mut reader: OwnedReadHalf,
    cancel: CancellationToken,
) {
    tunnel_tcp_read_loop(
        TcpReadLoopTarget::AttachedSession(attachment),
        stream_id,
        &mut reader,
        cancel,
    )
    .await;
}

async fn tunnel_tcp_read_loop(
    target: TcpReadLoopTarget,
    stream_id: u32,
    reader: &mut OwnedReadHalf,
    cancel: CancellationToken,
) {
    let mut buf = vec![0; READ_BUF_SIZE];
    loop {
        let read = tokio::select! {
            _ = cancel.cancelled() => {
                target.cleanup_stream(stream_id).await;
                return;
            }
            read = reader.read(&mut buf) => read,
        };
        match read {
            Ok(0) => {
                let _ = target
                    .send_frame(empty_frame(FrameType::TcpEof, stream_id))
                    .await;
                target.clear_cancel(stream_id).await;
                return;
            }
            Ok(read) => {
                if let Err(err) = target
                    .send_frame(data_frame(
                        FrameType::TcpData,
                        stream_id,
                        buf[..read].to_vec(),
                    ))
                    .await
                {
                    target
                        .send_error_code(stream_id, err.code.to_string(), err.message)
                        .await;
                    target.cancel_stream(stream_id).await;
                    return;
                }
            }
            Err(err) => {
                target.send_read_failed(stream_id, err.to_string()).await;
                target.cancel_stream(stream_id).await;
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
    let writer = match tunnel_access(tunnel)
        .await?
        .require_protocol(TunnelForwardProtocol::Tcp, "tcp data")?
    {
        OpenProtocolAccess::Listen(session) => {
            let attachment = session.current_attachment().await.ok_or_else(|| {
                rpc_error(
                    RpcErrorCode::PortTunnelClosed,
                    "port tunnel attachment is closed",
                )
            })?;
            attachment
                .tcp_streams
                .lock()
                .await
                .get(&stream_id)
                .map(|entry| entry.writer.clone())
                .ok_or_else(|| {
                    rpc_error(
                        RpcErrorCode::UnknownPortConnection,
                        format!("unknown tunnel tcp stream `{stream_id}`"),
                    )
                })?
        }
        OpenProtocolAccess::Connect => tunnel
            .tcp_streams
            .lock()
            .await
            .get(&stream_id)
            .map(|entry| entry.writer.clone())
            .ok_or_else(|| {
                rpc_error(
                    RpcErrorCode::UnknownPortConnection,
                    format!("unknown tunnel tcp stream `{stream_id}`"),
                )
            })?,
    };
    writer
        .tx
        .try_send(TcpWriteCommand::Data(data.to_vec()))
        .map_err(|err| match err {
            mpsc::error::TrySendError::Full(_) => {
                writer.cancel.cancel();
                rpc_error(
                    RpcErrorCode::PortTunnelLimitExceeded,
                    "tcp write queue limit reached",
                )
            }
            mpsc::error::TrySendError::Closed(_) => {
                rpc_error(RpcErrorCode::PortConnectionClosed, "connection was closed")
            }
        })
}

async fn cleanup_tcp_stream(tcp_streams: &TcpStreamMap, stream_id: u32) {
    let _ = tcp_streams.lock().await.remove(&stream_id);
}

async fn cancel_tcp_stream(tcp_streams: &TcpStreamMap, stream_id: u32) {
    if let Some(mut stream) = tcp_streams.lock().await.remove(&stream_id)
        && let Some(cancel) = stream.cancel.take()
    {
        cancel.cancel();
    }
}

async fn clear_tcp_cancel(tcp_streams: &TcpStreamMap, stream_id: u32) {
    if let Some(stream) = tcp_streams.lock().await.get_mut(&stream_id) {
        stream.cancel = None;
    }
}

async fn send_tcp_shutdown(writer: &TcpWriterHandle) -> bool {
    tokio::time::timeout(
        TCP_CONTROL_SEND_TIMEOUT,
        writer.tx.send(TcpWriteCommand::Shutdown),
    )
    .await
    .is_ok_and(|result| result.is_ok())
}

pub(super) async fn tunnel_tcp_eof(
    tunnel: &Arc<TunnelState>,
    stream_id: u32,
) -> Result<(), HostRpcError> {
    match tunnel_access(tunnel)
        .await?
        .protocol_access(TunnelForwardProtocol::Tcp)
    {
        Some(OpenProtocolAccess::Listen(session)) => {
            let Some(attachment) = session.current_attachment().await else {
                return Ok(());
            };
            let writer = {
                attachment
                    .tcp_streams
                    .lock()
                    .await
                    .get(&stream_id)
                    .map(|entry| entry.writer.clone())
            };
            if let Some(writer) = writer
                && send_tcp_shutdown(&writer).await
            {
                clear_tcp_cancel(&attachment.tcp_streams, stream_id).await;
            }
        }
        Some(OpenProtocolAccess::Connect) => {
            let writer = {
                tunnel
                    .tcp_streams
                    .lock()
                    .await
                    .get(&stream_id)
                    .map(|entry| entry.writer.clone())
            };
            if let Some(writer) = writer
                && send_tcp_shutdown(&writer).await
            {
                clear_tcp_cancel(&tunnel.tcp_streams, stream_id).await;
            }
        }
        None => {}
    }
    Ok(())
}

pub(super) async fn tunnel_close_stream(
    tunnel: &Arc<TunnelState>,
    stream_id: u32,
) -> Result<(), HostRpcError> {
    match tunnel_access(tunnel).await?.role_access() {
        OpenTunnelRole::Listen(session) => {
            if let Some(attachment) = session.current_attachment().await {
                cancel_tcp_stream(&attachment.tcp_streams, stream_id).await;
                if let Some(reader) = attachment.udp_readers.lock().await.remove(&stream_id) {
                    reader.cancel.cancel();
                }
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
        OpenTunnelRole::Connect => {
            cancel_tcp_stream(&tunnel.tcp_streams, stream_id).await;
            if let Some(bind) = tunnel.udp_binds.lock().await.remove(&stream_id) {
                bind.cancel.cancel();
            }
        }
        OpenTunnelRole::Unopened => {}
    }
    tunnel
        .send(empty_frame(FrameType::Close, stream_id))
        .await?;
    Ok(())
}
