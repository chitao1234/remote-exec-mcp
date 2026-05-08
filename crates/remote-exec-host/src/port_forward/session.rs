use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64};
use std::time::Instant;

use remote_exec_proto::port_tunnel::{Frame, FrameType};
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

use crate::HostRpcError;

use super::codec::encode_frame_meta;
use super::error::{SessionCloseMode, rpc_error};
use super::session_store::TunnelSessionStore;
use super::udp::tunnel_udp_read_loop_session_owned;
use super::{ErrorMeta, RESUME_TIMEOUT, TunnelState};

pub(super) struct SessionState {
    pub(super) id: String,
    pub(super) root_cancel: CancellationToken,
    pub(super) attachment: Mutex<Option<Arc<AttachmentState>>>,
    pub(super) attachment_notify: tokio::sync::Notify,
    pub(super) resume_deadline: Mutex<Option<Instant>>,
    pub(super) retained_listener: Mutex<Option<RetainedListener>>,
    pub(super) retained_udp_bind: Mutex<Option<RetainedUdpBind>>,
    pub(super) next_daemon_stream_id: AtomicU32,
    pub(super) generation: AtomicU64,
}

pub(super) struct AttachmentState {
    pub(super) tx: mpsc::Sender<Frame>,
    pub(super) cancel: CancellationToken,
    pub(super) tcp_writers: Mutex<HashMap<u32, Arc<Mutex<tokio::net::tcp::OwnedWriteHalf>>>>,
    pub(super) stream_cancels: Mutex<HashMap<u32, CancellationToken>>,
}

pub(super) enum RetainedListener {
    Tcp {
        stream_id: u32,
        _listener: Arc<TcpListener>,
    },
}

pub(super) enum RetainedUdpBind {
    Udp {
        stream_id: u32,
        socket: Arc<UdpSocket>,
    },
}

impl SessionState {
    pub(super) async fn current_attachment(&self) -> Option<Arc<AttachmentState>> {
        self.attachment.lock().await.clone()
    }

    pub(super) async fn is_expired(&self) -> bool {
        self.resume_deadline
            .lock()
            .await
            .as_ref()
            .is_some_and(|deadline| Instant::now() >= *deadline)
    }

    pub(super) async fn replace_listener(&self, stream_id: u32, listener: Arc<TcpListener>) {
        *self.retained_listener.lock().await = Some(RetainedListener::Tcp {
            stream_id,
            _listener: listener,
        });
    }

    pub(super) async fn replace_udp_bind(&self, stream_id: u32, socket: Arc<UdpSocket>) {
        *self.retained_udp_bind.lock().await = Some(RetainedUdpBind::Udp { stream_id, socket });
    }

    pub(super) async fn udp_socket(&self, stream_id: u32) -> Option<Arc<UdpSocket>> {
        match self.retained_udp_bind.lock().await.as_ref() {
            Some(RetainedUdpBind::Udp {
                stream_id: retained_stream_id,
                socket,
            }) if *retained_stream_id == stream_id => Some(socket.clone()),
            _ => None,
        }
    }

    pub(super) async fn udp_bind_snapshot(&self) -> Option<(u32, Arc<UdpSocket>)> {
        self.retained_udp_bind
            .lock()
            .await
            .as_ref()
            .map(|RetainedUdpBind::Udp { stream_id, socket }| (*stream_id, socket.clone()))
    }

    pub(super) async fn close_non_resumable_streams(&self) {
        if let Some(attachment) = self.attachment.lock().await.clone() {
            for (_, cancel) in attachment.stream_cancels.lock().await.drain() {
                cancel.cancel();
            }
            attachment.tcp_writers.lock().await.clear();
        }
    }

    pub(super) async fn close_retained_resources(&self) {
        *self.retained_listener.lock().await = None;
        *self.retained_udp_bind.lock().await = None;
        self.close_non_resumable_streams().await;
    }
}

pub(super) async fn explicit_session(tunnel: &Arc<TunnelState>) -> Option<Arc<SessionState>> {
    tunnel.attached_session.lock().await.clone()
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
            tcp_writers: Mutex::new(HashMap::new()),
            stream_cancels: Mutex::new(HashMap::new()),
        }));
    }
    *session.resume_deadline.lock().await = None;
    *tunnel.attached_session.lock().await = Some(session.clone());
    session.attachment_notify.notify_waiters();
    Ok(())
}

pub(super) async fn close_attached_session(tunnel: &Arc<TunnelState>, mode: SessionCloseMode) {
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

pub(super) fn close_mode_for_tunnel_result(result: &Result<(), HostRpcError>) -> SessionCloseMode {
    match result {
        Ok(()) => SessionCloseMode::RetryableDetach,
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

pub(super) fn schedule_session_expiry(store: TunnelSessionStore, session: Arc<SessionState>) {
    tokio::spawn(async move {
        tokio::time::sleep(RESUME_TIMEOUT).await;
        if session.is_expired().await && session.current_attachment().await.is_none() {
            store.remove(&session.id).await;
            session.close_retained_resources().await;
            session.root_cancel.cancel();
        }
    });
}

pub(super) async fn reactivate_retained_udp_bind(
    session: &Arc<SessionState>,
) -> Result<(), HostRpcError> {
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

pub(super) async fn send_tunnel_error_with_sender(
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
        generation: None,
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
