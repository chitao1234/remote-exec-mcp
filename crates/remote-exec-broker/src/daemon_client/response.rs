use remote_exec_proto::rpc::RpcErrorBody;

use super::{DaemonClientError, DaemonRpcCode};

#[derive(Clone, Copy)]
pub(in crate::daemon_client) enum RpcErrorDecodePolicy {
    Strict,
    Lenient,
}

pub(in crate::daemon_client) async fn decode_rpc_error(
    response: reqwest::Response,
    decode_policy: RpcErrorDecodePolicy,
) -> DaemonClientError {
    let status = response.status();
    match response.text().await {
        Ok(body) => decode_rpc_error_body(status, body),
        Err(err) => match decode_policy {
            RpcErrorDecodePolicy::Strict => DaemonClientError::Transport(err.into()),
            RpcErrorDecodePolicy::Lenient => decode_rpc_error_body(status, err.to_string()),
        },
    }
}

pub(in crate::daemon_client) fn decode_rpc_error_body(
    status: reqwest::StatusCode,
    body: String,
) -> DaemonClientError {
    if let Ok(error) = serde_json::from_str::<RpcErrorBody>(&body) {
        DaemonClientError::Rpc {
            status,
            code: Some(DaemonRpcCode::from_wire_value(error.wire_code())),
            message: error.message,
        }
    } else {
        DaemonClientError::Rpc {
            status,
            code: None,
            message: body,
        }
    }
}
