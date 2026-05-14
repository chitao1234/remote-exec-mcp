mod active;
mod codec;
mod error;
mod frames;
mod limiter;
mod session;
mod session_store;
mod tcp;
mod timings;
mod tunnel;
mod types;
mod udp;

use remote_exec_proto::port_tunnel::{ForwardDropKind, ForwardDropMeta, Frame, FrameType};
use remote_exec_proto::rpc::RpcErrorCode;

pub use session_store::TunnelSessionStore;
pub use tunnel::{reserve_tunnel_connection, serve_tunnel, serve_tunnel_with_permit};

pub use limiter::{PortForwardLimiter, PortForwardPermit};
use timings::timings;
use types::{
    ActiveTunnelState, ConnectRuntimeState, ConnectionLocalUdpBind, EndpointOkMeta, ErrorMeta,
    QueuedFrame, TcpStreamEntry, TcpWriteCommand, TcpWriterHandle, TunnelSender, TunnelState,
    UdpReaderEntry,
};

const READ_BUF_SIZE: usize = 64 * 1024;
const TCP_WRITE_QUEUE_FRAMES: usize = 8;

impl TunnelState {
    async fn send(&self, frame: Frame) -> Result<(), crate::HostRpcError> {
        self.tx.send(frame).await
    }
}

impl TunnelSender {
    async fn send(&self, frame: Frame) -> Result<(), crate::HostRpcError> {
        let permit = self.limiter.try_acquire_queued_frame(&frame)?;
        let queued = QueuedFrame {
            frame,
            _permit: permit,
        };
        self.tx.send(queued).await.map_err(|_| {
            error::rpc_error(
                RpcErrorCode::PortTunnelClosed,
                "port tunnel writer is closed",
            )
        })
    }
}

async fn send_forward_drop_report(
    tx: &TunnelSender,
    stream_id: u32,
    kind: ForwardDropKind,
    reason: impl Into<String>,
    message: impl Into<String>,
) -> Result<(), crate::HostRpcError> {
    tx.send(frames::meta_frame(
        FrameType::ForwardDrop,
        stream_id,
        &ForwardDropMeta {
            kind,
            count: 1,
            reason: reason.into(),
            message: Some(message.into()),
        },
    )?)
    .await
}

pub(super) fn tunnel_error_frame(
    stream_id: u32,
    code: impl Into<String>,
    message: impl Into<String>,
    fatal: bool,
    generation: Option<u64>,
) -> Result<Frame, crate::HostRpcError> {
    frames::meta_frame(
        FrameType::Error,
        stream_id,
        &ErrorMeta {
            code: code.into(),
            message: message.into(),
            fatal,
            generation,
        },
    )
}

#[cfg(test)]
mod port_tunnel_tests;
