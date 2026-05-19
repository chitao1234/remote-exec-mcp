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

pub fn encode_transfer_stream_frame_header(header: TransferStreamFrameHeader) -> [u8; 12] {
    let mut output = [0_u8; TRANSFER_STREAM_FRAME_HEADER_LEN];
    output[0] = header.frame_type.as_byte();
    output[1] = 0;
    output[2..4].copy_from_slice(&0_u16.to_be_bytes());
    output[4..12].copy_from_slice(&header.payload_len.to_be_bytes());
    output
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
