use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use remote_exec_proto::port_tunnel::Frame;
use remote_exec_proto::rpc::RpcErrorCode;

use crate::{HostPortForwardLimits, HostRpcError};

use super::error::rpc_error;
#[derive(Debug)]
pub struct PortForwardLimiter {
    limits: HostPortForwardLimits,
    tunnel_connections: AtomicUsize,
    retained_listeners: AtomicUsize,
    udp_binds: AtomicUsize,
    active_tcp_streams: AtomicUsize,
    queued_bytes: AtomicUsize,
}

pub struct PortForwardPermit {
    limiter: Arc<PortForwardLimiter>,
    kind: PermitKind,
    amount: usize,
}

#[derive(Debug, Clone, Copy)]
enum PermitKind {
    TunnelConnection,
    RetainedListener,
    UdpBind,
    ActiveTcpStream,
    QueuedBytes,
}

impl PortForwardLimiter {
    pub fn new(limits: HostPortForwardLimits) -> Self {
        Self {
            limits,
            tunnel_connections: AtomicUsize::new(0),
            retained_listeners: AtomicUsize::new(0),
            udp_binds: AtomicUsize::new(0),
            active_tcp_streams: AtomicUsize::new(0),
            queued_bytes: AtomicUsize::new(0),
        }
    }

    pub(super) fn try_acquire_tunnel_connection(
        self: &Arc<Self>,
    ) -> Result<PortForwardPermit, HostRpcError> {
        self.try_acquire_counter(
            &self.tunnel_connections,
            self.limits.max_tunnel_connections,
            1,
            PermitKind::TunnelConnection,
            "port tunnel connection limit reached",
        )
    }

    pub(super) fn try_acquire_retained_listener(
        self: &Arc<Self>,
    ) -> Result<PortForwardPermit, HostRpcError> {
        self.try_acquire_counter(
            &self.retained_listeners,
            self.limits.max_retained_listeners,
            1,
            PermitKind::RetainedListener,
            "retained port tunnel listener limit reached",
        )
    }

    pub(super) fn try_acquire_udp_bind(
        self: &Arc<Self>,
    ) -> Result<PortForwardPermit, HostRpcError> {
        self.try_acquire_counter(
            &self.udp_binds,
            self.limits.max_udp_binds,
            1,
            PermitKind::UdpBind,
            "port tunnel udp bind limit reached",
        )
    }

    pub(super) fn try_acquire_active_tcp_stream(
        self: &Arc<Self>,
    ) -> Result<PortForwardPermit, HostRpcError> {
        self.try_acquire_counter(
            &self.active_tcp_streams,
            self.limits.max_active_tcp_streams,
            1,
            PermitKind::ActiveTcpStream,
            "port tunnel active tcp stream limit reached",
        )
    }

    pub(super) fn try_acquire_queued_frame(
        self: &Arc<Self>,
        frame: &Frame,
    ) -> Result<Option<PortForwardPermit>, HostRpcError> {
        let charge = frame.data_plane_charge();
        if charge == 0 {
            return Ok(None);
        }
        self.try_acquire_counter(
            &self.queued_bytes,
            self.limits.max_tunnel_queued_bytes,
            charge,
            PermitKind::QueuedBytes,
            "port tunnel queued byte limit reached",
        )
        .map(Some)
    }

    fn try_acquire_counter(
        self: &Arc<Self>,
        counter: &AtomicUsize,
        limit: usize,
        amount: usize,
        kind: PermitKind,
        message: &'static str,
    ) -> Result<PortForwardPermit, HostRpcError> {
        let mut current = counter.load(Ordering::Relaxed);
        loop {
            let Some(next) = current.checked_add(amount) else {
                return Err(limit_error(message));
            };
            if next > limit {
                return Err(limit_error(message));
            }
            match counter.compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Relaxed)
            {
                Ok(_) => {
                    return Ok(PortForwardPermit {
                        limiter: self.clone(),
                        kind,
                        amount,
                    });
                }
                Err(observed) => current = observed,
            }
        }
    }

    fn release(&self, kind: PermitKind, amount: usize) {
        match kind {
            PermitKind::TunnelConnection => {
                self.tunnel_connections.fetch_sub(amount, Ordering::AcqRel);
            }
            PermitKind::RetainedListener => {
                self.retained_listeners.fetch_sub(amount, Ordering::AcqRel);
            }
            PermitKind::UdpBind => {
                self.udp_binds.fetch_sub(amount, Ordering::AcqRel);
            }
            PermitKind::ActiveTcpStream => {
                self.active_tcp_streams.fetch_sub(amount, Ordering::AcqRel);
            }
            PermitKind::QueuedBytes => {
                self.queued_bytes.fetch_sub(amount, Ordering::AcqRel);
            }
        }
    }
}

impl Drop for PortForwardPermit {
    fn drop(&mut self) {
        self.limiter.release(self.kind, self.amount);
    }
}

fn limit_error(message: &'static str) -> HostRpcError {
    rpc_error(RpcErrorCode::PortTunnelLimitExceeded, message)
}
