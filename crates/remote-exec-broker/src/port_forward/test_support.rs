use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::{Arc, Mutex as StdMutex};
use std::task::{Context as TaskContext, Poll, Waker};
use std::time::Duration;

use remote_exec_proto::port_forward::ForwardId;
use remote_exec_proto::port_tunnel::{Frame, FrameType, HEADER_LEN};
use remote_exec_proto::public::ForwardPortEntry;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use super::store::{PortForwardFilter, PortForwardRecord};
use super::supervisor::ForwardRuntime;
use super::tunnel::PortTunnel;

#[derive(Clone, Default)]
pub(super) struct ScriptedTunnelIo {
    state: Arc<StdMutex<ScriptedTunnelState>>,
}

#[derive(Default)]
struct ScriptedTunnelState {
    read_bytes: VecDeque<u8>,
    written_bytes: Vec<u8>,
    fail_writes: bool,
    read_waker: Option<Waker>,
}

impl ScriptedTunnelIo {
    pub(super) fn fail_writes(&self) {
        self.state.lock().unwrap().fail_writes = true;
    }

    pub(super) fn push_read_frame(&self, frame: &Frame) {
        let mut state = self.state.lock().unwrap();
        state.read_bytes.extend(frame_bytes(frame));
        if let Some(waker) = state.read_waker.take() {
            waker.wake();
        }
    }

    pub(super) async fn wait_for_written_frame(&self, frame_type: FrameType, stream_id: u32) {
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if self.pop_matching_written_frame(frame_type, stream_id) {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("expected tunnel frame should be written");
    }

    fn pop_matching_written_frame(&self, frame_type: FrameType, stream_id: u32) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.written_bytes.len() < HEADER_LEN {
            return false;
        }
        let meta_len = u32::from_be_bytes(
            state.written_bytes[8..12]
                .try_into()
                .expect("header slice length"),
        ) as usize;
        let data_len = u32::from_be_bytes(
            state.written_bytes[12..16]
                .try_into()
                .expect("header slice length"),
        ) as usize;
        let frame_len = HEADER_LEN + meta_len + data_len;
        if state.written_bytes.len() < frame_len {
            return false;
        }
        let written_frame_type = state.written_bytes[0];
        let written_stream_id = u32::from_be_bytes(
            state.written_bytes[4..8]
                .try_into()
                .expect("header slice length"),
        );
        assert_eq!(written_frame_type, frame_type as u8);
        assert_eq!(written_stream_id, stream_id);
        state.written_bytes.drain(..frame_len);
        true
    }
}

impl AsyncRead for ScriptedTunnelIo {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let mut state = self.state.lock().unwrap();
        if state.read_bytes.is_empty() {
            state.read_waker = Some(cx.waker().clone());
            return Poll::Pending;
        }
        let read = buf.remaining().min(state.read_bytes.len());
        let bytes: Vec<u8> = state.read_bytes.drain(..read).collect();
        buf.put_slice(&bytes);
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for ScriptedTunnelIo {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let mut state = self.state.lock().unwrap();
        if state.fail_writes {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "forced closed writer",
            )));
        }
        state.written_bytes.extend_from_slice(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut TaskContext<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut TaskContext<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

pub(super) fn filter_one(forward_id: &str) -> PortForwardFilter {
    PortForwardFilter {
        listen_side: None,
        connect_side: None,
        forward_ids: vec![ForwardId::new(forward_id)],
    }
}

pub(super) fn test_record(runtime: &ForwardRuntime, listen_endpoint: &str) -> PortForwardRecord {
    PortForwardRecord::new(
        ForwardPortEntry::new_open(
            runtime.forward_id().clone(),
            runtime.listen_side().name().to_string(),
            listen_endpoint.to_string(),
            runtime.connect_side().name().to_string(),
            runtime.connect_endpoint().to_string(),
            runtime.protocol(),
            runtime.limits.public_summary(),
        ),
        runtime.listen_session.clone(),
        runtime.cancel.clone(),
    )
}

pub(super) async fn wait_until_send_fails(tunnel: &PortTunnel) {
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let result = tunnel
                .send(Frame {
                    frame_type: FrameType::Close,
                    flags: 0,
                    stream_id: 99,
                    meta: Vec::new(),
                    data: Vec::new(),
                })
                .await;
            if result.is_err() {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("connect tunnel writer should close after forced write failure");
}

fn frame_bytes(frame: &Frame) -> Vec<u8> {
    let mut bytes = vec![0; HEADER_LEN];
    bytes[0] = frame.frame_type as u8;
    bytes[1] = frame.flags;
    bytes[4..8].copy_from_slice(&frame.stream_id.to_be_bytes());
    bytes[8..12].copy_from_slice(&(frame.meta.len() as u32).to_be_bytes());
    bytes[12..16].copy_from_slice(&(frame.data.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&frame.meta);
    bytes.extend_from_slice(&frame.data);
    bytes
}
