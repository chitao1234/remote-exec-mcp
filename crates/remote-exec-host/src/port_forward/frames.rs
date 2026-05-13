use remote_exec_proto::port_tunnel::{Frame, FrameType};
use serde::Serialize;

use crate::HostRpcError;

use super::EndpointOkMeta;
use super::codec::encode_frame_meta;

pub(super) fn frame(frame_type: FrameType, stream_id: u32, meta: Vec<u8>, data: Vec<u8>) -> Frame {
    Frame {
        frame_type,
        flags: 0,
        stream_id,
        meta,
        data,
    }
}

pub(super) fn empty_frame(frame_type: FrameType, stream_id: u32) -> Frame {
    frame(frame_type, stream_id, Vec::new(), Vec::new())
}

pub(super) fn data_frame(frame_type: FrameType, stream_id: u32, data: Vec<u8>) -> Frame {
    frame(frame_type, stream_id, Vec::new(), data)
}

pub(super) fn meta_frame<T: Serialize>(
    frame_type: FrameType,
    stream_id: u32,
    meta: &T,
) -> Result<Frame, HostRpcError> {
    Ok(frame(
        frame_type,
        stream_id,
        encode_frame_meta(meta)?,
        Vec::new(),
    ))
}

pub(super) fn endpoint_ok_frame(
    frame_type: FrameType,
    stream_id: u32,
    endpoint: impl Into<String>,
) -> Result<Frame, HostRpcError> {
    meta_frame(
        frame_type,
        stream_id,
        &EndpointOkMeta {
            endpoint: endpoint.into(),
        },
    )
}
