use std::collections::HashMap;
use std::io::ErrorKind;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use remote_exec_proto::port_forward::{ensure_nonzero_connect_endpoint, normalize_endpoint};
use remote_exec_proto::port_tunnel::{Frame, FrameType, read_frame, read_preface, write_frame};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

use crate::{AppState, HostRpcError};

const READ_BUF_SIZE: usize = 64 * 1024;
#[cfg(not(test))]
const RESUME_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(test)]
const RESUME_TIMEOUT: Duration = Duration::from_millis(100);

#[derive(Clone, Default)]
pub struct TunnelSessionStore {
    sessions: Arc<Mutex<HashMap<String, Arc<SessionState>>>>,
}

struct TunnelState {
    state: Arc<AppState>,
    cancel: CancellationToken,
    tx: mpsc::Sender<Frame>,
    tcp_writers: Mutex<HashMap<u32, Arc<Mutex<OwnedWriteHalf>>>>,
    udp_sockets: Mutex<HashMap<u32, Arc<UdpSocket>>>,
    stream_cancels: Mutex<HashMap<u32, CancellationToken>>,
    next_daemon_stream_id: AtomicU32,
    attached_session: Mutex<Option<Arc<SessionState>>>,
}

struct SessionState {
    id: String,
    root_cancel: CancellationToken,
    attachment: Mutex<Option<Arc<AttachmentState>>>,
    attachment_notify: tokio::sync::Notify,
    resume_deadline: Mutex<Option<Instant>>,
    retained_listener: Mutex<Option<RetainedListener>>,
    retained_udp_bind: Mutex<Option<RetainedUdpBind>>,
    next_daemon_stream_id: AtomicU32,
}

struct AttachmentState {
    tx: mpsc::Sender<Frame>,
    cancel: CancellationToken,
    tcp_writers: Mutex<HashMap<u32, Arc<Mutex<OwnedWriteHalf>>>>,
    stream_cancels: Mutex<HashMap<u32, CancellationToken>>,
}

enum RetainedListener {
    Tcp {
        stream_id: u32,
        _listener: Arc<TcpListener>,
    },
}

enum RetainedUdpBind {
    Udp {
        stream_id: u32,
        socket: Arc<UdpSocket>,
    },
}

#[derive(Debug, Deserialize)]
struct EndpointMeta {
    endpoint: String,
}

#[derive(Debug, Serialize)]
struct EndpointOkMeta {
    endpoint: String,
}

#[derive(Debug, Serialize)]
struct TcpAcceptMeta {
    listener_stream_id: u32,
    peer: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct UdpDatagramMeta {
    peer: String,
}

#[derive(Debug, Serialize)]
struct ErrorMeta {
    code: String,
    message: String,
    fatal: bool,
}

#[derive(Debug, Deserialize)]
struct SessionResumeMeta {
    session_id: String,
}

#[derive(Debug, Serialize)]
struct SessionReadyMeta {
    session_id: String,
    resume_timeout_ms: u64,
}

enum TunnelMode {
    Transport,
    Session(Arc<SessionState>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SessionCloseMode {
    GracefulClose,
    RetryableDetach,
    TerminalFailure,
}

pub async fn serve_tunnel<S>(state: Arc<AppState>, stream: S) -> Result<(), HostRpcError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut reader, mut writer) = tokio::io::split(stream);
    read_preface(&mut reader)
        .await
        .map_err(|err| rpc_error("invalid_port_tunnel", err.to_string()))?;

    let (tx, mut rx) = mpsc::channel::<Frame>(128);
    let tunnel = Arc::new(TunnelState {
        state: state.clone(),
        cancel: state.shutdown.child_token(),
        tx: tx.clone(),
        tcp_writers: Mutex::new(HashMap::new()),
        udp_sockets: Mutex::new(HashMap::new()),
        stream_cancels: Mutex::new(HashMap::new()),
        next_daemon_stream_id: AtomicU32::new(2),
        attached_session: Mutex::new(None),
    });
    let writer_cancel = tunnel.cancel.clone();
    let writer_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                frame = rx.recv() => {
                    let Some(frame) = frame else {
                        return;
                    };
                    if write_frame(&mut writer, &frame).await.is_err() {
                        writer_cancel.cancel();
                        return;
                    }
                }
                _ = writer_cancel.cancelled() => {
                    while let Ok(frame) = rx.try_recv() {
                        if write_frame(&mut writer, &frame).await.is_err() {
                            return;
                        }
                    }
                    return;
                }
            }
        }
    });

    let result = tunnel_read_loop(tunnel.clone(), &mut reader).await;
    close_attached_session(&tunnel, close_mode_for_tunnel_result(&result)).await;
    tunnel.cancel.cancel();
    drop(tx);
    let _ = tokio::time::timeout(Duration::from_millis(100), writer_task).await;
    result
}

async fn tunnel_read_loop<R>(tunnel: Arc<TunnelState>, reader: &mut R) -> Result<(), HostRpcError>
where
    R: AsyncRead + Unpin,
{
    loop {
        let frame = tokio::select! {
            _ = tunnel.cancel.cancelled() => return Ok(()),
            frame = read_frame(reader) => {
                match frame {
                    Ok(frame) => frame,
                    Err(err) if err.kind() == ErrorKind::UnexpectedEof => return Ok(()),
                    Err(err) => {
                        let _ = send_tunnel_error(&tunnel, 0, "invalid_port_tunnel", err.to_string(), true)
                            .await;
                        return Err(rpc_error("invalid_port_tunnel", err.to_string()));
                    }
                }
            }
        };

        let stream_id = frame.stream_id;
        if let Err(err) = handle_tunnel_frame(tunnel.clone(), frame).await {
            let _ =
                send_tunnel_error(&tunnel, stream_id, err.code, err.message.clone(), false).await;
        }
    }
}

async fn handle_tunnel_frame(tunnel: Arc<TunnelState>, frame: Frame) -> Result<(), HostRpcError> {
    match frame.frame_type {
        FrameType::SessionOpen => tunnel_session_open(tunnel, frame).await,
        FrameType::SessionResume => tunnel_session_resume(tunnel, frame).await,
        FrameType::TcpListen => tunnel_tcp_listen(tunnel, frame).await,
        FrameType::TcpConnect => tunnel_tcp_connect(tunnel, frame).await,
        FrameType::TcpData => tunnel_tcp_data(&tunnel, frame.stream_id, &frame.data).await,
        FrameType::TcpEof => tunnel_tcp_eof(&tunnel, frame.stream_id).await,
        FrameType::Close => tunnel_close_stream(&tunnel, frame.stream_id).await,
        FrameType::UdpBind => tunnel_udp_bind(tunnel, frame).await,
        FrameType::UdpDatagram => tunnel_udp_datagram(&tunnel, frame).await,
        _ => Err(rpc_error(
            "invalid_port_tunnel",
            format!("unexpected frame type `{:?}` from broker", frame.frame_type),
        )),
    }
}

async fn tunnel_session_open(tunnel: Arc<TunnelState>, frame: Frame) -> Result<(), HostRpcError> {
    if frame.stream_id != 0 {
        return Err(rpc_error(
            "invalid_port_tunnel",
            "session open must use stream_id 0",
        ));
    }
    let session = Arc::new(SessionState {
        id: format!("sess_{}", uuid::Uuid::new_v4().simple()),
        root_cancel: tunnel.state.shutdown.child_token(),
        attachment: Mutex::new(None),
        attachment_notify: tokio::sync::Notify::new(),
        resume_deadline: Mutex::new(None),
        retained_listener: Mutex::new(None),
        retained_udp_bind: Mutex::new(None),
        next_daemon_stream_id: AtomicU32::new(2),
    });
    attach_session_to_tunnel(&session, &tunnel).await?;
    tunnel
        .state
        .port_forward_sessions
        .insert(session.clone())
        .await;
    tunnel
        .send(Frame {
            frame_type: FrameType::SessionReady,
            flags: 0,
            stream_id: 0,
            meta: encode_frame_meta(&SessionReadyMeta {
                session_id: session.id.clone(),
                resume_timeout_ms: RESUME_TIMEOUT.as_millis() as u64,
            })?,
            data: Vec::new(),
        })
        .await
}

async fn tunnel_session_resume(tunnel: Arc<TunnelState>, frame: Frame) -> Result<(), HostRpcError> {
    if frame.stream_id != 0 {
        return Err(rpc_error(
            "invalid_port_tunnel",
            "session resume must use stream_id 0",
        ));
    }
    let meta: SessionResumeMeta = decode_frame_meta(&frame)?;
    let session = tunnel
        .state
        .port_forward_sessions
        .get(&meta.session_id)
        .await
        .ok_or_else(|| rpc_error("unknown_port_tunnel_session", "unknown port tunnel session"))?;
    if session.is_expired().await {
        tunnel
            .state
            .port_forward_sessions
            .remove(&meta.session_id)
            .await;
        session.close_retained_resources().await;
        return Err(rpc_error(
            "port_tunnel_resume_expired",
            "port tunnel resume expired",
        ));
    }
    attach_session_to_tunnel(&session, &tunnel).await?;
    tunnel
        .send(Frame {
            frame_type: FrameType::SessionResumed,
            flags: 0,
            stream_id: 0,
            meta: Vec::new(),
            data: Vec::new(),
        })
        .await?;
    reactivate_retained_udp_bind(&session).await
}

async fn tunnel_tcp_listen(tunnel: Arc<TunnelState>, frame: Frame) -> Result<(), HostRpcError> {
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

async fn tunnel_tcp_listen_transport_owned(
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

async fn tunnel_tcp_accept_loop(session: Arc<SessionState>, listener: Arc<TcpListener>) {
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
        let listener_stream_id = listener_stream_id(&session).await.unwrap_or(0);
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
                    listener_stream_id,
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

async fn tunnel_tcp_accept_loop_transport_owned(
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

async fn tunnel_tcp_connect(tunnel: Arc<TunnelState>, frame: Frame) -> Result<(), HostRpcError> {
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

async fn tunnel_tcp_connect_transport_owned(
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

async fn tunnel_tcp_read_loop_transport_owned(
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

async fn tunnel_tcp_read_loop_session_owned(
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

async fn tunnel_tcp_data(
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

async fn tunnel_tcp_eof(tunnel: &Arc<TunnelState>, stream_id: u32) -> Result<(), HostRpcError> {
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

async fn tunnel_close_stream(
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
            if let Some(listener_stream_id) = listener_stream_id(&session).await {
                if listener_stream_id == stream_id {
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

async fn tunnel_udp_bind(tunnel: Arc<TunnelState>, frame: Frame) -> Result<(), HostRpcError> {
    let TunnelMode::Session(session) = tunnel_mode(&tunnel).await else {
        return tunnel_udp_bind_transport_owned(tunnel, frame).await;
    };
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
        .replace_udp_bind(frame.stream_id, socket.clone())
        .await;
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

async fn tunnel_udp_bind_transport_owned(
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
    tunnel
        .udp_sockets
        .lock()
        .await
        .insert(frame.stream_id, socket.clone());
    let stream_cancel = tunnel.cancel.child_token();
    tunnel
        .stream_cancels
        .lock()
        .await
        .insert(frame.stream_id, stream_cancel.clone());
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

async fn tunnel_udp_read_loop_transport_owned(
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
        if tunnel
            .send(Frame {
                frame_type: FrameType::UdpDatagram,
                flags: 0,
                stream_id,
                meta,
                data: buf[..read].to_vec(),
            })
            .await
            .is_err()
        {
            return;
        }
    }
}

async fn tunnel_udp_read_loop_session_owned(
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
        if attachment
            .tx
            .send(Frame {
                frame_type: FrameType::UdpDatagram,
                flags: 0,
                stream_id,
                meta,
                data: buf[..read].to_vec(),
            })
            .await
            .is_err()
        {
            return;
        }
    }
}

async fn tunnel_udp_datagram(tunnel: &Arc<TunnelState>, frame: Frame) -> Result<(), HostRpcError> {
    let meta: UdpDatagramMeta = decode_frame_meta(&frame)?;
    let socket = match tunnel_mode(tunnel).await {
        TunnelMode::Session(session) => {
            session.udp_socket(frame.stream_id).await.ok_or_else(|| {
                rpc_error(
                    "unknown_port_bind",
                    format!("unknown tunnel udp stream `{}`", frame.stream_id),
                )
            })?
        }
        TunnelMode::Transport => tunnel
            .udp_sockets
            .lock()
            .await
            .get(&frame.stream_id)
            .cloned()
            .ok_or_else(|| {
                rpc_error(
                    "unknown_port_bind",
                    format!("unknown tunnel udp stream `{}`", frame.stream_id),
                )
            })?,
    };
    socket
        .send_to(&frame.data, &meta.peer)
        .await
        .map_err(|err| rpc_error("port_write_failed", err.to_string()))?;
    Ok(())
}

impl TunnelState {
    async fn send(&self, frame: Frame) -> Result<(), HostRpcError> {
        self.tx
            .send(frame)
            .await
            .map_err(|_| rpc_error("port_tunnel_closed", "port tunnel writer is closed"))
    }
}

impl TunnelSessionStore {
    async fn insert(&self, session: Arc<SessionState>) {
        self.sessions
            .lock()
            .await
            .insert(session.id.clone(), session);
    }

    async fn get(&self, session_id: &str) -> Option<Arc<SessionState>> {
        self.sessions.lock().await.get(session_id).cloned()
    }

    async fn remove(&self, session_id: &str) -> Option<Arc<SessionState>> {
        self.sessions.lock().await.remove(session_id)
    }
}

impl SessionState {
    async fn current_attachment(&self) -> Option<Arc<AttachmentState>> {
        self.attachment.lock().await.clone()
    }

    async fn is_expired(&self) -> bool {
        self.resume_deadline
            .lock()
            .await
            .as_ref()
            .is_some_and(|deadline| Instant::now() >= *deadline)
    }

    async fn replace_listener(&self, stream_id: u32, listener: Arc<TcpListener>) {
        *self.retained_listener.lock().await = Some(RetainedListener::Tcp {
            stream_id,
            _listener: listener,
        });
    }

    async fn replace_udp_bind(&self, stream_id: u32, socket: Arc<UdpSocket>) {
        *self.retained_udp_bind.lock().await = Some(RetainedUdpBind::Udp { stream_id, socket });
    }

    async fn udp_socket(&self, stream_id: u32) -> Option<Arc<UdpSocket>> {
        match self.retained_udp_bind.lock().await.as_ref() {
            Some(RetainedUdpBind::Udp {
                stream_id: retained_stream_id,
                socket,
            }) if *retained_stream_id == stream_id => Some(socket.clone()),
            _ => None,
        }
    }

    async fn udp_bind_snapshot(&self) -> Option<(u32, Arc<UdpSocket>)> {
        self.retained_udp_bind
            .lock()
            .await
            .as_ref()
            .map(|RetainedUdpBind::Udp { stream_id, socket }| (*stream_id, socket.clone()))
    }

    async fn close_non_resumable_streams(&self) {
        if let Some(attachment) = self.attachment.lock().await.clone() {
            for (_, cancel) in attachment.stream_cancels.lock().await.drain() {
                cancel.cancel();
            }
            attachment.tcp_writers.lock().await.clear();
        }
    }

    async fn close_retained_resources(&self) {
        *self.retained_listener.lock().await = None;
        *self.retained_udp_bind.lock().await = None;
        self.close_non_resumable_streams().await;
    }
}

async fn explicit_session(tunnel: &Arc<TunnelState>) -> Option<Arc<SessionState>> {
    tunnel.attached_session.lock().await.clone()
}

async fn tunnel_mode(tunnel: &Arc<TunnelState>) -> TunnelMode {
    match explicit_session(tunnel).await {
        Some(session) => TunnelMode::Session(session),
        None => TunnelMode::Transport,
    }
}

async fn attach_session_to_tunnel(
    session: &Arc<SessionState>,
    tunnel: &Arc<TunnelState>,
) -> Result<(), HostRpcError> {
    {
        let mut attachment = session.attachment.lock().await;
        if let Some(existing) = attachment.as_ref() {
            if !existing.cancel.is_cancelled() {
                existing.cancel.cancel();
            }
        }
        *attachment = Some(Arc::new(AttachmentState {
            tx: tunnel.tx.clone(),
            cancel: tunnel.cancel.child_token(),
            tcp_writers: Mutex::new(HashMap::new()),
            stream_cancels: Mutex::new(HashMap::new()),
        }));
    }
    *session.resume_deadline.lock().await = None;
    *tunnel.attached_session.lock().await = Some(session.clone());
    session.attachment_notify.notify_waiters();
    Ok(())
}

async fn close_attached_session(tunnel: &Arc<TunnelState>, mode: SessionCloseMode) {
    let Some(session) = tunnel.attached_session.lock().await.take() else {
        return;
    };
    if let Some(attachment) = session.attachment.lock().await.take() {
        attachment.cancel.cancel();
        for (_, cancel) in attachment.stream_cancels.lock().await.drain() {
            cancel.cancel();
        }
        attachment.tcp_writers.lock().await.clear();
    }

    match mode {
        SessionCloseMode::RetryableDetach => {
            *session.resume_deadline.lock().await = Some(Instant::now() + RESUME_TIMEOUT);
            schedule_session_expiry(tunnel.state.port_forward_sessions.clone(), session);
        }
        SessionCloseMode::GracefulClose | SessionCloseMode::TerminalFailure => {
            *session.resume_deadline.lock().await = None;
            tunnel.state.port_forward_sessions.remove(&session.id).await;
            session.close_retained_resources().await;
            session.root_cancel.cancel();
        }
    }
}

fn close_mode_for_tunnel_result(result: &Result<(), HostRpcError>) -> SessionCloseMode {
    match result {
        Ok(()) => SessionCloseMode::RetryableDetach,
        Err(_) => SessionCloseMode::TerminalFailure,
    }
}

async fn listener_stream_id(session: &Arc<SessionState>) -> Option<u32> {
    session
        .retained_listener
        .lock()
        .await
        .as_ref()
        .map(|RetainedListener::Tcp { stream_id, .. }| *stream_id)
}

async fn wait_for_session_attachment(session: &Arc<SessionState>) -> Option<Arc<AttachmentState>> {
    loop {
        if let Some(attachment) = session.current_attachment().await {
            return Some(attachment);
        }
        tokio::select! {
            _ = session.root_cancel.cancelled() => return None,
            _ = session.attachment_notify.notified() => {}
        }
    }
}

async fn udp_bind_stream_id(session: &Arc<SessionState>) -> Option<u32> {
    session
        .retained_udp_bind
        .lock()
        .await
        .as_ref()
        .map(|RetainedUdpBind::Udp { stream_id, .. }| *stream_id)
}

fn schedule_session_expiry(store: TunnelSessionStore, session: Arc<SessionState>) {
    tokio::spawn(async move {
        tokio::time::sleep(RESUME_TIMEOUT).await;
        if session.is_expired().await && session.current_attachment().await.is_none() {
            store.remove(&session.id).await;
            session.close_retained_resources().await;
            session.root_cancel.cancel();
        }
    });
}

async fn reactivate_retained_udp_bind(session: &Arc<SessionState>) -> Result<(), HostRpcError> {
    let Some((stream_id, socket)) = session.udp_bind_snapshot().await else {
        return Ok(());
    };
    let attachment = session
        .current_attachment()
        .await
        .ok_or_else(|| rpc_error("port_tunnel_closed", "port tunnel attachment is closed"))?;
    let stream_cancel = attachment.cancel.child_token();
    if let Some(existing) = attachment
        .stream_cancels
        .lock()
        .await
        .insert(stream_id, stream_cancel.clone())
    {
        existing.cancel();
    }
    tokio::spawn(tunnel_udp_read_loop_session_owned(
        attachment,
        stream_id,
        socket,
        stream_cancel,
    ));
    Ok(())
}

async fn send_tunnel_error_with_sender(
    tx: &mpsc::Sender<Frame>,
    stream_id: u32,
    code: impl Into<String>,
    message: impl Into<String>,
    fatal: bool,
) -> Result<(), HostRpcError> {
    let meta = encode_frame_meta(&ErrorMeta {
        code: code.into(),
        message: message.into(),
        fatal,
    })?;
    tx.send(Frame {
        frame_type: FrameType::Error,
        flags: 0,
        stream_id,
        meta,
        data: Vec::new(),
    })
    .await
    .map_err(|_| rpc_error("port_tunnel_closed", "port tunnel writer is closed"))
}

async fn send_tunnel_error(
    tunnel: &TunnelState,
    stream_id: u32,
    code: impl Into<String>,
    message: impl Into<String>,
    fatal: bool,
) -> Result<(), HostRpcError> {
    let meta = encode_frame_meta(&ErrorMeta {
        code: code.into(),
        message: message.into(),
        fatal,
    })?;
    tunnel
        .send(Frame {
            frame_type: FrameType::Error,
            flags: 0,
            stream_id,
            meta,
            data: Vec::new(),
        })
        .await
}

fn decode_frame_meta<T: DeserializeOwned>(frame: &Frame) -> Result<T, HostRpcError> {
    serde_json::from_slice(&frame.meta).map_err(|err| {
        rpc_error(
            "invalid_port_tunnel_metadata",
            format!("invalid port tunnel metadata: {err}"),
        )
    })
}

fn encode_frame_meta<T: Serialize>(meta: &T) -> Result<Vec<u8>, HostRpcError> {
    serde_json::to_vec(meta).map_err(|err| {
        rpc_error(
            "invalid_port_tunnel_metadata",
            format!("invalid port tunnel metadata: {err}"),
        )
    })
}

fn rpc_error(code: &'static str, message: impl Into<String>) -> HostRpcError {
    let message = message.into();
    tracing::warn!(code, %message, "daemon request rejected");
    HostRpcError {
        status: 400,
        code,
        message,
    }
}

#[cfg(test)]
mod port_tunnel_tests {
    use std::sync::Arc;
    use std::time::Duration;

    use remote_exec_proto::port_tunnel::{
        Frame, FrameType, read_frame, write_frame, write_preface,
    };
    use serde_json::Value;
    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

    use super::*;
    use crate::{
        HostRuntimeConfig, ProcessEnvironment, PtyMode, YieldTimeConfig, build_runtime_state,
    };

    #[tokio::test]
    async fn tunnel_binds_tcp_listener_and_releases_it_on_drop() {
        let state = test_state();
        let listen_endpoint = free_loopback_endpoint().await;
        let (mut broker_side, daemon_side) = tokio::io::duplex(64 * 1024);
        tokio::spawn(serve_tunnel(state.clone(), daemon_side));

        write_preface(&mut broker_side).await.unwrap();
        write_frame(
            &mut broker_side,
            &json_frame(
                FrameType::TcpListen,
                1,
                serde_json::json!({ "endpoint": listen_endpoint }),
            ),
        )
        .await
        .unwrap();

        let ok = read_frame(&mut broker_side).await.unwrap();
        assert_eq!(ok.frame_type, FrameType::TcpListenOk);
        let bound_endpoint = endpoint_from_frame(&ok);
        drop(broker_side);

        wait_until_bindable(&bound_endpoint).await;
    }

    #[tokio::test]
    async fn tunnel_tcp_connect_echoes_binary_data_full_duplex() {
        let state = test_state();
        let echo_endpoint = spawn_tcp_echo_server().await;
        let (mut broker_side, daemon_side) = tokio::io::duplex(64 * 1024);
        tokio::spawn(serve_tunnel(state, daemon_side));

        write_preface(&mut broker_side).await.unwrap();
        write_frame(
            &mut broker_side,
            &json_frame(
                FrameType::TcpConnect,
                1,
                serde_json::json!({ "endpoint": echo_endpoint }),
            ),
        )
        .await
        .unwrap();
        assert_eq!(
            read_frame(&mut broker_side).await.unwrap().frame_type,
            FrameType::TcpConnectOk
        );

        write_frame(
            &mut broker_side,
            &data_frame(FrameType::TcpData, 1, b"\0hello\xff".to_vec()),
        )
        .await
        .unwrap();
        let echoed = read_frame(&mut broker_side).await.unwrap();
        assert_eq!(echoed.frame_type, FrameType::TcpData);
        assert_eq!(echoed.data, b"\0hello\xff");
    }

    #[tokio::test]
    async fn tunnel_udp_bind_relays_datagrams_from_two_peers() {
        let state = test_state();
        let endpoint = free_loopback_endpoint().await;
        let (mut broker_side, daemon_side) = tokio::io::duplex(64 * 1024);
        tokio::spawn(serve_tunnel(state, daemon_side));

        write_preface(&mut broker_side).await.unwrap();
        write_frame(
            &mut broker_side,
            &json_frame(
                FrameType::UdpBind,
                1,
                serde_json::json!({ "endpoint": endpoint }),
            ),
        )
        .await
        .unwrap();
        let bind_ok = read_frame(&mut broker_side).await.unwrap();
        assert_eq!(bind_ok.frame_type, FrameType::UdpBindOk);
        let bound_endpoint = endpoint_from_frame(&bind_ok);

        let peer_a = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let peer_b = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        peer_a.send_to(b"from-a", &bound_endpoint).await.unwrap();
        peer_b.send_to(b"from-b", &bound_endpoint).await.unwrap();

        let first = read_frame(&mut broker_side).await.unwrap();
        let second = read_frame(&mut broker_side).await.unwrap();
        assert_eq!(first.frame_type, FrameType::UdpDatagram);
        assert_eq!(second.frame_type, FrameType::UdpDatagram);
        assert_eq!(
            sorted_payloads([first.data, second.data]),
            vec![b"from-a".to_vec(), b"from-b".to_vec()]
        );
    }

    #[tokio::test]
    async fn tcp_listener_session_can_resume_after_transport_drop() {
        let state = test_state();
        let listen_endpoint = free_loopback_endpoint().await;
        let (listen_bound_endpoint, session_id) =
            open_resumable_tcp_listener(&state, &listen_endpoint).await;

        let mut resumed = resume_session(&state, &session_id).await;
        let accept = tokio::net::TcpStream::connect(&listen_bound_endpoint)
            .await
            .unwrap();
        drop(accept);

        let frame = read_frame(&mut resumed).await.unwrap();
        assert_eq!(frame.frame_type, FrameType::TcpAccept);
    }

    #[tokio::test]
    async fn udp_bind_session_can_resume_after_transport_drop() {
        let state = test_state();
        let listen_endpoint = free_loopback_endpoint().await;
        let (listen_bound_endpoint, session_id) =
            open_resumable_udp_bind(&state, &listen_endpoint).await;

        let mut resumed = resume_session(&state, &session_id).await;
        let sender = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        sender
            .send_to(b"ping", &listen_bound_endpoint)
            .await
            .unwrap();

        let frame = read_frame(&mut resumed).await.unwrap();
        assert_eq!(frame.frame_type, FrameType::UdpDatagram);
        assert_eq!(frame.data, b"ping");
    }

    #[tokio::test]
    async fn expired_detached_listener_is_released() {
        let state = test_state();
        let listen_endpoint = free_loopback_endpoint().await;
        let (bound_endpoint, _session_id) =
            open_resumable_tcp_listener(&state, &listen_endpoint).await;

        tokio::time::sleep(Duration::from_millis(250)).await;

        wait_until_bindable(&bound_endpoint).await;
    }

    #[tokio::test]
    async fn detached_tcp_listener_accepts_after_resume_not_before() {
        let state = test_state();
        let listen_endpoint = free_loopback_endpoint().await;
        let (bound_endpoint, session_id) =
            open_resumable_tcp_listener(&state, &listen_endpoint).await;

        tokio::time::sleep(Duration::from_millis(20)).await;
        let client = tokio::net::TcpStream::connect(&bound_endpoint)
            .await
            .unwrap();
        let mut resumed = resume_session(&state, &session_id).await;

        let frame = tokio::time::timeout(Duration::from_secs(1), read_frame(&mut resumed))
            .await
            .expect("accept should arrive after resume")
            .unwrap();
        assert_eq!(frame.frame_type, FrameType::TcpAccept);

        drop(client);
    }

    #[tokio::test]
    async fn tunnel_reports_tcp_listen_errors_on_request_stream() {
        let state = test_state();
        let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let occupied_endpoint = occupied.local_addr().unwrap().to_string();
        let (mut broker_side, daemon_side) = tokio::io::duplex(64 * 1024);
        tokio::spawn(serve_tunnel(state, daemon_side));

        write_preface(&mut broker_side).await.unwrap();
        write_frame(
            &mut broker_side,
            &json_frame(
                FrameType::TcpListen,
                7,
                serde_json::json!({ "endpoint": occupied_endpoint }),
            ),
        )
        .await
        .unwrap();

        let error = tokio::time::timeout(Duration::from_secs(1), read_frame(&mut broker_side))
            .await
            .expect("listen error frame should arrive")
            .unwrap();
        assert_eq!(error.frame_type, FrameType::Error);
        assert_eq!(error.stream_id, 7);
    }

    #[tokio::test]
    async fn tunnel_exits_promptly_when_host_shuts_down() {
        let state = test_state();
        let (mut broker_side, daemon_side) = tokio::io::duplex(64 * 1024);
        let tunnel_task = tokio::spawn(serve_tunnel(state.clone(), daemon_side));

        write_preface(&mut broker_side).await.unwrap();
        state.shutdown.cancel();

        let result = tokio::time::timeout(Duration::from_secs(1), tunnel_task)
            .await
            .expect("tunnel should exit after host shutdown")
            .unwrap();
        result.unwrap();
    }

    fn test_state() -> Arc<AppState> {
        let workdir = std::env::temp_dir().join(format!(
            "remote-exec-host-port-tunnel-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&workdir).unwrap();
        Arc::new(
            build_runtime_state(HostRuntimeConfig {
                target: "test".to_string(),
                default_workdir: workdir,
                windows_posix_root: None,
                sandbox: None,
                enable_transfer_compression: true,
                allow_login_shell: true,
                pty: PtyMode::None,
                default_shell: None,
                yield_time: YieldTimeConfig::default(),
                experimental_apply_patch_target_encoding_autodetect: false,
                process_environment: ProcessEnvironment::capture_current(),
            })
            .unwrap(),
        )
    }

    async fn start_tunnel(state: Arc<AppState>) -> DuplexStream {
        let (mut broker_side, daemon_side) = tokio::io::duplex(64 * 1024);
        tokio::spawn(serve_tunnel(state, daemon_side));
        write_preface(&mut broker_side).await.unwrap();
        broker_side
    }

    async fn open_resumable_tcp_listener(
        state: &Arc<AppState>,
        endpoint: &str,
    ) -> (String, String) {
        let mut broker_side = start_tunnel(state.clone()).await;
        write_frame(
            &mut broker_side,
            &Frame {
                frame_type: FrameType::SessionOpen,
                flags: 0,
                stream_id: 0,
                meta: Vec::new(),
                data: Vec::new(),
            },
        )
        .await
        .unwrap();
        let ready = read_frame(&mut broker_side).await.unwrap();
        let session_id = serde_json::from_slice::<Value>(&ready.meta).unwrap()["session_id"]
            .as_str()
            .unwrap()
            .to_string();
        write_frame(
            &mut broker_side,
            &json_frame(
                FrameType::TcpListen,
                1,
                serde_json::json!({ "endpoint": endpoint }),
            ),
        )
        .await
        .unwrap();
        let ok = read_frame(&mut broker_side).await.unwrap();
        let bound_endpoint = endpoint_from_frame(&ok);
        drop(broker_side);
        (bound_endpoint, session_id)
    }

    async fn open_resumable_udp_bind(state: &Arc<AppState>, endpoint: &str) -> (String, String) {
        let mut broker_side = start_tunnel(state.clone()).await;
        write_frame(
            &mut broker_side,
            &Frame {
                frame_type: FrameType::SessionOpen,
                flags: 0,
                stream_id: 0,
                meta: Vec::new(),
                data: Vec::new(),
            },
        )
        .await
        .unwrap();
        let ready = read_frame(&mut broker_side).await.unwrap();
        let session_id = serde_json::from_slice::<Value>(&ready.meta).unwrap()["session_id"]
            .as_str()
            .unwrap()
            .to_string();
        write_frame(
            &mut broker_side,
            &json_frame(
                FrameType::UdpBind,
                1,
                serde_json::json!({ "endpoint": endpoint }),
            ),
        )
        .await
        .unwrap();
        let ok = read_frame(&mut broker_side).await.unwrap();
        let bound_endpoint = endpoint_from_frame(&ok);
        drop(broker_side);
        (bound_endpoint, session_id)
    }

    async fn resume_session(state: &Arc<AppState>, session_id: &str) -> DuplexStream {
        let mut broker_side = start_tunnel(state.clone()).await;
        write_frame(
            &mut broker_side,
            &json_frame(
                FrameType::SessionResume,
                0,
                serde_json::json!({ "session_id": session_id }),
            ),
        )
        .await
        .unwrap();
        let resumed = read_frame(&mut broker_side).await.unwrap();
        assert_eq!(resumed.frame_type, FrameType::SessionResumed);
        broker_side
    }

    async fn free_loopback_endpoint() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = listener.local_addr().unwrap().to_string();
        drop(listener);
        endpoint
    }

    async fn wait_until_bindable(endpoint: &str) {
        for _ in 0..40 {
            if tokio::net::TcpListener::bind(endpoint).await.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("endpoint `{endpoint}` did not become bindable");
    }

    async fn spawn_tcp_echo_server() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = listener.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = vec![0; 1024];
            let read = stream.read(&mut buf).await.unwrap();
            stream.write_all(&buf[..read]).await.unwrap();
        });
        endpoint
    }

    fn json_frame(frame_type: FrameType, stream_id: u32, meta: Value) -> Frame {
        Frame {
            frame_type,
            flags: 0,
            stream_id,
            meta: serde_json::to_vec(&meta).unwrap(),
            data: Vec::new(),
        }
    }

    fn data_frame(frame_type: FrameType, stream_id: u32, data: Vec<u8>) -> Frame {
        Frame {
            frame_type,
            flags: 0,
            stream_id,
            meta: Vec::new(),
            data,
        }
    }

    fn endpoint_from_frame(frame: &Frame) -> String {
        serde_json::from_slice::<Value>(&frame.meta).unwrap()["endpoint"]
            .as_str()
            .unwrap()
            .to_string()
    }

    fn sorted_payloads<const N: usize>(payloads: [Vec<u8>; N]) -> Vec<Vec<u8>> {
        let mut payloads = Vec::from(payloads);
        payloads.sort();
        payloads
    }
}
