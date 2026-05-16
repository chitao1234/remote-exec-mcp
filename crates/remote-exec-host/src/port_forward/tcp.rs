use std::net::SocketAddr;
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

use super::active::{
    ActiveTunnelRole, ConnectContext, ListenContext, active_access, send_tunnel_error,
    send_tunnel_error_code,
};
use super::codec::decode_frame_meta;
use super::error::{SessionCloseMode, operational_error, request_error};
use super::frames::{data_frame, empty_frame, endpoint_ok_frame, meta_frame};
use super::session::{
    AttachmentState, SessionState, close_attached_session, listener_stream_id, udp_bind_stream_id,
    wait_for_session_attachment,
};
use super::{
    PortForwardPermit, READ_BUF_SIZE, TCP_WRITE_QUEUE_FRAMES, TcpStreamEntry, TcpWriteCommand,
    TcpWriterHandle, TunnelState, send_forward_drop_report,
};

const TCP_CONTROL_SEND_TIMEOUT: Duration = Duration::from_secs(1);
type TcpStreamMap = tokio::sync::Mutex<std::collections::HashMap<u32, TcpStreamEntry>>;

enum AcceptLoopOutcome {
    Continue,
    Return,
    Accepted {
        attachment: Arc<AttachmentState>,
        stream: TcpStream,
        peer: SocketAddr,
    },
}

enum RegisteredAcceptedTcpStream {
    Continue,
    Return,
    Spawn {
        stream_id: u32,
        reader: OwnedReadHalf,
        stream_cancel: CancellationToken,
    },
}

trait TcpReadLoopContext {
    fn tx(&self) -> &super::TunnelSender;
    fn generation(&self) -> u64;
    fn tcp_streams(&self) -> &TcpStreamMap;
}

impl TcpReadLoopContext for ConnectContext {
    fn tx(&self) -> &super::TunnelSender {
        ConnectContext::tx(self)
    }

    fn generation(&self) -> u64 {
        ConnectContext::generation(self)
    }

    fn tcp_streams(&self) -> &TcpStreamMap {
        ConnectContext::tcp_streams(self)
    }
}

impl TcpReadLoopContext for ListenContext {
    fn tx(&self) -> &super::TunnelSender {
        ListenContext::tx(self)
    }

    fn generation(&self) -> u64 {
        ListenContext::generation(self)
    }

    fn tcp_streams(&self) -> &TcpStreamMap {
        ListenContext::tcp_streams(self)
    }
}

async fn send_tcp_read_frame<T: TcpReadLoopContext>(
    target: &T,
    frame: Frame,
) -> Result<(), HostRpcError> {
    target.tx().send(frame).await
}

async fn send_tcp_read_error_code<T: TcpReadLoopContext>(
    target: &T,
    stream_id: u32,
    code: String,
    message: String,
) {
    let _ = send_tunnel_error_code(
        target.tx(),
        Some(target.generation()),
        stream_id,
        code,
        message,
        false,
    )
    .await;
}

async fn send_tcp_read_failed<T: TcpReadLoopContext>(target: &T, stream_id: u32, message: String) {
    let _ = send_tunnel_error(
        target.tx(),
        Some(target.generation()),
        stream_id,
        RpcErrorCode::PortReadFailed,
        message,
        false,
    )
    .await;
}

async fn cleanup_tcp_read_stream<T: TcpReadLoopContext>(target: &T, stream_id: u32) {
    cleanup_tcp_stream(target.tcp_streams(), stream_id).await;
}

async fn cancel_tcp_read_stream<T: TcpReadLoopContext>(target: &T, stream_id: u32) {
    cancel_tcp_stream(target.tcp_streams(), stream_id).await;
}

async fn clear_tcp_read_cancel<T: TcpReadLoopContext>(target: &T, stream_id: u32) {
    clear_tcp_cancel(target.tcp_streams(), stream_id).await;
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
        .map_err(|_| operational_error(RpcErrorCode::PortConnectFailed, "tcp connect timed out"))?
        .map_err(|err| operational_error(RpcErrorCode::PortConnectFailed, err.to_string()))
}

pub(super) async fn tunnel_tcp_listen(
    tunnel: Arc<TunnelState>,
    frame: Frame,
) -> Result<(), HostRpcError> {
    let listen = active_access(&tunnel)
        .await?
        .require_listen_session(TunnelForwardProtocol::Tcp, "tcp listen")?;
    let meta: EndpointMeta = decode_frame_meta(&frame)?;
    let endpoint = normalize_endpoint(&meta.endpoint)
        .map_err(|err| request_error(RpcErrorCode::InvalidEndpoint, err.to_string()))?;
    let listener = Arc::new(
        TcpListener::bind(&endpoint)
            .await
            .map_err(|err| operational_error(RpcErrorCode::PortBindFailed, err.to_string()))?,
    );
    let bound_endpoint = listener
        .local_addr()
        .map_err(|err| operational_error(RpcErrorCode::PortBindFailed, err.to_string()))?
        .to_string();
    listen
        .session()
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
    tokio::spawn(tunnel_tcp_accept_loop(listen.session().clone(), listener));
    Ok(())
}

pub(super) async fn tunnel_tcp_accept_loop(session: Arc<SessionState>, listener: Arc<TcpListener>) {
    loop {
        let (attachment, stream, peer) = match accept_attached_tcp_stream(&session, &listener).await
        {
            AcceptLoopOutcome::Continue => continue,
            AcceptLoopOutcome::Return => return,
            AcceptLoopOutcome::Accepted {
                attachment,
                stream,
                peer,
            } => (attachment, stream, peer),
        };
        let Some(stream_permit) =
            acquire_active_tcp_stream_permit(&session, &attachment, &stream).await
        else {
            continue;
        };
        let registered =
            register_accepted_tcp_stream(&session, &attachment, stream_permit, stream, peer).await;
        let RegisteredAcceptedTcpStream::Spawn {
            stream_id,
            reader,
            stream_cancel,
        } = registered
        else {
            if matches!(registered, RegisteredAcceptedTcpStream::Return) {
                return;
            }
            continue;
        };
        tokio::spawn(tunnel_tcp_read_loop_attached_session(
            session.clone(),
            attachment,
            stream_id,
            reader,
            stream_cancel,
        ));
    }
}

async fn accept_attached_tcp_stream(
    session: &Arc<SessionState>,
    listener: &Arc<TcpListener>,
) -> AcceptLoopOutcome {
    let Some(attachment) = wait_for_session_attachment(session).await else {
        return AcceptLoopOutcome::Return;
    };
    let accepted = tokio::select! {
        _ = session.root_cancel.cancelled() => return AcceptLoopOutcome::Return,
        _ = attachment.cancel.cancelled() => return AcceptLoopOutcome::Continue,
        accepted = listener.accept() => accepted,
    };
    let (stream, peer) = match accepted {
        Ok(accepted) => accepted,
        Err(err) => {
            let _ = send_tunnel_error(
                &attachment.tx,
                Some(session.generation()),
                listener_stream_id(session).await.unwrap_or(0),
                RpcErrorCode::PortAcceptFailed,
                err.to_string(),
                false,
            )
            .await;
            return AcceptLoopOutcome::Return;
        }
    };
    if attachment.cancel.is_cancelled() {
        drop(stream);
        return AcceptLoopOutcome::Continue;
    }
    AcceptLoopOutcome::Accepted {
        attachment,
        stream,
        peer,
    }
}

async fn acquire_active_tcp_stream_permit(
    session: &Arc<SessionState>,
    attachment: &Arc<AttachmentState>,
    stream: &TcpStream,
) -> Option<PortForwardPermit> {
    match attachment.tx.limiter.try_acquire_active_tcp_stream() {
        Ok(permit) => Some(permit),
        Err(err) => {
            let _ = stream;
            let _ = send_forward_drop_report(
                &attachment.tx,
                listener_stream_id(session).await.unwrap_or(0),
                ForwardDropKind::TcpStream,
                err.wire_code(),
                err.message.clone(),
            )
            .await;
            tracing::debug!(
                code = err.wire_code(),
                message = %err.message,
                "dropping accepted tcp stream due to local port tunnel pressure"
            );
            None
        }
    }
}

async fn register_accepted_tcp_stream(
    session: &Arc<SessionState>,
    attachment: &Arc<AttachmentState>,
    stream_permit: PortForwardPermit,
    stream: TcpStream,
    peer: SocketAddr,
) -> RegisteredAcceptedTcpStream {
    let stream_id = session
        .next_daemon_stream_id
        .fetch_add(2, Ordering::Relaxed);
    let listener_stream = listener_stream_id(session).await.unwrap_or(0);
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
    let accept_frame = match meta_frame(
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
            let _ = send_tunnel_error_code(
                &attachment.tx,
                Some(session.generation()),
                stream_id,
                err.wire_code(),
                err.message,
                false,
            )
            .await;
            return RegisteredAcceptedTcpStream::Continue;
        }
    };
    if attachment.tx.send(accept_frame).await.is_err() {
        cancel_tcp_stream(&attachment.tcp_streams, stream_id).await;
        return RegisteredAcceptedTcpStream::Return;
    }
    RegisteredAcceptedTcpStream::Spawn {
        stream_id,
        reader,
        stream_cancel,
    }
}

pub(super) async fn tunnel_tcp_connect(
    tunnel: Arc<TunnelState>,
    frame: Frame,
) -> Result<(), HostRpcError> {
    let connect = active_access(&tunnel)
        .await?
        .require_connect_tunnel(TunnelForwardProtocol::Tcp, "tcp connect")?;
    tunnel_tcp_connect_connection_local(tunnel, connect, frame).await
}

pub(super) async fn tunnel_tcp_connect_connection_local(
    tunnel: Arc<TunnelState>,
    connect: ConnectContext,
    frame: Frame,
) -> Result<(), HostRpcError> {
    let meta: EndpointMeta = decode_frame_meta(&frame)?;
    let endpoint = ensure_nonzero_connect_endpoint(&meta.endpoint)
        .map_err(|err| request_error(RpcErrorCode::InvalidEndpoint, err.to_string()))?;
    let stream = tcp_connect_with_timeout(&tunnel, endpoint.as_str()).await?;
    let stream_permit = tunnel
        .state
        .port_forward_limiter
        .try_acquire_active_tcp_stream()?;
    let (reader, writer) = stream.into_split();
    let stream_cancel = connect.cancel().child_token();
    connect.tcp_streams().lock().await.insert(
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
        connect,
        frame.stream_id,
        reader,
        stream_cancel,
    ));
    Ok(())
}

pub(super) async fn tunnel_tcp_read_loop_connection_local(
    connect: ConnectContext,
    stream_id: u32,
    mut reader: OwnedReadHalf,
    cancel: CancellationToken,
) {
    tunnel_tcp_read_loop(connect, stream_id, &mut reader, cancel).await;
}

pub(super) async fn tunnel_tcp_read_loop_attached_session(
    session: Arc<SessionState>,
    attachment: Arc<AttachmentState>,
    stream_id: u32,
    mut reader: OwnedReadHalf,
    cancel: CancellationToken,
) {
    tunnel_tcp_read_loop(
        ListenContext::new(session, attachment),
        stream_id,
        &mut reader,
        cancel,
    )
    .await;
}

async fn tunnel_tcp_read_loop<T: TcpReadLoopContext>(
    target: T,
    stream_id: u32,
    reader: &mut OwnedReadHalf,
    cancel: CancellationToken,
) {
    let mut buf = vec![0; READ_BUF_SIZE];
    loop {
        let read = tokio::select! {
            _ = cancel.cancelled() => {
                cleanup_tcp_read_stream(&target, stream_id).await;
                return;
            }
            read = reader.read(&mut buf) => read,
        };
        match read {
            Ok(0) => {
                let _ =
                    send_tcp_read_frame(&target, empty_frame(FrameType::TcpEof, stream_id)).await;
                clear_tcp_read_cancel(&target, stream_id).await;
                return;
            }
            Ok(read) => {
                if let Err(err) = send_tcp_read_frame(
                    &target,
                    data_frame(FrameType::TcpData, stream_id, buf[..read].to_vec()),
                )
                .await
                {
                    send_tcp_read_error_code(
                        &target,
                        stream_id,
                        err.wire_code().to_string(),
                        err.message,
                    )
                    .await;
                    cancel_tcp_read_stream(&target, stream_id).await;
                    return;
                }
            }
            Err(err) => {
                send_tcp_read_failed(&target, stream_id, err.to_string()).await;
                cancel_tcp_read_stream(&target, stream_id).await;
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
    let access = active_access(tunnel)
        .await?
        .require_protocol(TunnelForwardProtocol::Tcp, "tcp data")?;
    let writer = lookup_tcp_writer(access.tcp_streams(), stream_id).await?;
    writer
        .tx
        .try_send(TcpWriteCommand::Data(data.to_vec()))
        .map_err(|err| match err {
            mpsc::error::TrySendError::Full(_) => {
                writer.cancel.cancel();
                request_error(
                    RpcErrorCode::PortTunnelLimitExceeded,
                    "tcp write queue limit reached",
                )
            }
            mpsc::error::TrySendError::Closed(_) => {
                operational_error(RpcErrorCode::PortConnectionClosed, "connection was closed")
            }
        })
}

async fn cleanup_tcp_stream(tcp_streams: &TcpStreamMap, stream_id: u32) {
    let _ = tcp_streams.lock().await.remove(&stream_id);
}

async fn cancel_tcp_stream(tcp_streams: &TcpStreamMap, stream_id: u32) {
    if let Some(mut stream) = tcp_streams.lock().await.remove(&stream_id) {
        if let Some(cancel) = stream.cancel.take() {
            cancel.cancel();
        }
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
    if let Some(access) = active_access(tunnel)
        .await?
        .protocol_access_if(TunnelForwardProtocol::Tcp)
    {
        let tcp_streams = access.tcp_streams();
        if let Some(writer) = optional_tcp_writer(tcp_streams, stream_id).await {
            if send_tcp_shutdown(&writer).await {
                clear_tcp_cancel(tcp_streams, stream_id).await;
            }
        }
    }
    Ok(())
}

async fn lookup_tcp_writer(
    tcp_streams: &TcpStreamMap,
    stream_id: u32,
) -> Result<TcpWriterHandle, HostRpcError> {
    optional_tcp_writer(tcp_streams, stream_id)
        .await
        .ok_or_else(|| {
            request_error(
                RpcErrorCode::UnknownPortConnection,
                format!("unknown tunnel tcp stream `{stream_id}`"),
            )
        })
}

async fn optional_tcp_writer(
    tcp_streams: &TcpStreamMap,
    stream_id: u32,
) -> Option<TcpWriterHandle> {
    tcp_streams
        .lock()
        .await
        .get(&stream_id)
        .map(|entry| entry.writer.clone())
}

pub(super) async fn tunnel_close_stream(
    tunnel: &Arc<TunnelState>,
    stream_id: u32,
) -> Result<(), HostRpcError> {
    match active_access(tunnel).await?.role_access() {
        ActiveTunnelRole::Listen(listen) => {
            cancel_tcp_stream(listen.tcp_streams(), stream_id).await;
            if let Some(reader) = listen.udp_readers().lock().await.remove(&stream_id) {
                reader.cancel.cancel();
            }
            if let Some(bind_stream_id) = udp_bind_stream_id(listen.session()).await {
                if bind_stream_id == stream_id {
                    close_attached_session(tunnel, SessionCloseMode::GracefulClose).await;
                }
            }
            if let Some(listener_stream) = listener_stream_id(listen.session()).await {
                if listener_stream == stream_id {
                    close_attached_session(tunnel, SessionCloseMode::GracefulClose).await;
                }
            }
        }
        ActiveTunnelRole::Connect(connect) => {
            cancel_tcp_stream(connect.tcp_streams(), stream_id).await;
            if let Some(bind) = connect.udp_binds().lock().await.remove(&stream_id) {
                bind.cancel.cancel();
            }
        }
        ActiveTunnelRole::Unopened => {}
    }
    tunnel
        .send(empty_frame(FrameType::Close, stream_id))
        .await?;
    Ok(())
}
