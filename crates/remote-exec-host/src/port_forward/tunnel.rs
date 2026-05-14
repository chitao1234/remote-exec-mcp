use std::io::ErrorKind;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::{AppState, HostRpcError};
use remote_exec_proto::port_tunnel::{
    Frame, FrameType, TunnelCloseMeta, TunnelForwardProtocol, TunnelLimitSummary, TunnelOpenMeta,
    TunnelReadyMeta, TunnelRole, read_frame, read_preface, write_frame,
};
use remote_exec_proto::rpc::RpcErrorCode;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{Mutex, mpsc};

use super::active::{connection_generation, send_tunnel_error, send_tunnel_error_code};
use super::codec::decode_frame_meta;
use super::error::rpc_error;
use super::frames::{frame as raw_frame, meta_frame};
use super::session::{
    SessionState, attach_session_to_tunnel, close_listen_session, close_mode_for_tunnel_result,
    reactivate_retained_udp_bind,
};
use super::tcp::{
    tunnel_close_stream, tunnel_tcp_connect, tunnel_tcp_data, tunnel_tcp_eof, tunnel_tcp_listen,
};
use super::udp::{tunnel_udp_bind, tunnel_udp_datagram};
use super::{
    ActiveTunnelState, ConnectRuntimeState, PortForwardPermit, QueuedFrame, TunnelMode,
    TunnelSender, TunnelState,
};

// Keep enough buffered slack for queued data plus follow-up control frames
// without letting tunnel-local memory growth become unbounded.
const TUNNEL_FRAME_QUEUE_CAPACITY: usize = 128;

// On shutdown, give already queued frames a brief chance to flush without
// blocking tunnel detach indefinitely.
const WRITER_TASK_DRAIN_TIMEOUT: Duration = Duration::from_millis(100);

pub async fn serve_tunnel<S>(state: Arc<AppState>, stream: S) -> Result<(), HostRpcError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let connection_permit = state.port_forward_limiter.try_acquire_tunnel_connection()?;
    serve_tunnel_with_permit(state, stream, connection_permit).await
}

pub fn reserve_tunnel_connection(state: &AppState) -> Result<PortForwardPermit, HostRpcError> {
    state.port_forward_limiter.try_acquire_tunnel_connection()
}

pub async fn serve_tunnel_with_permit<S>(
    state: Arc<AppState>,
    stream: S,
    connection_permit: PortForwardPermit,
) -> Result<(), HostRpcError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut reader, mut writer) = tokio::io::split(stream);
    read_preface(&mut reader)
        .await
        .map_err(|err| rpc_error(RpcErrorCode::InvalidPortTunnel, err.to_string()))?;

    let (tx, mut rx) = mpsc::channel::<QueuedFrame>(TUNNEL_FRAME_QUEUE_CAPACITY);
    let sender = TunnelSender {
        tx: tx.clone(),
        limiter: state.port_forward_limiter.clone(),
    };
    let tunnel = Arc::new(TunnelState {
        state: state.clone(),
        cancel: state.shutdown.child_token(),
        tx: sender,
        open_mode: tokio::sync::Mutex::new(TunnelMode::Unopened),
        last_generation: std::sync::atomic::AtomicU64::new(0),
        active: Mutex::new(None),
        _connection_permit: connection_permit,
    });
    let writer_cancel = tunnel.cancel.clone();
    let writer_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                queued = rx.recv() => {
                    let Some(queued) = queued else {
                        return;
                    };
                    let QueuedFrame { frame, .. } = queued;
                    if write_frame(&mut writer, &frame).await.is_err() {
                        writer_cancel.cancel();
                        return;
                    }
                }
                _ = writer_cancel.cancelled() => {
                    while let Ok(queued) = rx.try_recv() {
                        let QueuedFrame { frame, .. } = queued;
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
    close_tunnel_runtime(
        &tunnel,
        close_mode_for_tunnel_result(&result, state.shutdown.is_cancelled()),
    )
    .await;
    tunnel.cancel.cancel();
    drop(tx);
    let _ = tokio::time::timeout(WRITER_TASK_DRAIN_TIMEOUT, writer_task).await;
    result.map(|_| ())
}

pub(super) async fn tunnel_read_loop<R>(
    tunnel: Arc<TunnelState>,
    reader: &mut R,
) -> Result<super::error::SessionCloseMode, HostRpcError>
where
    R: AsyncRead + Unpin,
{
    loop {
        let frame = tokio::select! {
            _ = tunnel.cancel.cancelled() => return Ok(super::error::SessionCloseMode::RetryableDetach),
            frame = read_frame(reader) => {
                match frame {
                    Ok(frame) => frame,
                    Err(err) if err.kind() == ErrorKind::UnexpectedEof => {
                        return Ok(super::error::SessionCloseMode::RetryableDetach);
                    }
                    Err(err) => {
                        let generation = connection_generation(&tunnel).await;
                        let _ = send_tunnel_error(
                            &tunnel.tx,
                            generation,
                            0,
                            RpcErrorCode::InvalidPortTunnel,
                            err.to_string(),
                            true,
                        )
                        .await;
                        return Err(rpc_error(RpcErrorCode::InvalidPortTunnel, err.to_string()));
                    }
                }
            }
        };

        let stream_id = frame.stream_id;
        let graceful_close = frame.frame_type == FrameType::TunnelClose && stream_id == 0;
        if let Err(err) = handle_tunnel_frame(tunnel.clone(), frame).await {
            let generation = connection_generation(&tunnel).await;
            let _ = send_tunnel_error_code(
                &tunnel.tx,
                generation,
                stream_id,
                err.code,
                err.message.clone(),
                false,
            )
            .await;
            continue;
        }
        if graceful_close {
            return Ok(super::error::SessionCloseMode::GracefulClose);
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
                .send(raw_frame(
                    FrameType::TunnelHeartbeatAck,
                    0,
                    frame.meta,
                    Vec::new(),
                ))
                .await
        }
        FrameType::TcpListen => tunnel_tcp_listen(tunnel, frame).await,
        FrameType::TcpConnect => tunnel_tcp_connect(tunnel, frame).await,
        FrameType::TcpData => tunnel_tcp_data(&tunnel, frame.stream_id, &frame.data).await,
        FrameType::TcpEof => tunnel_tcp_eof(&tunnel, frame.stream_id).await,
        FrameType::Close => tunnel_close_stream(&tunnel, frame.stream_id).await,
        FrameType::UdpBind => tunnel_udp_bind(tunnel, frame).await,
        FrameType::UdpDatagram => tunnel_udp_datagram(&tunnel, frame).await,
        _ => Err(rpc_error(
            RpcErrorCode::InvalidPortTunnel,
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
            RpcErrorCode::InvalidPortTunnel,
            "tunnel open must use stream_id 0",
        ));
    }
    let meta: TunnelOpenMeta = decode_frame_meta(&frame)?;
    match meta.role {
        TunnelRole::Listen => tunnel_open_listen(tunnel, meta).await,
        TunnelRole::Connect => tunnel_open_connect(tunnel, meta).await,
    }
}

async fn tunnel_open_listen(
    tunnel: Arc<TunnelState>,
    meta: TunnelOpenMeta,
) -> Result<(), HostRpcError> {
    tunnel
        .last_generation
        .store(meta.generation, Ordering::Release);
    let listen_session =
        acquire_listen_open_session(&tunnel, meta.resume_session_id.as_deref()).await?;
    open_listen_session(
        &tunnel,
        &listen_session,
        meta.generation,
        meta.protocol.clone(),
    )
    .await?;
    send_tunnel_ready(
        &tunnel,
        meta.generation,
        Some(listen_session.session.id.clone()),
        Some(super::timings().resume_timeout.as_millis() as u64),
    )
    .await?;
    if meta.protocol == TunnelForwardProtocol::Udp {
        reactivate_retained_udp_bind(&listen_session.session).await?;
    }
    Ok(())
}

async fn tunnel_open_connect(
    tunnel: Arc<TunnelState>,
    meta: TunnelOpenMeta,
) -> Result<(), HostRpcError> {
    claim_tunnel_mode(
        &tunnel,
        TunnelMode::Connect {
            protocol: meta.protocol,
        },
    )
    .await?;
    tunnel
        .last_generation
        .store(meta.generation, Ordering::Release);
    *tunnel.active.lock().await = Some(ActiveTunnelState::Connect(Arc::new(ConnectRuntimeState {
        tx: tunnel.tx.clone(),
        cancel: tunnel.cancel.child_token(),
        generation: meta.generation,
        tcp_streams: Mutex::new(std::collections::HashMap::new()),
        udp_binds: Mutex::new(std::collections::HashMap::new()),
    })));
    send_tunnel_ready(&tunnel, meta.generation, None, None).await
}

async fn claim_tunnel_mode(
    tunnel: &Arc<TunnelState>,
    mode: TunnelMode,
) -> Result<(), HostRpcError> {
    let mut open_mode = tunnel.open_mode.lock().await;
    if !matches!(*open_mode, TunnelMode::Unopened) {
        return Err(rpc_error(
            RpcErrorCode::PortTunnelAlreadyAttached,
            "port tunnel is already open",
        ));
    }
    *open_mode = mode;
    Ok(())
}

async fn tunnel_close(tunnel: Arc<TunnelState>, frame: Frame) -> Result<(), HostRpcError> {
    if frame.stream_id != 0 {
        return Err(rpc_error(
            RpcErrorCode::InvalidPortTunnel,
            "tunnel close must use stream_id 0",
        ));
    }
    let meta: TunnelCloseMeta = decode_frame_meta(&frame)?;
    ensure_tunnel_generation(&tunnel, meta.generation).await?;
    tunnel
        .send(meta_frame(FrameType::TunnelClosed, 0, &meta)?)
        .await
}

fn new_session(tunnel: &Arc<TunnelState>) -> Arc<SessionState> {
    Arc::new(SessionState::new(tunnel.state.shutdown.child_token()))
}

#[cfg(test)]
pub(super) fn new_session_for_test(state: &Arc<AppState>) -> Arc<SessionState> {
    Arc::new(SessionState::new(state.shutdown.child_token()))
}

fn tunnel_limit_summary(tunnel: &TunnelState) -> TunnelLimitSummary {
    let limits = tunnel.state.config.port_forward_limits.capacity();
    TunnelLimitSummary {
        max_active_tcp_streams: limits.max_active_tcp_streams as u64,
        max_udp_peers: limits.max_udp_binds as u64,
        max_queued_bytes: limits.max_tunnel_queued_bytes as u64,
    }
}

struct ListenOpenSession {
    session: Arc<SessionState>,
    inserted_session: bool,
}

async fn acquire_listen_open_session(
    tunnel: &Arc<TunnelState>,
    resume_session_id: Option<&str>,
) -> Result<ListenOpenSession, HostRpcError> {
    match resume_session_id {
        Some(session_id) => {
            let session = tunnel
                .state
                .port_forward_sessions
                .get(session_id)
                .await
                .ok_or_else(|| {
                    rpc_error(
                        RpcErrorCode::UnknownPortTunnelSession,
                        "unknown port tunnel session",
                    )
                })?;
            if session.is_expired().await {
                tunnel.state.port_forward_sessions.remove(session_id).await;
                session.close_retained_resources().await;
                return Err(rpc_error(
                    RpcErrorCode::PortTunnelResumeExpired,
                    "port tunnel resume expired",
                ));
            }
            Ok(ListenOpenSession {
                session,
                inserted_session: false,
            })
        }
        None => {
            let session = new_session(tunnel);
            tunnel
                .state
                .port_forward_sessions
                .try_insert(
                    session.clone(),
                    tunnel
                        .state
                        .config
                        .port_forward_limits
                        .capacity()
                        .max_retained_sessions,
                )
                .await?;
            Ok(ListenOpenSession {
                session,
                inserted_session: true,
            })
        }
    }
}

async fn open_listen_session(
    tunnel: &Arc<TunnelState>,
    listen_session: &ListenOpenSession,
    generation: u64,
    protocol: TunnelForwardProtocol,
) -> Result<(), HostRpcError> {
    listen_session.session.set_generation(generation);
    if let Err(err) = claim_tunnel_mode(tunnel, TunnelMode::Listen { protocol }).await {
        cleanup_inserted_listen_session(tunnel, listen_session).await;
        return Err(err);
    }
    attach_session_to_tunnel(&listen_session.session, tunnel).await
}

async fn cleanup_inserted_listen_session(
    tunnel: &Arc<TunnelState>,
    listen_session: &ListenOpenSession,
) {
    if listen_session.inserted_session {
        tunnel
            .state
            .port_forward_sessions
            .remove(&listen_session.session.id)
            .await;
        listen_session.session.root_cancel.cancel();
    }
}

async fn send_tunnel_ready(
    tunnel: &Arc<TunnelState>,
    generation: u64,
    session_id: Option<String>,
    resume_timeout_ms: Option<u64>,
) -> Result<(), HostRpcError> {
    tunnel
        .send(meta_frame(
            FrameType::TunnelReady,
            0,
            &TunnelReadyMeta {
                generation,
                session_id,
                resume_timeout_ms,
                limits: tunnel_limit_summary(tunnel),
            },
        )?)
        .await
}

async fn ensure_tunnel_generation(
    tunnel: &Arc<TunnelState>,
    frame_generation: u64,
) -> Result<(), HostRpcError> {
    let current_generation = connection_generation(tunnel).await.ok_or_else(|| {
        rpc_error(
            RpcErrorCode::InvalidPortTunnel,
            "frame generation requires an active tunnel",
        )
    })?;
    if frame_generation != current_generation {
        return Err(rpc_error(
            RpcErrorCode::PortTunnelGenerationMismatch,
            format!(
                "frame generation `{frame_generation}` does not match tunnel generation `{current_generation}`"
            ),
        ));
    }
    Ok(())
}

pub(super) async fn tunnel_mode(tunnel: &Arc<TunnelState>) -> TunnelMode {
    tunnel.open_mode.lock().await.clone()
}

async fn close_tunnel_runtime(tunnel: &Arc<TunnelState>, mode: super::error::SessionCloseMode) {
    let Some(active) = tunnel.active.lock().await.take() else {
        return;
    };
    match active {
        ActiveTunnelState::Listen(session) => {
            close_listen_session(tunnel, session, mode).await;
        }
        ActiveTunnelState::Connect(runtime) => {
            runtime.cancel.cancel();
            for (_, mut stream) in runtime.tcp_streams.lock().await.drain() {
                if let Some(cancel) = stream.cancel.take() {
                    cancel.cancel();
                }
            }
            for (_, bind) in runtime.udp_binds.lock().await.drain() {
                bind.cancel.cancel();
            }
        }
    }
}
