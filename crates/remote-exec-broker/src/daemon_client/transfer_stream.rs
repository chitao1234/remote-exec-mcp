use bytes::Bytes;
use futures_util::{Stream, StreamExt, TryStream, TryStreamExt};
use remote_exec_proto::rpc::{
    RpcErrorBody, RpcErrorCode, TRANSFER_STREAM_FRAME_HEADER_LEN, TRANSFER_STREAM_PREFACE,
    TransferStreamComplete, TransferStreamFrameType, decode_transfer_stream_frame_header,
    encode_transfer_stream_complete_frame, encode_transfer_stream_data_frame,
    parse_transfer_stream_complete_payload,
};
use reqwest::StatusCode;

use super::{DaemonClientError, DaemonRpcCode};

pub(crate) fn decode_response_body(
    response: reqwest::Response,
) -> impl Stream<Item = Result<Bytes, DaemonClientError>> {
    let decoder = TransferStreamDecoder::new(response.bytes_stream());
    futures_util::stream::try_unfold(decoder, |mut decoder| async move {
        match decoder.next_data_frame().await? {
            Some(bytes) => Ok(Some((bytes, decoder))),
            None => Ok(None),
        }
    })
}

pub(crate) fn encode_request_body<S, E>(stream: S) -> reqwest::Body
where
    S: TryStream<Ok = Bytes, Error = E> + Send + 'static,
    E: std::error::Error + Send + Sync + 'static,
{
    enum State {
        Preface {
            stream: futures_util::stream::BoxStream<'static, Result<Bytes, BoxError>>,
            archive_bytes: u64,
        },
        Data {
            stream: futures_util::stream::BoxStream<'static, Result<Bytes, BoxError>>,
            archive_bytes: u64,
        },
        Done,
    }

    let stream = stream.map_err(|err| -> BoxError { Box::new(err) }).boxed();
    let framed = futures_util::stream::try_unfold(
        State::Preface {
            stream,
            archive_bytes: 0,
        },
        |state| async move {
            match state {
                State::Preface {
                    stream,
                    archive_bytes,
                } => Ok::<Option<(Bytes, State)>, BoxError>(Some((
                    Bytes::copy_from_slice(TRANSFER_STREAM_PREFACE),
                    State::Data {
                        stream,
                        archive_bytes,
                    },
                ))),
                State::Data {
                    mut stream,
                    archive_bytes,
                } => loop {
                    match stream.try_next().await? {
                        Some(bytes) if bytes.is_empty() => {}
                        Some(bytes) => {
                            let next_archive_bytes =
                                archive_bytes.saturating_add(bytes.len() as u64);
                            return Ok::<Option<(Bytes, State)>, BoxError>(Some((
                                data_frame(&bytes),
                                State::Data {
                                    stream,
                                    archive_bytes: next_archive_bytes,
                                },
                            )));
                        }
                        None => {
                            return Ok::<Option<(Bytes, State)>, BoxError>(Some((
                                complete_frame(archive_bytes),
                                State::Done,
                            )));
                        }
                    }
                },
                State::Done => Ok::<Option<(Bytes, State)>, BoxError>(None),
            }
        },
    );
    reqwest::Body::wrap_stream(framed)
}

pub(crate) fn data_frame(payload: &[u8]) -> Bytes {
    Bytes::from(encode_transfer_stream_data_frame(payload))
}

pub(crate) fn complete_frame(archive_bytes: u64) -> Bytes {
    Bytes::from(encode_transfer_stream_complete_frame(archive_bytes))
}

type BoxError = Box<dyn std::error::Error + Send + Sync>;

struct TransferStreamDecoder<S> {
    stream: S,
    buffer: Vec<u8>,
    offset: usize,
    preface_read: bool,
    terminal: bool,
}

impl<S> TransferStreamDecoder<S> {
    fn new(stream: S) -> Self {
        Self {
            stream,
            buffer: Vec::new(),
            offset: 0,
            preface_read: false,
            terminal: false,
        }
    }
}

impl<S, E> TransferStreamDecoder<S>
where
    S: TryStream<Ok = Bytes, Error = E> + Unpin,
    E: std::error::Error + Send + Sync + 'static,
{
    async fn next_data_frame(&mut self) -> Result<Option<Bytes>, DaemonClientError> {
        if self.terminal {
            return Ok(None);
        }
        self.read_preface().await?;

        loop {
            let header_bytes = self
                .read_exact(
                    TRANSFER_STREAM_FRAME_HEADER_LEN,
                    "transfer stream frame header",
                )
                .await?;
            let header_array: [u8; TRANSFER_STREAM_FRAME_HEADER_LEN] = header_bytes
                .try_into()
                .expect("read_exact returned requested length");
            let header = decode_transfer_stream_frame_header(header_array)
                .map_err(|err| DaemonClientError::Decode(err.into()))?;
            let payload = self
                .read_exact(header.payload_len as usize, "transfer stream frame payload")
                .await?;

            match header.frame_type {
                TransferStreamFrameType::Data if payload.is_empty() => continue,
                TransferStreamFrameType::Data => return Ok(Some(Bytes::from(payload))),
                TransferStreamFrameType::Complete => {
                    parse_complete_payload(&payload)?;
                    self.terminal = true;
                    return Ok(None);
                }
                TransferStreamFrameType::Error => {
                    self.terminal = true;
                    return Err(error_payload_to_client_error(&payload));
                }
            }
        }
    }

    async fn read_preface(&mut self) -> Result<(), DaemonClientError> {
        if self.preface_read {
            return Ok(());
        }
        let preface = self
            .read_exact(TRANSFER_STREAM_PREFACE.len(), "transfer stream preface")
            .await?;
        if preface.as_slice() != TRANSFER_STREAM_PREFACE {
            return Err(DaemonClientError::Decode(anyhow::anyhow!(
                "daemon returned invalid transfer stream preface"
            )));
        }
        self.preface_read = true;
        Ok(())
    }

    async fn read_exact(
        &mut self,
        len: usize,
        label: &'static str,
    ) -> Result<Vec<u8>, DaemonClientError> {
        while self.available() < len {
            match self.stream.try_next().await {
                Ok(Some(chunk)) if !chunk.is_empty() => self.buffer.extend_from_slice(&chunk),
                Ok(Some(_)) => {}
                Ok(None) => {
                    return Err(DaemonClientError::Decode(anyhow::anyhow!(
                        "daemon transfer stream ended before {label}"
                    )));
                }
                Err(err) => return Err(DaemonClientError::Transport(anyhow::Error::new(err))),
            }
        }

        let start = self.offset;
        let end = start + len;
        let output = self.buffer[start..end].to_vec();
        self.offset = end;
        self.compact_buffer();
        Ok(output)
    }

    fn available(&self) -> usize {
        self.buffer.len().saturating_sub(self.offset)
    }

    fn compact_buffer(&mut self) {
        if self.offset == 0 {
            return;
        }
        if self.offset == self.buffer.len() {
            self.buffer.clear();
            self.offset = 0;
            return;
        }
        if self.offset >= 64 * 1024 {
            self.buffer.drain(..self.offset);
            self.offset = 0;
        }
    }
}

fn parse_complete_payload(payload: &[u8]) -> Result<TransferStreamComplete, DaemonClientError> {
    parse_transfer_stream_complete_payload(payload)
        .map_err(|err| DaemonClientError::Decode(err.into()))
}

fn error_payload_to_client_error(payload: &[u8]) -> DaemonClientError {
    match serde_json::from_slice::<RpcErrorBody>(payload) {
        Ok(body) => {
            let code = body
                .code()
                .map(DaemonRpcCode::Known)
                .or_else(|| Some(DaemonRpcCode::Unknown(body.wire_code().to_string())));
            DaemonClientError::Rpc {
                status: status_for_terminal_error(body.code()),
                code,
                message: body.message,
            }
        }
        Err(err) => DaemonClientError::Decode(
            anyhow::Error::from(err)
                .context("daemon returned malformed transfer stream error frame"),
        ),
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
    use remote_exec_proto::rpc::encode_transfer_stream_frame;

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
        let mut decoder = TransferStreamDecoder::new(futures_util::stream::iter(chunks));
        let mut output = Vec::new();
        while let Some(chunk) = decoder.next_data_frame().await? {
            output.extend_from_slice(&chunk);
        }
        Ok(output)
    }
}
