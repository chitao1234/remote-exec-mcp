use bytes::{Bytes, BytesMut};
use futures_util::{Stream, StreamExt, TryStream, TryStreamExt};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const TRANSFER_STREAM_PROTOCOL_VERSION: u32 = 2;
pub const TRANSFER_STREAM_VERSION_HEADER: &str = "x-remote-exec-transfer-stream-version";
pub const TRANSFER_STREAM_CONTENT_TYPE: &str = "application/vnd.remote-exec.transfer-stream.v2";
pub const TRANSFER_STREAM_PREFACE: &[u8; 8] = b"REXFER2\n";
pub const TRANSFER_STREAM_FRAME_HEADER_LEN: usize = 12;
pub const TRANSFER_STREAM_DATA_FRAME_MAX_BYTES: u64 = 64 * 1024;
pub const TRANSFER_STREAM_CONTROL_FRAME_MAX_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TransferStreamFrameType {
    Data = 0x01,
    Complete = 0x02,
    Error = 0x03,
}

impl TransferStreamFrameType {
    pub fn from_byte(value: u8) -> Option<Self> {
        match value {
            0x01 => Some(Self::Data),
            0x02 => Some(Self::Complete),
            0x03 => Some(Self::Error),
            _ => None,
        }
    }

    pub fn as_byte(self) -> u8 {
        self as u8
    }

    pub fn payload_limit(self) -> u64 {
        match self {
            Self::Data => TRANSFER_STREAM_DATA_FRAME_MAX_BYTES,
            Self::Complete | Self::Error => TRANSFER_STREAM_CONTROL_FRAME_MAX_BYTES,
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Complete | Self::Error)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferStreamFrameHeader {
    pub frame_type: TransferStreamFrameType,
    pub payload_len: u64,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum TransferStreamFrameDecodeError {
    #[error("transfer stream frame type `{0}` is unknown")]
    UnknownFrameType(u8),
    #[error("transfer stream frame flags must be zero")]
    NonZeroFlags(u8),
    #[error("transfer stream frame reserved field must be zero")]
    NonZeroReserved(u16),
    #[error("transfer stream frame payload length {payload_len} exceeds limit {limit}")]
    PayloadTooLarge { payload_len: u64, limit: u64 },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransferStreamComplete {
    pub archive_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferStreamTerminalFrame {
    Complete(TransferStreamComplete),
    Error(super::super::RpcErrorBody),
}

#[derive(Debug, Error)]
pub enum TransferStreamDecodeError<E>
where
    E: std::error::Error + Send + Sync + 'static,
{
    #[error("transfer stream transport error: {0}")]
    Transport(E),
    #[error("{0}")]
    Invalid(String),
    #[error("malformed transfer stream complete frame: {0}")]
    MalformedComplete(serde_json::Error),
    #[error("malformed transfer stream error frame: {0}")]
    MalformedError(serde_json::Error),
    #[error("transfer stream error {code}: {message}")]
    ErrorFrame { code: String, message: String },
}

impl<E> TransferStreamDecodeError<E>
where
    E: std::error::Error + Send + Sync + 'static,
{
    pub fn error_frame_body(&self) -> Option<super::super::RpcErrorBody> {
        match self {
            Self::ErrorFrame { code, message } => {
                Some(super::super::RpcErrorBody::from_raw_code(code, message))
            }
            _ => None,
        }
    }
}

pub fn encode_transfer_stream_frame_header(header: TransferStreamFrameHeader) -> [u8; 12] {
    let mut output = [0_u8; TRANSFER_STREAM_FRAME_HEADER_LEN];
    output[0] = header.frame_type.as_byte();
    output[1] = 0;
    output[2..4].copy_from_slice(&0_u16.to_be_bytes());
    output[4..12].copy_from_slice(&header.payload_len.to_be_bytes());
    output
}

pub fn encode_transfer_stream_frame(
    frame_type: TransferStreamFrameType,
    payload: &[u8],
) -> Vec<u8> {
    let header = encode_transfer_stream_frame_header(TransferStreamFrameHeader {
        frame_type,
        payload_len: payload.len() as u64,
    });
    let mut output = Vec::with_capacity(header.len() + payload.len());
    output.extend_from_slice(&header);
    output.extend_from_slice(payload);
    output
}

pub fn encode_transfer_stream_data_frame(payload: &[u8]) -> Vec<u8> {
    encode_transfer_stream_frame(TransferStreamFrameType::Data, payload)
}

pub fn encode_transfer_stream_complete_frame(archive_bytes: u64) -> Vec<u8> {
    let payload = serde_json::to_vec(&TransferStreamComplete { archive_bytes })
        .expect("transfer complete payload serializes");
    encode_transfer_stream_frame(TransferStreamFrameType::Complete, &payload)
}

pub fn encode_transfer_stream_error_frame(error: &super::super::RpcErrorBody) -> Vec<u8> {
    let payload = serde_json::to_vec(error).expect("transfer error payload serializes");
    encode_transfer_stream_frame(TransferStreamFrameType::Error, &payload)
}

pub fn parse_transfer_stream_complete_payload(
    payload: &[u8],
) -> Result<TransferStreamComplete, serde_json::Error> {
    serde_json::from_slice(payload)
}

pub fn parse_transfer_stream_error_payload(
    payload: &[u8],
) -> Result<super::super::RpcErrorBody, serde_json::Error> {
    serde_json::from_slice(payload)
}

pub fn transfer_stream_data_frame(payload: impl AsRef<[u8]>) -> Bytes {
    Bytes::from(encode_transfer_stream_data_frame(payload.as_ref()))
}

fn transfer_stream_data_frame_header(payload_len: usize) -> Bytes {
    Bytes::copy_from_slice(&encode_transfer_stream_frame_header(
        TransferStreamFrameHeader {
            frame_type: TransferStreamFrameType::Data,
            payload_len: payload_len as u64,
        },
    ))
}

pub fn transfer_stream_complete_frame(archive_bytes: u64) -> Bytes {
    Bytes::from(encode_transfer_stream_complete_frame(archive_bytes))
}

pub fn transfer_stream_error_frame(error: &super::super::RpcErrorBody) -> Bytes {
    Bytes::from(encode_transfer_stream_error_frame(error))
}

type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub fn encode_transfer_stream_body<S, E>(stream: S) -> impl Stream<Item = Result<Bytes, BoxError>>
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
        DataPayload {
            stream: futures_util::stream::BoxStream<'static, Result<Bytes, BoxError>>,
            archive_bytes: u64,
            payload: Bytes,
        },
        Done,
    }

    let stream = stream.map_err(|err| -> BoxError { Box::new(err) }).boxed();
    futures_util::stream::try_unfold(
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
                                transfer_stream_data_frame_header(bytes.len()),
                                State::DataPayload {
                                    stream,
                                    archive_bytes: next_archive_bytes,
                                    payload: bytes,
                                },
                            )));
                        }
                        None => {
                            return Ok::<Option<(Bytes, State)>, BoxError>(Some((
                                transfer_stream_complete_frame(archive_bytes),
                                State::Done,
                            )));
                        }
                    }
                },
                State::DataPayload {
                    stream,
                    archive_bytes,
                    payload,
                } => Ok::<Option<(Bytes, State)>, BoxError>(Some((
                    payload,
                    State::Data {
                        stream,
                        archive_bytes,
                    },
                ))),
                State::Done => Ok::<Option<(Bytes, State)>, BoxError>(None),
            }
        },
    )
}

pub fn encode_transfer_export_item_stream<S, E, F>(
    stream: S,
    map_error: F,
) -> impl Stream<Item = Result<Bytes, std::convert::Infallible>>
where
    S: Stream<Item = TransferStreamExportItem<E>> + Send + 'static,
    E: Send + 'static,
    F: Fn(E) -> super::super::RpcErrorBody + Clone + Send + 'static,
{
    enum State<E> {
        Preface(futures_util::stream::BoxStream<'static, TransferStreamExportItem<E>>),
        Items(futures_util::stream::BoxStream<'static, TransferStreamExportItem<E>>),
        DataPayload {
            stream: futures_util::stream::BoxStream<'static, TransferStreamExportItem<E>>,
            payload: Bytes,
        },
        Done,
    }

    let stream = stream.boxed();

    futures_util::stream::unfold(State::Preface(stream), move |state| {
        let map_error = map_error.clone();
        async move {
            match state {
                State::Preface(stream) => Some((
                    Ok(Bytes::copy_from_slice(TRANSFER_STREAM_PREFACE)),
                    State::Items(stream),
                )),
                State::Items(mut stream) => match stream.next().await {
                    Some(TransferStreamExportItem::Data(bytes)) => Some((
                        Ok(transfer_stream_data_frame_header(bytes.len())),
                        State::DataPayload {
                            stream,
                            payload: bytes,
                        },
                    )),
                    Some(TransferStreamExportItem::Complete { archive_bytes }) => Some((
                        Ok(transfer_stream_complete_frame(archive_bytes)),
                        State::Done,
                    )),
                    Some(TransferStreamExportItem::Error(err)) => Some((
                        Ok(transfer_stream_error_frame(&map_error(err))),
                        State::Done,
                    )),
                    None => Some((
                        Ok(transfer_stream_error_frame(
                            &super::super::RpcErrorBody::new(
                                super::super::RpcErrorCode::Internal,
                                "transfer export stream ended before terminal state",
                            ),
                        )),
                        State::Done,
                    )),
                },
                State::DataPayload { stream, payload } => Some((Ok(payload), State::Items(stream))),
                State::Done => None,
            }
        }
    })
}

pub enum TransferStreamExportItem<E> {
    Data(Bytes),
    Complete { archive_bytes: u64 },
    Error(E),
}

pub fn decode_transfer_stream_body<S, E>(
    stream: S,
) -> impl Stream<Item = Result<Bytes, TransferStreamDecodeError<E>>>
where
    S: TryStream<Ok = Bytes, Error = E> + Unpin,
    E: std::error::Error + Send + Sync + 'static,
{
    let decoder = TransferStreamDecoder::new(stream);
    futures_util::stream::try_unfold(decoder, |mut decoder| async move {
        match decoder.next_data_frame().await? {
            Some(bytes) => Ok(Some((bytes, decoder))),
            None => Ok(None),
        }
    })
}

pub struct TransferStreamDecoder<S> {
    stream: S,
    buffer: BytesMut,
    preface_read: bool,
    terminal: bool,
}

impl<S> TransferStreamDecoder<S> {
    pub fn new(stream: S) -> Self {
        Self {
            stream,
            buffer: BytesMut::new(),
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
    pub async fn next_data_frame(&mut self) -> Result<Option<Bytes>, TransferStreamDecodeError<E>> {
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
                .as_ref()
                .try_into()
                .expect("read_exact returned requested length");
            let header = decode_transfer_stream_frame_header(header_array)
                .map_err(|err| TransferStreamDecodeError::Invalid(err.to_string()))?;
            let payload = self
                .read_exact(header.payload_len as usize, "transfer stream frame payload")
                .await?;

            match header.frame_type {
                TransferStreamFrameType::Data if payload.is_empty() => continue,
                TransferStreamFrameType::Data => return Ok(Some(payload)),
                TransferStreamFrameType::Complete => {
                    self.parse_terminal_payload(TransferStreamFrameType::Complete, &payload)?;
                    self.terminal = true;
                    return Ok(None);
                }
                TransferStreamFrameType::Error => {
                    self.terminal = true;
                    return Err(error_frame_to_decode_error(&payload));
                }
            }
        }
    }

    pub async fn next_frame(
        &mut self,
    ) -> Result<Option<TransferStreamFrame>, TransferStreamDecodeError<E>> {
        if self.terminal {
            return Ok(None);
        }
        self.read_preface().await?;

        let header_bytes = self
            .read_exact(
                TRANSFER_STREAM_FRAME_HEADER_LEN,
                "transfer stream frame header",
            )
            .await?;
        let header_array: [u8; TRANSFER_STREAM_FRAME_HEADER_LEN] = header_bytes
            .as_ref()
            .try_into()
            .expect("read_exact returned requested length");
        let header = decode_transfer_stream_frame_header(header_array)
            .map_err(|err| TransferStreamDecodeError::Invalid(err.to_string()))?;
        let payload = self
            .read_exact(header.payload_len as usize, "transfer stream frame payload")
            .await?;

        match header.frame_type {
            TransferStreamFrameType::Data => Ok(Some(TransferStreamFrame::Data(payload))),
            TransferStreamFrameType::Complete => {
                let complete = parse_transfer_stream_complete_payload(&payload)
                    .map_err(TransferStreamDecodeError::MalformedComplete)?;
                self.terminal = true;
                Ok(Some(TransferStreamFrame::Complete(complete)))
            }
            TransferStreamFrameType::Error => {
                let error = parse_transfer_stream_error_payload(&payload)
                    .map_err(TransferStreamDecodeError::MalformedError)?;
                self.terminal = true;
                Ok(Some(TransferStreamFrame::Error(error)))
            }
        }
    }

    async fn read_preface(&mut self) -> Result<(), TransferStreamDecodeError<E>> {
        if self.preface_read {
            return Ok(());
        }
        let preface = self
            .read_exact(TRANSFER_STREAM_PREFACE.len(), "transfer stream preface")
            .await?;
        if preface.as_ref() != TRANSFER_STREAM_PREFACE {
            return Err(TransferStreamDecodeError::Invalid(
                "invalid transfer stream preface".to_string(),
            ));
        }
        self.preface_read = true;
        Ok(())
    }

    async fn read_exact(
        &mut self,
        len: usize,
        label: &'static str,
    ) -> Result<Bytes, TransferStreamDecodeError<E>> {
        while self.available() < len {
            match self.stream.try_next().await {
                Ok(Some(chunk)) if !chunk.is_empty() => self.buffer.extend_from_slice(&chunk),
                Ok(Some(_)) => {}
                Ok(None) => {
                    return Err(TransferStreamDecodeError::Invalid(format!(
                        "transfer stream ended before {label}"
                    )));
                }
                Err(err) => return Err(TransferStreamDecodeError::Transport(err)),
            }
        }

        Ok(self.buffer.split_to(len).freeze())
    }

    fn available(&self) -> usize {
        self.buffer.len()
    }

    fn parse_terminal_payload(
        &self,
        frame_type: TransferStreamFrameType,
        payload: &[u8],
    ) -> Result<TransferStreamTerminalFrame, TransferStreamDecodeError<E>> {
        parse_transfer_stream_terminal_payload(frame_type, payload).map_err(|err| match err {
            TransferStreamTerminalDecodeError::MalformedComplete(err) => {
                TransferStreamDecodeError::MalformedComplete(err)
            }
            TransferStreamTerminalDecodeError::MalformedError(err) => {
                TransferStreamDecodeError::MalformedError(err)
            }
            TransferStreamTerminalDecodeError::NonTerminalFrame => {
                TransferStreamDecodeError::Invalid("terminal frame expected".to_string())
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferStreamFrame {
    Data(Bytes),
    Complete(TransferStreamComplete),
    Error(super::super::RpcErrorBody),
}

#[derive(Debug, Error)]
pub enum TransferStreamTerminalDecodeError {
    #[error("malformed transfer stream complete frame: {0}")]
    MalformedComplete(serde_json::Error),
    #[error("malformed transfer stream error frame: {0}")]
    MalformedError(serde_json::Error),
    #[error("terminal frame expected")]
    NonTerminalFrame,
}

pub fn parse_transfer_stream_terminal_payload(
    frame_type: TransferStreamFrameType,
    payload: &[u8],
) -> Result<TransferStreamTerminalFrame, TransferStreamTerminalDecodeError> {
    match frame_type {
        TransferStreamFrameType::Complete => parse_transfer_stream_complete_payload(payload)
            .map(TransferStreamTerminalFrame::Complete)
            .map_err(TransferStreamTerminalDecodeError::MalformedComplete),
        TransferStreamFrameType::Error => parse_transfer_stream_error_payload(payload)
            .map(TransferStreamTerminalFrame::Error)
            .map_err(TransferStreamTerminalDecodeError::MalformedError),
        TransferStreamFrameType::Data => Err(TransferStreamTerminalDecodeError::NonTerminalFrame),
    }
}

fn error_frame_to_decode_error<E>(payload: &[u8]) -> TransferStreamDecodeError<E>
where
    E: std::error::Error + Send + Sync + 'static,
{
    match parse_transfer_stream_error_payload(payload) {
        Ok(error) => TransferStreamDecodeError::ErrorFrame {
            code: error.wire_code().to_string(),
            message: error.message,
        },
        Err(err) => TransferStreamDecodeError::MalformedError(err),
    }
}

pub fn decode_transfer_stream_frame_header(
    bytes: [u8; TRANSFER_STREAM_FRAME_HEADER_LEN],
) -> Result<TransferStreamFrameHeader, TransferStreamFrameDecodeError> {
    let frame_type = TransferStreamFrameType::from_byte(bytes[0])
        .ok_or(TransferStreamFrameDecodeError::UnknownFrameType(bytes[0]))?;
    if bytes[1] != 0 {
        return Err(TransferStreamFrameDecodeError::NonZeroFlags(bytes[1]));
    }

    let reserved = u16::from_be_bytes([bytes[2], bytes[3]]);
    if reserved != 0 {
        return Err(TransferStreamFrameDecodeError::NonZeroReserved(reserved));
    }

    let payload_len = u64::from_be_bytes([
        bytes[4], bytes[5], bytes[6], bytes[7], bytes[8], bytes[9], bytes[10], bytes[11],
    ]);
    let limit = frame_type.payload_limit();
    if payload_len > limit {
        return Err(TransferStreamFrameDecodeError::PayloadTooLarge { payload_len, limit });
    }

    Ok(TransferStreamFrameHeader {
        frame_type,
        payload_len,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_header_round_trips_data() {
        let header = TransferStreamFrameHeader {
            frame_type: TransferStreamFrameType::Data,
            payload_len: 42,
        };

        let encoded = encode_transfer_stream_frame_header(header);
        let decoded = decode_transfer_stream_frame_header(encoded).unwrap();

        assert_eq!(decoded, header);
    }

    #[test]
    fn data_frame_encodes_header_and_payload() {
        let encoded = encode_transfer_stream_data_frame(b"abc");
        let header: [u8; TRANSFER_STREAM_FRAME_HEADER_LEN] = encoded
            [..TRANSFER_STREAM_FRAME_HEADER_LEN]
            .try_into()
            .unwrap();

        assert_eq!(
            decode_transfer_stream_frame_header(header).unwrap(),
            TransferStreamFrameHeader {
                frame_type: TransferStreamFrameType::Data,
                payload_len: 3,
            }
        );
        assert_eq!(&encoded[TRANSFER_STREAM_FRAME_HEADER_LEN..], b"abc");
    }

    #[test]
    fn complete_frame_payload_round_trips() {
        let encoded = encode_transfer_stream_complete_frame(42);
        let header: [u8; TRANSFER_STREAM_FRAME_HEADER_LEN] = encoded
            [..TRANSFER_STREAM_FRAME_HEADER_LEN]
            .try_into()
            .unwrap();
        let decoded = decode_transfer_stream_frame_header(header).unwrap();

        assert_eq!(decoded.frame_type, TransferStreamFrameType::Complete);
        assert_eq!(
            parse_transfer_stream_complete_payload(&encoded[TRANSFER_STREAM_FRAME_HEADER_LEN..])
                .unwrap(),
            TransferStreamComplete { archive_bytes: 42 }
        );
    }

    #[test]
    fn frame_header_rejects_unknown_type() {
        let mut encoded = encode_transfer_stream_frame_header(TransferStreamFrameHeader {
            frame_type: TransferStreamFrameType::Data,
            payload_len: 0,
        });
        encoded[0] = 0x7f;

        assert_eq!(
            decode_transfer_stream_frame_header(encoded),
            Err(TransferStreamFrameDecodeError::UnknownFrameType(0x7f))
        );
    }

    #[test]
    fn frame_header_rejects_non_zero_flags() {
        let mut encoded = encode_transfer_stream_frame_header(TransferStreamFrameHeader {
            frame_type: TransferStreamFrameType::Data,
            payload_len: 0,
        });
        encoded[1] = 1;

        assert_eq!(
            decode_transfer_stream_frame_header(encoded),
            Err(TransferStreamFrameDecodeError::NonZeroFlags(1))
        );
    }

    #[test]
    fn frame_header_rejects_non_zero_reserved() {
        let mut encoded = encode_transfer_stream_frame_header(TransferStreamFrameHeader {
            frame_type: TransferStreamFrameType::Data,
            payload_len: 0,
        });
        encoded[2..4].copy_from_slice(&7_u16.to_be_bytes());

        assert_eq!(
            decode_transfer_stream_frame_header(encoded),
            Err(TransferStreamFrameDecodeError::NonZeroReserved(7))
        );
    }

    #[test]
    fn frame_header_rejects_oversized_data_payload() {
        let encoded = encode_transfer_stream_frame_header(TransferStreamFrameHeader {
            frame_type: TransferStreamFrameType::Data,
            payload_len: TRANSFER_STREAM_DATA_FRAME_MAX_BYTES + 1,
        });

        assert_eq!(
            decode_transfer_stream_frame_header(encoded),
            Err(TransferStreamFrameDecodeError::PayloadTooLarge {
                payload_len: TRANSFER_STREAM_DATA_FRAME_MAX_BYTES + 1,
                limit: TRANSFER_STREAM_DATA_FRAME_MAX_BYTES,
            })
        );
    }

    #[test]
    fn frame_header_rejects_oversized_control_payload() {
        let encoded = encode_transfer_stream_frame_header(TransferStreamFrameHeader {
            frame_type: TransferStreamFrameType::Complete,
            payload_len: TRANSFER_STREAM_CONTROL_FRAME_MAX_BYTES + 1,
        });

        assert_eq!(
            decode_transfer_stream_frame_header(encoded),
            Err(TransferStreamFrameDecodeError::PayloadTooLarge {
                payload_len: TRANSFER_STREAM_CONTROL_FRAME_MAX_BYTES + 1,
                limit: TRANSFER_STREAM_CONTROL_FRAME_MAX_BYTES,
            })
        );
    }
}
