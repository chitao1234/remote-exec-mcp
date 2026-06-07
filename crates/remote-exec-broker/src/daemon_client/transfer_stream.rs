use bytes::Bytes;
use futures_util::{Stream, TryStream, TryStreamExt};
use remote_exec_proto::rpc::{
    RpcErrorCode, TransferStreamDecodeError, decode_transfer_stream_body,
    encode_transfer_stream_body,
};
use reqwest::StatusCode;

use super::{DaemonClientError, DaemonRpcCode};

pub(crate) fn decode_response_body(
    response: reqwest::Response,
) -> impl Stream<Item = Result<Bytes, DaemonClientError>> {
    decode_transfer_stream_body(response.bytes_stream()).map_err(decode_error_to_client_error)
}

pub(crate) fn encode_request_body<S, E>(stream: S) -> reqwest::Body
where
    S: TryStream<Ok = Bytes, Error = E> + Send + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    reqwest::Body::wrap_stream(encode_transfer_stream_body(stream))
}

fn decode_error_to_client_error<E>(err: TransferStreamDecodeError<E>) -> DaemonClientError
where
    E: std::error::Error + Send + Sync + 'static,
{
    match err {
        TransferStreamDecodeError::Transport(err) => {
            DaemonClientError::Transport(anyhow::Error::new(err))
        }
        TransferStreamDecodeError::Invalid(message) => {
            DaemonClientError::Decode(anyhow::anyhow!("daemon returned {message}"))
        }
        TransferStreamDecodeError::MalformedComplete(err) => DaemonClientError::Decode(err.into()),
        TransferStreamDecodeError::MalformedError(err) => DaemonClientError::Decode(
            anyhow::Error::from(err)
                .context("daemon returned malformed transfer stream error frame"),
        ),
        TransferStreamDecodeError::ErrorFrame { code, message } => {
            let code = DaemonRpcCode::from_wire_value(code);
            let status = status_for_terminal_error(code.known());
            DaemonClientError::Rpc {
                status,
                code: Some(code),
                message,
            }
        }
    }
}

fn status_for_terminal_error(code: Option<RpcErrorCode>) -> StatusCode {
    match code {
        Some(RpcErrorCode::Internal | RpcErrorCode::TransferFailed) | None => {
            StatusCode::INTERNAL_SERVER_ERROR
        }
        _ => StatusCode::BAD_REQUEST,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::pin_mut;
    use remote_exec_proto::rpc::{
        RpcErrorBody, TRANSFER_STREAM_PREFACE, TransferStreamComplete, TransferStreamFrameType,
        encode_transfer_stream_frame,
    };

    fn frame(frame_type: TransferStreamFrameType, payload: &[u8]) -> Bytes {
        Bytes::from(encode_transfer_stream_frame(frame_type, payload))
    }

    fn complete_payload() -> Vec<u8> {
        serde_json::to_vec(&TransferStreamComplete { archive_bytes: 3 }).unwrap()
    }

    #[tokio::test]
    async fn decoder_yields_data_until_complete() {
        let chunks = vec![
            Ok::<_, std::io::Error>(Bytes::from_static(TRANSFER_STREAM_PREFACE)),
            Ok(frame(TransferStreamFrameType::Data, b"abc")),
            Ok(frame(
                TransferStreamFrameType::Complete,
                &complete_payload(),
            )),
        ];
        let output = decode_stream_for_test(chunks).await.unwrap();

        assert_eq!(output, b"abc");
    }

    #[tokio::test]
    async fn decoder_rejects_eof_before_terminal_frame() {
        let chunks = vec![
            Ok::<_, std::io::Error>(Bytes::from_static(TRANSFER_STREAM_PREFACE)),
            Ok(frame(TransferStreamFrameType::Data, b"abc")),
        ];
        let err = decode_stream_for_test(chunks).await.unwrap_err();

        assert!(
            err.to_string()
                .contains("ended before transfer stream frame header")
        );
    }

    #[tokio::test]
    async fn decoder_maps_error_frame_to_rpc_error() {
        let error = RpcErrorBody::new(RpcErrorCode::TransferFailed, "source changed");
        let chunks = vec![
            Ok::<_, std::io::Error>(Bytes::from_static(TRANSFER_STREAM_PREFACE)),
            Ok(frame(TransferStreamFrameType::Data, b"abc")),
            Ok(frame(
                TransferStreamFrameType::Error,
                &serde_json::to_vec(&error).unwrap(),
            )),
        ];
        let err = decode_stream_for_test(chunks).await.unwrap_err();

        assert!(matches!(
            err,
            DaemonClientError::Rpc {
                code: Some(DaemonRpcCode::Known(RpcErrorCode::TransferFailed)),
                ..
            }
        ));
    }

    async fn decode_stream_for_test(
        chunks: Vec<Result<Bytes, std::io::Error>>,
    ) -> Result<Vec<u8>, DaemonClientError> {
        decode_response_stream_for_test(chunks).await
    }

    async fn decode_response_stream_for_test(
        chunks: Vec<Result<Bytes, std::io::Error>>,
    ) -> Result<Vec<u8>, DaemonClientError> {
        let mut output = Vec::new();
        let stream = decode_transfer_stream_body(futures_util::stream::iter(chunks))
            .map_err(decode_error_to_client_error);
        pin_mut!(stream);
        while let Some(chunk) = stream.try_next().await? {
            output.extend_from_slice(&chunk);
        }
        Ok(output)
    }
}
