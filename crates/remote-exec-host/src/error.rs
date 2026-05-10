use std::fmt;

use remote_exec_proto::rpc::{RpcErrorBody, RpcErrorCode};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostRpcError {
    pub status: u16,
    pub code: String,
    pub message: String,
}

impl HostRpcError {
    pub fn new(status: u16, code: RpcErrorCode, message: impl Into<String>) -> Self {
        Self {
            status,
            code: code.wire_value().to_string(),
            message: message.into(),
        }
    }

    pub fn code(&self) -> Option<RpcErrorCode> {
        RpcErrorCode::from_wire_value(&self.code)
    }

    pub fn into_rpc_parts(self) -> (u16, RpcErrorBody) {
        (
            self.status,
            RpcErrorBody {
                code: self.code,
                message: self.message,
            },
        )
    }
}

pub(crate) fn rpc_error(
    status: u16,
    code: remote_exec_proto::rpc::RpcErrorCode,
    message: impl Into<String>,
) -> HostRpcError {
    HostRpcError::new(status, code, message)
}

pub(crate) fn bad_request(
    code: remote_exec_proto::rpc::RpcErrorCode,
    message: impl Into<String>,
) -> HostRpcError {
    rpc_error(400, code, message)
}

pub(crate) fn internal(
    code: remote_exec_proto::rpc::RpcErrorCode,
    message: impl Into<String>,
) -> HostRpcError {
    rpc_error(500, code, message)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferErrorKind {
    SandboxDenied,
    PathNotAbsolute,
    DestinationExists,
    ParentMissing,
    DestinationUnsupported,
    CompressionUnsupported,
    SourceUnsupported,
    SourceMissing,
    Internal,
}

#[derive(Debug)]
pub struct TransferError {
    kind: TransferErrorKind,
    message: String,
}

impl TransferError {
    pub fn sandbox_denied(message: impl Into<String>) -> Self {
        Self::new(TransferErrorKind::SandboxDenied, message)
    }

    pub fn path_not_absolute(message: impl Into<String>) -> Self {
        Self::new(TransferErrorKind::PathNotAbsolute, message)
    }

    pub fn destination_exists(message: impl Into<String>) -> Self {
        Self::new(TransferErrorKind::DestinationExists, message)
    }

    pub fn parent_missing(message: impl Into<String>) -> Self {
        Self::new(TransferErrorKind::ParentMissing, message)
    }

    pub fn destination_unsupported(message: impl Into<String>) -> Self {
        Self::new(TransferErrorKind::DestinationUnsupported, message)
    }

    pub fn compression_unsupported(message: impl Into<String>) -> Self {
        Self::new(TransferErrorKind::CompressionUnsupported, message)
    }

    pub fn source_unsupported(message: impl Into<String>) -> Self {
        Self::new(TransferErrorKind::SourceUnsupported, message)
    }

    pub fn source_missing(message: impl Into<String>) -> Self {
        Self::new(TransferErrorKind::SourceMissing, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(TransferErrorKind::Internal, message)
    }

    pub fn code(&self) -> RpcErrorCode {
        match self.kind {
            TransferErrorKind::SandboxDenied => RpcErrorCode::SandboxDenied,
            TransferErrorKind::PathNotAbsolute => RpcErrorCode::TransferPathNotAbsolute,
            TransferErrorKind::DestinationExists => RpcErrorCode::TransferDestinationExists,
            TransferErrorKind::ParentMissing => RpcErrorCode::TransferParentMissing,
            TransferErrorKind::DestinationUnsupported => {
                RpcErrorCode::TransferDestinationUnsupported
            }
            TransferErrorKind::CompressionUnsupported => {
                RpcErrorCode::TransferCompressionUnsupported
            }
            TransferErrorKind::SourceUnsupported => RpcErrorCode::TransferSourceUnsupported,
            TransferErrorKind::SourceMissing => RpcErrorCode::TransferSourceMissing,
            TransferErrorKind::Internal => RpcErrorCode::Internal,
        }
    }

    fn into_host_rpc_error(self) -> HostRpcError {
        let code = self.code();
        let message = self.message;
        if self.kind == TransferErrorKind::Internal {
            tracing::error!(code = code.wire_value(), %message, "daemon internal transfer error");
        } else {
            tracing::warn!(code = code.wire_value(), %message, "daemon request rejected");
        }
        if self.kind == TransferErrorKind::Internal {
            internal(code, message)
        } else {
            bad_request(code, message)
        }
    }

    fn new(kind: TransferErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl fmt::Display for TransferError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for TransferError {}

impl From<TransferError> for HostRpcError {
    fn from(value: TransferError) -> Self {
        value.into_host_rpc_error()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageErrorKind {
    SandboxDenied,
    InvalidDetail,
    Missing,
    NotFile,
    DecodeFailed,
    Internal,
}

#[derive(Debug)]
pub struct ImageError {
    kind: ImageErrorKind,
    message: String,
}

impl ImageError {
    pub fn sandbox_denied(message: impl Into<String>) -> Self {
        Self::new(ImageErrorKind::SandboxDenied, message)
    }

    pub fn invalid_detail(message: impl Into<String>) -> Self {
        Self::new(ImageErrorKind::InvalidDetail, message)
    }

    pub fn missing(message: impl Into<String>) -> Self {
        Self::new(ImageErrorKind::Missing, message)
    }

    pub fn not_file(message: impl Into<String>) -> Self {
        Self::new(ImageErrorKind::NotFile, message)
    }

    pub fn decode_failed(message: impl Into<String>) -> Self {
        Self::new(ImageErrorKind::DecodeFailed, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ImageErrorKind::Internal, message)
    }

    pub fn code(&self) -> RpcErrorCode {
        match self.kind {
            ImageErrorKind::SandboxDenied => RpcErrorCode::SandboxDenied,
            ImageErrorKind::InvalidDetail => RpcErrorCode::InvalidDetail,
            ImageErrorKind::Missing => RpcErrorCode::ImageMissing,
            ImageErrorKind::NotFile => RpcErrorCode::ImageNotFile,
            ImageErrorKind::DecodeFailed => RpcErrorCode::ImageDecodeFailed,
            ImageErrorKind::Internal => RpcErrorCode::Internal,
        }
    }

    fn into_host_rpc_error(self) -> HostRpcError {
        let code = self.code();
        let message = self.message;
        if self.kind == ImageErrorKind::Internal {
            tracing::error!(code = code.wire_value(), %message, "daemon internal image error");
        } else {
            tracing::warn!(code = code.wire_value(), %message, "daemon request rejected");
        }
        if self.kind == ImageErrorKind::Internal {
            internal(code, message)
        } else {
            bad_request(code, message)
        }
    }

    fn new(kind: ImageErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ImageError {}

impl From<ImageError> for HostRpcError {
    fn from(value: ImageError) -> Self {
        value.into_host_rpc_error()
    }
}

#[cfg(test)]
mod tests {
    use super::{HostRpcError, ImageError, TransferError};

    #[test]
    fn transfer_internal_maps_to_internal_error_server_response() {
        let err: HostRpcError = TransferError::internal("transfer boom").into();
        assert_eq!(err.status, 500);
        assert_eq!(err.code, "internal_error");
        assert_eq!(
            err.code(),
            Some(remote_exec_proto::rpc::RpcErrorCode::Internal)
        );
        assert_eq!(err.message, "transfer boom");
    }

    #[test]
    fn image_internal_maps_to_internal_error_server_response() {
        let err: HostRpcError = ImageError::internal("image boom").into();
        assert_eq!(err.status, 500);
        assert_eq!(err.code, "internal_error");
        assert_eq!(
            err.code(),
            Some(remote_exec_proto::rpc::RpcErrorCode::Internal)
        );
        assert_eq!(err.message, "image boom");
    }

    #[test]
    fn image_decode_failed_stays_a_client_error() {
        let err: HostRpcError = ImageError::decode_failed("bad image bytes").into();
        assert_eq!(err.status, 400);
        assert_eq!(err.code, "image_decode_failed");
        assert_eq!(
            err.code(),
            Some(remote_exec_proto::rpc::RpcErrorCode::ImageDecodeFailed)
        );
        assert_eq!(err.message, "bad image bytes");
    }
}
