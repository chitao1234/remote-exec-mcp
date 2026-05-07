use remote_exec_proto::port_tunnel::Frame;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::HostRpcError;

use super::error::rpc_error;

pub(super) fn decode_frame_meta<T: DeserializeOwned>(frame: &Frame) -> Result<T, HostRpcError> {
    serde_json::from_slice(&frame.meta).map_err(|err| {
        rpc_error(
            "invalid_port_tunnel_metadata",
            format!("invalid port tunnel metadata: {err}"),
        )
    })
}

pub(super) fn encode_frame_meta<T: Serialize>(meta: &T) -> Result<Vec<u8>, HostRpcError> {
    serde_json::to_vec(meta).map_err(|err| {
        rpc_error(
            "invalid_port_tunnel_metadata",
            format!("invalid port tunnel metadata: {err}"),
        )
    })
}
