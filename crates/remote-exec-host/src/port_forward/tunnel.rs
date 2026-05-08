use std::io::ErrorKind;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::{AppState, HostRpcError};
use remote_exec_proto::port_tunnel::{
    Frame, FrameType, TunnelCloseMeta, TunnelForwardProtocol, TunnelLimitSummary, TunnelOpenMeta,
    TunnelReadyMeta, TunnelRole, read_frame, read_preface, write_frame,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{Mutex, mpsc};

use super::codec::{decode_frame_meta, encode_frame_meta};
use super::error::rpc_error;
use super::session::{
    SessionState, attach_session_to_tunnel, close_attached_session, close_mode_for_tunnel_result,
    explicit_session, reactivate_retained_udp_bind,
};
use super::tcp::{
    tunnel_close_stream, tunnel_tcp_connect, tunnel_tcp_data, tunnel_tcp_eof, tunnel_tcp_listen,
};
use super::udp::{tunnel_udp_bind, tunnel_udp_datagram};
use super::{SessionReadyMeta, SessionResumeMeta, TunnelMode, TunnelState};

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
        tcp_writers: Mutex::new(std::collections::HashMap::new()),
        udp_sockets: Mutex::new(std::collections::HashMap::new()),
        stream_cancels: Mutex::new(std::collections::HashMap::new()),
        next_daemon_stream_id: std::sync::atomic::AtomicU32::new(2),
        generation: std::sync::atomic::AtomicU64::new(0),
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
    let _ = tokio::time::timeout(std::time::Duration::from_millis(100), writer_task).await;
    result
}

pub(super) async fn tunnel_read_loop<R>(
    tunnel: Arc<TunnelState>,
    reader: &mut R,
) -> Result<(), HostRpcError>
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

pub(super) async fn handle_tunnel_frame(
    tunnel: Arc<TunnelState>,
    frame: Frame,
) -> Result<(), HostRpcError> {
    match frame.frame_type {
        FrameType::TunnelOpen => tunnel_open(tunnel, frame).await,
        FrameType::TunnelClose => tunnel_close(tunnel, frame).await,
        FrameType::TunnelHeartbeat => {
            tunnel
                .send(Frame {
                    frame_type: FrameType::TunnelHeartbeatAck,
                    flags: 0,
                    stream_id: 0,
                    meta: frame.meta,
                    data: Vec::new(),
                })
                .await
        }
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

pub(super) async fn tunnel_open(
    tunnel: Arc<TunnelState>,
    frame: Frame,
) -> Result<(), HostRpcError> {
    if frame.stream_id != 0 {
        return Err(rpc_error(
            "invalid_port_tunnel",
            "tunnel open must use stream_id 0",
        ));
    }
    let meta: TunnelOpenMeta = decode_frame_meta(&frame)?;
    tunnel.generation.store(meta.generation, Ordering::Release);
    match meta.role {
        TunnelRole::Listen => tunnel_open_listen(tunnel, meta).await,
        TunnelRole::Connect => tunnel_open_connect(tunnel, meta).await,
    }
}

async fn tunnel_open_listen(
    tunnel: Arc<TunnelState>,
    meta: TunnelOpenMeta,
) -> Result<(), HostRpcError> {
    let session = match meta.resume_session_id {
        Some(session_id) => {
            let session = tunnel
                .state
                .port_forward_sessions
                .get(&session_id)
                .await
                .ok_or_else(|| {
                    rpc_error("unknown_port_tunnel_session", "unknown port tunnel session")
                })?;
            if session.is_expired().await {
                tunnel.state.port_forward_sessions.remove(&session_id).await;
                session.close_retained_resources().await;
                return Err(rpc_error(
                    "port_tunnel_resume_expired",
                    "port tunnel resume expired",
                ));
            }
            session
        }
        None => {
            let session = new_session(&tunnel);
            tunnel
                .state
                .port_forward_sessions
                .try_insert(
                    session.clone(),
                    tunnel
                        .state
                        .config
                        .port_forward_limits
                        .max_retained_sessions,
                )
                .await?;
            session
        }
    };
    session.generation.store(meta.generation, Ordering::Release);
    attach_session_to_tunnel(&session, &tunnel).await?;
    tunnel
        .send(Frame {
            frame_type: FrameType::TunnelReady,
            flags: 0,
            stream_id: 0,
            meta: encode_frame_meta(&TunnelReadyMeta {
                generation: meta.generation,
                session_id: Some(session.id.clone()),
                resume_timeout_ms: Some(super::RESUME_TIMEOUT.as_millis() as u64),
                limits: tunnel_limit_summary(&tunnel),
            })?,
            data: Vec::new(),
        })
        .await?;
    if meta.protocol == TunnelForwardProtocol::Udp {
        reactivate_retained_udp_bind(&session).await?;
    }
    Ok(())
}

async fn tunnel_open_connect(
    tunnel: Arc<TunnelState>,
    meta: TunnelOpenMeta,
) -> Result<(), HostRpcError> {
    tunnel
        .send(Frame {
            frame_type: FrameType::TunnelReady,
            flags: 0,
            stream_id: 0,
            meta: encode_frame_meta(&TunnelReadyMeta {
                generation: meta.generation,
                session_id: None,
                resume_timeout_ms: None,
                limits: tunnel_limit_summary(&tunnel),
            })?,
            data: Vec::new(),
        })
        .await
}

async fn tunnel_close(tunnel: Arc<TunnelState>, frame: Frame) -> Result<(), HostRpcError> {
    if frame.stream_id != 0 {
        return Err(rpc_error(
            "invalid_port_tunnel",
            "tunnel close must use stream_id 0",
        ));
    }
    let meta: TunnelCloseMeta = decode_frame_meta(&frame)?;
    ensure_tunnel_generation(&tunnel, meta.generation)?;
    close_attached_session(&tunnel, super::error::SessionCloseMode::GracefulClose).await;
    tunnel
        .send(Frame {
            frame_type: FrameType::TunnelClosed,
            flags: 0,
            stream_id: 0,
            meta: encode_frame_meta(&meta)?,
            data: Vec::new(),
        })
        .await
}

pub(super) async fn tunnel_session_open(
    tunnel: Arc<TunnelState>,
    frame: Frame,
) -> Result<(), HostRpcError> {
    if frame.stream_id != 0 {
        return Err(rpc_error(
            "invalid_port_tunnel",
            "session open must use stream_id 0",
        ));
    }
    let session = new_session(&tunnel);
    tunnel
        .state
        .port_forward_sessions
        .try_insert(
            session.clone(),
            tunnel
                .state
                .config
                .port_forward_limits
                .max_retained_sessions,
        )
        .await?;
    attach_session_to_tunnel(&session, &tunnel).await?;
    tunnel
        .send(Frame {
            frame_type: FrameType::SessionReady,
            flags: 0,
            stream_id: 0,
            meta: encode_frame_meta(&SessionReadyMeta {
                session_id: session.id.clone(),
                resume_timeout_ms: super::RESUME_TIMEOUT.as_millis() as u64,
            })?,
            data: Vec::new(),
        })
        .await
}

fn new_session(tunnel: &Arc<TunnelState>) -> Arc<SessionState> {
    Arc::new(SessionState {
        id: format!("sess_{}", uuid::Uuid::new_v4().simple()),
        root_cancel: tunnel.state.shutdown.child_token(),
        attachment: Mutex::new(None),
        attachment_notify: tokio::sync::Notify::new(),
        resume_deadline: Mutex::new(None),
        retained_listener: Mutex::new(None),
        retained_udp_bind: Mutex::new(None),
        next_daemon_stream_id: std::sync::atomic::AtomicU32::new(2),
        generation: std::sync::atomic::AtomicU64::new(0),
    })
}

fn tunnel_limit_summary(tunnel: &TunnelState) -> TunnelLimitSummary {
    let limits = tunnel.state.config.port_forward_limits;
    TunnelLimitSummary {
        max_active_tcp_streams: limits.max_active_tcp_streams as u64,
        max_udp_peers: limits.max_udp_binds as u64,
        max_queued_bytes: limits.max_tunnel_queued_bytes as u64,
    }
}

pub(super) async fn tunnel_session_resume(
    tunnel: Arc<TunnelState>,
    frame: Frame,
) -> Result<(), HostRpcError> {
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

fn ensure_tunnel_generation(
    tunnel: &TunnelState,
    frame_generation: u64,
) -> Result<(), HostRpcError> {
    let current_generation = tunnel.generation.load(Ordering::Acquire);
    if frame_generation != current_generation {
        return Err(rpc_error(
            "port_tunnel_generation_mismatch",
            format!(
                "frame generation `{frame_generation}` does not match tunnel generation `{current_generation}`"
            ),
        ));
    }
    Ok(())
}

pub(super) async fn tunnel_mode(tunnel: &Arc<TunnelState>) -> TunnelMode {
    match explicit_session(tunnel).await {
        Some(session) => TunnelMode::Session(session),
        None => TunnelMode::Transport,
    }
}

pub(super) async fn send_tunnel_error(
    tunnel: &TunnelState,
    stream_id: u32,
    code: impl Into<String>,
    message: impl Into<String>,
    fatal: bool,
) -> Result<(), HostRpcError> {
    let meta = encode_frame_meta(&super::ErrorMeta {
        code: code.into(),
        message: message.into(),
        fatal,
        generation: Some(tunnel.generation.load(Ordering::Acquire)),
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
