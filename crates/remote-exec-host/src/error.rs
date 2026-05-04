use std::fmt;

use axum::Json;
use axum::http::StatusCode;
use remote_exec_proto::rpc::RpcErrorBody;

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

    pub fn code(&self) -> &'static str {
        match self.kind {
            TransferErrorKind::SandboxDenied => "sandbox_denied",
            TransferErrorKind::PathNotAbsolute => "transfer_path_not_absolute",
            TransferErrorKind::DestinationExists => "transfer_destination_exists",
            TransferErrorKind::ParentMissing => "transfer_parent_missing",
            TransferErrorKind::DestinationUnsupported => "transfer_destination_unsupported",
            TransferErrorKind::CompressionUnsupported => "transfer_compression_unsupported",
            TransferErrorKind::SourceUnsupported => "transfer_source_unsupported",
            TransferErrorKind::SourceMissing => "transfer_source_missing",
            TransferErrorKind::Internal => "transfer_failed",
        }
    }

    pub fn into_rpc(self) -> (StatusCode, Json<RpcErrorBody>) {
        let code = self.code();
        let message = self.message;
        if self.kind == TransferErrorKind::Internal {
            tracing::error!(code, %message, "daemon internal transfer error");
        } else {
            tracing::warn!(code, %message, "daemon request rejected");
        }
        (
            StatusCode::BAD_REQUEST,
            Json(RpcErrorBody {
                code: code.to_string(),
                message,
            }),
        )
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

    pub fn code(&self) -> &'static str {
        match self.kind {
            ImageErrorKind::SandboxDenied => "sandbox_denied",
            ImageErrorKind::InvalidDetail => "invalid_detail",
            ImageErrorKind::Missing => "image_missing",
            ImageErrorKind::NotFile => "image_not_file",
            ImageErrorKind::DecodeFailed | ImageErrorKind::Internal => "image_decode_failed",
        }
    }

    pub fn into_rpc(self) -> (StatusCode, Json<RpcErrorBody>) {
        let code = self.code();
        let message = self.message;
        if self.kind == ImageErrorKind::Internal {
            tracing::error!(code, %message, "daemon internal image error");
        } else {
            tracing::warn!(code, %message, "daemon request rejected");
        }
        (
            StatusCode::BAD_REQUEST,
            Json(RpcErrorBody {
                code: code.to_string(),
                message,
            }),
        )
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
