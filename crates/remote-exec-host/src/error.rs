use std::fmt;

use remote_exec_proto::rpc::{RpcErrorBody, RpcErrorCode};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostRpcError {
    pub status: u16,
    pub code: RpcErrorCode,
    pub message: String,
}

impl HostRpcError {
    pub fn new(status: u16, code: RpcErrorCode, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    pub fn wire_code(&self) -> &'static str {
        self.code.wire_value()
    }

    pub fn into_http_rpc_parts(self, sink: &'static str) -> (u16, RpcErrorBody) {
        let normalized_status = if (100..=999).contains(&self.status) {
            self.status
        } else {
            tracing::error!(
                sink,
                status = self.status,
                code = self.code.wire_value(),
                message = %self.message,
                "invalid host rpc http status; normalizing to 500"
            );
            500
        };
        (
            normalized_status,
            RpcErrorBody::new(self.code, self.message),
        )
    }

    pub fn into_rpc_parts(self) -> (u16, RpcErrorBody) {
        (self.status, RpcErrorBody::new(self.code, self.message))
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

pub(crate) fn logged_bad_request(
    code: remote_exec_proto::rpc::RpcErrorCode,
    message: impl Into<String>,
) -> HostRpcError {
    let message = message.into();
    tracing::warn!(code = code.wire_value(), %message, "daemon request rejected");
    bad_request(code, message)
}

pub(crate) fn internal(
    code: remote_exec_proto::rpc::RpcErrorCode,
    message: impl Into<String>,
) -> HostRpcError {
    rpc_error(500, code, message)
}

macro_rules! define_domain_error {
    (
        $error:ident,
        $kind:ident,
        $internal_log:literal,
        {
            $($ctor:ident => $variant:ident => $code:expr),+ $(,)?
        }
    ) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum $kind {
            $($variant,)+
        }

        #[derive(Debug)]
        pub struct $error {
            kind: $kind,
            message: String,
        }

        impl $error {
            $(
                pub fn $ctor(message: impl Into<String>) -> Self {
                    Self::new($kind::$variant, message)
                }
            )+

            pub fn code(&self) -> RpcErrorCode {
                match self.kind {
                    $($kind::$variant => $code,)+
                }
            }

            fn into_host_rpc_error(self) -> HostRpcError {
                let code = self.code();
                let message = self.message;
                if self.kind == $kind::Internal {
                    tracing::error!(code = code.wire_value(), %message, $internal_log);
                    internal(code, message)
                } else {
                    tracing::warn!(code = code.wire_value(), %message, "daemon request rejected");
                    bad_request(code, message)
                }
            }

            fn new(kind: $kind, message: impl Into<String>) -> Self {
                Self {
                    kind,
                    message: message.into(),
                }
            }
        }

        impl fmt::Display for $error {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.message)
            }
        }

        impl std::error::Error for $error {}

        impl From<$error> for HostRpcError {
            fn from(value: $error) -> Self {
                value.into_host_rpc_error()
            }
        }
    };
}

define_domain_error!(
    TransferError,
    TransferErrorKind,
    "daemon internal transfer error",
    {
        sandbox_denied => SandboxDenied => RpcErrorCode::SandboxDenied,
        path_not_absolute => PathNotAbsolute => RpcErrorCode::TransferPathNotAbsolute,
        destination_exists => DestinationExists => RpcErrorCode::TransferDestinationExists,
        parent_missing => ParentMissing => RpcErrorCode::TransferParentMissing,
        destination_unsupported => DestinationUnsupported => RpcErrorCode::TransferDestinationUnsupported,
        compression_unsupported => CompressionUnsupported => RpcErrorCode::TransferCompressionUnsupported,
        source_unsupported => SourceUnsupported => RpcErrorCode::TransferSourceUnsupported,
        source_missing => SourceMissing => RpcErrorCode::TransferSourceMissing,
        failed => Failed => RpcErrorCode::TransferFailed,
        internal => Internal => RpcErrorCode::Internal,
    }
);

define_domain_error!(
    ImageError,
    ImageErrorKind,
    "daemon internal image error",
    {
        sandbox_denied => SandboxDenied => RpcErrorCode::SandboxDenied,
        invalid_detail => InvalidDetail => RpcErrorCode::InvalidDetail,
        missing => Missing => RpcErrorCode::ImageMissing,
        not_file => NotFile => RpcErrorCode::ImageNotFile,
        decode_failed => DecodeFailed => RpcErrorCode::ImageDecodeFailed,
        internal => Internal => RpcErrorCode::Internal,
    }
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PatchErrorKind {
    SandboxDenied,
    Failed,
}

#[derive(Debug)]
pub(crate) struct PatchError {
    kind: PatchErrorKind,
    message: String,
}

impl PatchError {
    pub(crate) fn sandbox_denied(message: impl Into<String>) -> Self {
        Self::new(PatchErrorKind::SandboxDenied, message)
    }

    pub(crate) fn failed(message: impl Into<String>) -> Self {
        Self::new(PatchErrorKind::Failed, message)
    }

    pub(crate) fn failed_error(err: impl fmt::Display) -> Self {
        Self::failed(err.to_string())
    }

    pub(crate) fn code(&self) -> RpcErrorCode {
        match self.kind {
            PatchErrorKind::SandboxDenied => RpcErrorCode::SandboxDenied,
            PatchErrorKind::Failed => RpcErrorCode::PatchFailed,
        }
    }

    fn into_host_rpc_error(self) -> HostRpcError {
        let code = self.code();
        let message = self.message;
        tracing::warn!(code = code.wire_value(), %message, "daemon request rejected");
        bad_request(code, message)
    }

    fn new(kind: PatchErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl fmt::Display for PatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PatchError {}

impl From<PatchError> for HostRpcError {
    fn from(value: PatchError) -> Self {
        value.into_host_rpc_error()
    }
}

impl From<crate::sandbox::SandboxError> for PatchError {
    fn from(value: crate::sandbox::SandboxError) -> Self {
        Self::sandbox_denied(value.to_string())
    }
}

impl From<std::io::Error> for PatchError {
    fn from(value: std::io::Error) -> Self {
        Self::failed_error(value)
    }
}

impl From<std::string::FromUtf8Error> for PatchError {
    fn from(value: std::string::FromUtf8Error) -> Self {
        Self::failed_error(value)
    }
}

#[cfg(test)]
mod tests {
    use remote_exec_proto::rpc::RpcErrorCode;

    use super::{HostRpcError, ImageError, PatchError, TransferError};

    #[test]
    fn transfer_internal_maps_to_internal_error_server_response() {
        let err: HostRpcError = TransferError::internal("transfer boom").into();
        assert_eq!(err.status, 500);
        assert_eq!(err.code, RpcErrorCode::Internal);
        assert_eq!(err.wire_code(), "internal_error");
        assert_eq!(err.message, "transfer boom");
    }

    #[test]
    fn image_internal_maps_to_internal_error_server_response() {
        let err: HostRpcError = ImageError::internal("image boom").into();
        assert_eq!(err.status, 500);
        assert_eq!(err.code, RpcErrorCode::Internal);
        assert_eq!(err.wire_code(), "internal_error");
        assert_eq!(err.message, "image boom");
    }

    #[test]
    fn image_decode_failed_stays_a_client_error() {
        let err: HostRpcError = ImageError::decode_failed("bad image bytes").into();
        assert_eq!(err.status, 400);
        assert_eq!(err.code, RpcErrorCode::ImageDecodeFailed);
        assert_eq!(err.wire_code(), "image_decode_failed");
        assert_eq!(err.message, "bad image bytes");
    }

    #[test]
    fn patch_failed_stays_a_client_error() {
        let err: HostRpcError = PatchError::failed("patch boom").into();
        assert_eq!(err.status, 400);
        assert_eq!(err.code, RpcErrorCode::PatchFailed);
        assert_eq!(err.wire_code(), "patch_failed");
        assert_eq!(err.message, "patch boom");
    }

    #[test]
    fn invalid_http_status_normalizes_to_internal_server_error() {
        let err = HostRpcError::new(42, RpcErrorCode::Internal, "invalid status");
        let (status, body) = err.into_http_rpc_parts("test");
        assert_eq!(status, 500);
        assert_eq!(body.wire_code(), "internal_error");
        assert_eq!(body.message, "invalid status");
    }

    #[test]
    fn valid_http_status_is_preserved_during_http_normalization() {
        let err = HostRpcError::new(599, RpcErrorCode::Internal, "status preserved");
        let (status, body) = err.into_http_rpc_parts("test");
        assert_eq!(status, 599);
        assert_eq!(body.wire_code(), "internal_error");
        assert_eq!(body.message, "status preserved");
    }
}
