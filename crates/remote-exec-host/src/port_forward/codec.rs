use remote_exec_proto::port_tunnel::{
    Frame, decode_frame_meta as decode_port_tunnel_frame_meta,
    encode_frame_meta as encode_port_tunnel_frame_meta,
};
use remote_exec_proto::rpc::RpcErrorCode;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::HostRpcError;

use super::error::rpc_error;

pub(super) fn decode_frame_meta<T: DeserializeOwned>(frame: &Frame) -> Result<T, HostRpcError> {
    decode_port_tunnel_frame_meta(frame).map_err(|err| {
        rpc_error(
            RpcErrorCode::InvalidPortTunnelMetadata,
            format!("invalid port tunnel metadata: {err}"),
        )
    })
}

pub(super) fn encode_frame_meta<T: Serialize>(meta: &T) -> Result<Vec<u8>, HostRpcError> {
    encode_port_tunnel_frame_meta(meta).map_err(|err| {
        rpc_error(
            RpcErrorCode::InvalidPortTunnelMetadata,
            format!("invalid port tunnel metadata: {err}"),
        )
    })
}
