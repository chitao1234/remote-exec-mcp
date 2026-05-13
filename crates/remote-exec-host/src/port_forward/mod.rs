mod codec;
mod error;
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
    ConnectionLocalUdpBind, EndpointOkMeta, ErrorMeta, QueuedFrame, TcpStreamEntry,
    TcpWriteCommand, TcpWriterHandle, TunnelMode, TunnelSender, TunnelState, UdpReaderEntry,
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
    let meta = serde_json::to_vec(&ForwardDropMeta {
        kind,
        count: 1,
        reason: reason.into(),
        message: Some(message.into()),
    })
    .map_err(|err| error::rpc_error(RpcErrorCode::InvalidPortTunnel, err.to_string()))?;
    tx.send(Frame {
        frame_type: FrameType::ForwardDrop,
        flags: 0,
        stream_id,
        meta,
        data: Vec::new(),
    })
    .await
}

pub(super) fn tunnel_error_frame(
    stream_id: u32,
    code: impl Into<String>,
    message: impl Into<String>,
    fatal: bool,
    generation: Option<u64>,
) -> Result<Frame, crate::HostRpcError> {
    let meta = codec::encode_frame_meta(&ErrorMeta {
        code: code.into(),
        message: message.into(),
        fatal,
        generation,
    })?;
    Ok(Frame {
        frame_type: FrameType::Error,
        flags: 0,
        stream_id,
        meta,
        data: Vec::new(),
    })
}

#[cfg(test)]
mod port_tunnel_tests;
