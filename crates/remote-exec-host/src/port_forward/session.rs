use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

use remote_exec_proto::rpc::RpcErrorCode;
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::HostRpcError;

use super::error::{SessionCloseMode, rpc_error};
use super::limiter::{PortForwardLimiter, PortForwardPermit};
use super::session_store::TunnelSessionStore;
use super::udp::tunnel_udp_read_loop_attached_session;
use super::{
    ActiveTunnelState, TcpStreamEntry, TunnelSender, TunnelState, UdpReaderEntry, timings,
};

pub(super) struct SessionState {
    pub(super) id: String,
    pub(super) root_cancel: CancellationToken,
    pub(super) attachment: Mutex<Option<Arc<AttachmentState>>>,
    pub(super) attachment_notify: tokio::sync::Notify,
    pub(super) resume_deadline: Mutex<Option<Instant>>,
    pub(super) expiry_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    pub(super) retained_listener: Mutex<Option<RetainedListener>>,
    pub(super) retained_udp_bind: Mutex<Option<RetainedUdpBind>>,
    pub(super) next_daemon_stream_id: AtomicU32,
    pub(super) generation: AtomicU64,
}

pub(super) struct AttachmentState {
    pub(super) tx: TunnelSender,
    pub(super) cancel: CancellationToken,
    pub(super) tcp_streams: Mutex<HashMap<u32, TcpStreamEntry>>,
    pub(super) udp_readers: Mutex<HashMap<u32, UdpReaderEntry>>,
}

pub(super) enum RetainedListener {
    Tcp {
        stream_id: u32,
        _listener: Arc<TcpListener>,
        _permit: PortForwardPermit,
    },
}

pub(super) enum RetainedUdpBind {
    Udp {
        stream_id: u32,
        socket: Arc<UdpSocket>,
        _permit: PortForwardPermit,
    },
}

impl SessionState {
    pub(super) fn new(root_cancel: CancellationToken) -> Self {
        Self {
            id: crate::ids::new_tunnel_session_id(),
            root_cancel,
            attachment: Mutex::new(None),
            attachment_notify: tokio::sync::Notify::new(),
            resume_deadline: Mutex::new(None),
            expiry_task: Mutex::new(None),
            retained_listener: Mutex::new(None),
            retained_udp_bind: Mutex::new(None),
            next_daemon_stream_id: AtomicU32::new(2),
            generation: AtomicU64::new(0),
        }
    }

    pub(super) async fn current_attachment(&self) -> Option<Arc<AttachmentState>> {
        self.attachment.lock().await.clone()
    }

    pub(super) fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    pub(super) fn set_generation(&self, generation: u64) {
        self.generation.store(generation, Ordering::Release);
    }

    pub(super) async fn is_expired(&self) -> bool {
        self.resume_deadline
            .lock()
            .await
            .as_ref()
            .is_some_and(|deadline| Instant::now() >= *deadline)
    }

    pub(super) async fn replace_listener(
        &self,
        stream_id: u32,
        listener: Arc<TcpListener>,
        limiter: &Arc<PortForwardLimiter>,
    ) -> Result<(), HostRpcError> {
        let mut retained_listener = self.retained_listener.lock().await;
        let permit = match retained_listener.take() {
            Some(RetainedListener::Tcp { _permit, .. }) => _permit,
            None => limiter.try_acquire_retained_listener()?,
        };
        *retained_listener = Some(RetainedListener::Tcp {
            stream_id,
            _listener: listener,
            _permit: permit,
        });
        Ok(())
    }

    pub(super) async fn replace_udp_bind(
        &self,
        stream_id: u32,
        socket: Arc<UdpSocket>,
        limiter: &Arc<PortForwardLimiter>,
    ) -> Result<(), HostRpcError> {
        let mut retained_udp_bind = self.retained_udp_bind.lock().await;
        let permit = match retained_udp_bind.take() {
            Some(RetainedUdpBind::Udp { _permit, .. }) => _permit,
            None => limiter.try_acquire_udp_bind()?,
        };
        *retained_udp_bind = Some(RetainedUdpBind::Udp {
            stream_id,
            socket,
            _permit: permit,
        });
        Ok(())
    }

    pub(super) async fn udp_socket(&self, stream_id: u32) -> Option<Arc<UdpSocket>> {
        match self.retained_udp_bind.lock().await.as_ref() {
            Some(RetainedUdpBind::Udp {
                stream_id: retained_stream_id,
                socket,
                ..
            }) if *retained_stream_id == stream_id => Some(socket.clone()),
            _ => None,
        }
    }

    pub(super) async fn udp_bind_snapshot(&self) -> Option<(u32, Arc<UdpSocket>)> {
        self.retained_udp_bind.lock().await.as_ref().map(
            |RetainedUdpBind::Udp {
                 stream_id, socket, ..
             }| { (*stream_id, socket.clone()) },
        )
    }

    pub(super) async fn close_non_resumable_streams(&self) {
        if let Some(attachment) = self.attachment.lock().await.clone() {
            for (_, mut stream) in attachment.tcp_streams.lock().await.drain() {
                if let Some(cancel) = stream.cancel.take() {
                    cancel.cancel();
                }
            }
            for (_, reader) in attachment.udp_readers.lock().await.drain() {
                reader.cancel.cancel();
            }
        }
    }

    pub(super) async fn close_retained_resources(&self) {
        *self.retained_listener.lock().await = None;
        *self.retained_udp_bind.lock().await = None;
        self.close_non_resumable_streams().await;
    }

    #[cfg(test)]
    pub(super) async fn has_retained_listener(&self) -> bool {
        self.retained_listener.lock().await.is_some()
    }

    #[cfg(test)]
    pub(super) async fn has_expiry_task(&self) -> bool {
        self.expiry_task.lock().await.is_some()
    }
}

pub(super) async fn attach_session_to_tunnel(
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
            tcp_streams: Mutex::new(HashMap::new()),
            udp_readers: Mutex::new(HashMap::new()),
        }));
    }
    if let Some(task) = session.expiry_task.lock().await.take() {
        task.abort();
    }
    *session.resume_deadline.lock().await = None;
    *tunnel.active.lock().await = Some(ActiveTunnelState::Listen(session.clone()));
    session.attachment_notify.notify_waiters();
    Ok(())
}

pub(super) async fn close_attached_session(tunnel: &Arc<TunnelState>, mode: SessionCloseMode) {
    let session = {
        let mut active = tunnel.active.lock().await;
        match active.take() {
            Some(ActiveTunnelState::Listen(session)) => session,
            Some(other) => {
                *active = Some(other);
                return;
            }
            None => return,
        }
    };
    if let Some(attachment) = session.attachment.lock().await.take() {
        attachment.cancel.cancel();
        for (_, mut stream) in attachment.tcp_streams.lock().await.drain() {
            if let Some(cancel) = stream.cancel.take() {
                cancel.cancel();
            }
        }
        for (_, reader) in attachment.udp_readers.lock().await.drain() {
            reader.cancel.cancel();
        }
    }

    match mode {
        SessionCloseMode::RetryableDetach => {
            *session.resume_deadline.lock().await = Some(Instant::now() + timings().resume_timeout);
            schedule_session_expiry(tunnel.state.port_forward_sessions.clone(), session).await;
        }
        SessionCloseMode::GracefulClose | SessionCloseMode::TerminalFailure => {
            *session.resume_deadline.lock().await = None;
            tunnel.state.port_forward_sessions.remove(&session.id).await;
            session.close_retained_resources().await;
            session.root_cancel.cancel();
        }
    }
}

pub(super) fn close_mode_for_tunnel_result(
    result: &Result<SessionCloseMode, HostRpcError>,
    host_shutdown: bool,
) -> SessionCloseMode {
    if host_shutdown {
        return SessionCloseMode::TerminalFailure;
    }
    match result {
        Ok(mode) => *mode,
        Err(_) => SessionCloseMode::TerminalFailure,
    }
}

pub(super) async fn listener_stream_id(session: &Arc<SessionState>) -> Option<u32> {
    session
        .retained_listener
        .lock()
        .await
        .as_ref()
        .map(|RetainedListener::Tcp { stream_id, .. }| *stream_id)
}

pub(super) async fn wait_for_session_attachment(
    session: &Arc<SessionState>,
) -> Option<Arc<AttachmentState>> {
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

pub(super) async fn udp_bind_stream_id(session: &Arc<SessionState>) -> Option<u32> {
    session
        .retained_udp_bind
        .lock()
        .await
        .as_ref()
        .map(|RetainedUdpBind::Udp { stream_id, .. }| *stream_id)
}

pub(super) async fn schedule_session_expiry(store: TunnelSessionStore, session: Arc<SessionState>) {
    let task_session = session.clone();
    let handle = tokio::spawn(async move {
        tokio::time::sleep(timings().resume_timeout).await;
        if task_session.is_expired().await && task_session.current_attachment().await.is_none() {
            store.remove(&task_session.id).await;
            task_session.close_retained_resources().await;
            task_session.root_cancel.cancel();
        }
    });
    let mut expiry_task = session.expiry_task.lock().await;
    if let Some(existing) = expiry_task.take() {
        existing.abort();
    }
    *expiry_task = Some(handle);
}

pub(super) async fn reactivate_retained_udp_bind(
    session: &Arc<SessionState>,
) -> Result<(), HostRpcError> {
    let Some((stream_id, socket)) = session.udp_bind_snapshot().await else {
        return Ok(());
    };
    let attachment = session.current_attachment().await.ok_or_else(|| {
        rpc_error(
            RpcErrorCode::PortTunnelClosed,
            "port tunnel attachment is closed",
        )
    })?;
    let stream_cancel = attachment.cancel.child_token();
    if let Some(existing) = attachment.udp_readers.lock().await.insert(
        stream_id,
        UdpReaderEntry {
            cancel: stream_cancel.clone(),
        },
    ) {
        existing.cancel.cancel();
    }
    tokio::spawn(tunnel_udp_read_loop_attached_session(
        session.clone(),
        attachment,
        stream_id,
        socket,
        stream_cancel,
    ));
    Ok(())
}
