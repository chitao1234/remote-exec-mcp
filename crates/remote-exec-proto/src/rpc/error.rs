use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RpcErrorBody {
    code: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpcErrorCode {
    BadRequest,
    Unauthorized,
    UnknownSession,
    ExecSessionLockTimeout,
    NotFound,
    UnknownEndpoint,
    InvalidPortTunnel,
    PortTunnelUnavailable,
    PortTunnelLimitExceeded,
    PortTunnelAlreadyAttached,
    PortTunnelResumeExpired,
    PortTunnelGenerationMismatch,
    UnknownPortTunnelSession,
    PortTunnelClosed,
    PortForwardBackpressureExceeded,
    InvalidPortTunnelMetadata,
    InvalidEndpoint,
    PortBindFailed,
    PortAcceptFailed,
    PortConnectFailed,
    PortReadFailed,
    PortWriteFailed,
    PortConnectionClosed,
    UnknownPortConnection,
    UnknownPortBind,
    SandboxDenied,
    StdinClosed,
    TtyDisabled,
    TtyUnsupported,
    InvalidPtySize,
    LoginShellUnsupported,
    LoginShellDisabled,
    InvalidDetail,
    ImageMissing,
    ImageNotFile,
    ImageDecodeFailed,
    TransferPathNotAbsolute,
    TransferDestinationExists,
    TransferParentMissing,
    TransferDestinationUnsupported,
    TransferCompressionUnsupported,
    TransferSourceUnsupported,
    TransferSourceMissing,
    TransferFailed,
    PatchFailed,
    Internal,
}

macro_rules! rpc_error_code_mappings {
    ($macro:ident) => {
        $macro! {
            BadRequest => "bad_request",
            Unauthorized => "unauthorized",
            UnknownSession => "unknown_session",
            ExecSessionLockTimeout => "exec_session_lock_timeout",
            NotFound => "not_found",
            UnknownEndpoint => "unknown_endpoint",
            InvalidPortTunnel => "invalid_port_tunnel",
            PortTunnelUnavailable => "port_tunnel_unavailable",
            PortTunnelLimitExceeded => "port_tunnel_limit_exceeded",
            PortTunnelAlreadyAttached => "port_tunnel_already_attached",
            PortTunnelResumeExpired => "port_tunnel_resume_expired",
            PortTunnelGenerationMismatch => "port_tunnel_generation_mismatch",
            UnknownPortTunnelSession => "unknown_port_tunnel_session",
            PortTunnelClosed => "port_tunnel_closed",
            PortForwardBackpressureExceeded => "port_forward_backpressure_exceeded",
            InvalidPortTunnelMetadata => "invalid_port_tunnel_metadata",
            InvalidEndpoint => "invalid_endpoint",
            PortBindFailed => "port_bind_failed",
            PortAcceptFailed => "port_accept_failed",
            PortConnectFailed => "port_connect_failed",
            PortReadFailed => "port_read_failed",
            PortWriteFailed => "port_write_failed",
            PortConnectionClosed => "port_connection_closed",
            UnknownPortConnection => "unknown_port_connection",
            UnknownPortBind => "unknown_port_bind",
            SandboxDenied => "sandbox_denied",
            StdinClosed => "stdin_closed",
            TtyDisabled => "tty_disabled",
            TtyUnsupported => "tty_unsupported",
            InvalidPtySize => "invalid_pty_size",
            LoginShellUnsupported => "login_shell_unsupported",
            LoginShellDisabled => "login_shell_disabled",
            InvalidDetail => "invalid_detail",
            ImageMissing => "image_missing",
            ImageNotFile => "image_not_file",
            ImageDecodeFailed => "image_decode_failed",
            TransferPathNotAbsolute => "transfer_path_not_absolute",
            TransferDestinationExists => "transfer_destination_exists",
            TransferParentMissing => "transfer_parent_missing",
            TransferDestinationUnsupported => "transfer_destination_unsupported",
            TransferCompressionUnsupported => "transfer_compression_unsupported",
            TransferSourceUnsupported => "transfer_source_unsupported",
            TransferSourceMissing => "transfer_source_missing",
            TransferFailed => "transfer_failed",
            PatchFailed => "patch_failed",
            Internal => "internal_error",
        }
    };
}

macro_rules! impl_rpc_error_code_wire_values {
    ($($variant:ident => $wire:literal,)+) => {
        impl RpcErrorCode {
            pub fn wire_value(self) -> &'static str {
                match self {
                    $(Self::$variant => $wire,)+
                }
            }

            pub fn from_wire_value(value: &str) -> Option<Self> {
                match value {
                    $($wire => Some(Self::$variant),)+
                    "internal" => Some(Self::Internal),
                    _ => None,
                }
            }
        }
    };
}

rpc_error_code_mappings!(impl_rpc_error_code_wire_values);

impl RpcErrorBody {
    pub fn new(code: RpcErrorCode, message: impl Into<String>) -> Self {
        Self {
            code: code.wire_value().to_string(),
            message: message.into(),
        }
    }

    pub fn from_raw_code(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn code(&self) -> Option<RpcErrorCode> {
        RpcErrorCode::from_wire_value(&self.code)
    }

    pub fn wire_code(&self) -> &str {
        &self.code
    }
}

#[cfg(test)]
mod tests {
    use super::RpcErrorCode;

    #[test]
    fn rpc_error_code_internal_wire_value_round_trips() {
        assert_eq!(RpcErrorCode::Internal.wire_value(), "internal_error");
        assert_eq!(
            RpcErrorCode::from_wire_value("internal_error"),
            Some(RpcErrorCode::Internal)
        );
    }

    #[test]
    fn rpc_error_code_exec_session_lock_timeout_round_trips() {
        assert_eq!(
            RpcErrorCode::ExecSessionLockTimeout.wire_value(),
            "exec_session_lock_timeout"
        );
        assert_eq!(
            RpcErrorCode::from_wire_value("exec_session_lock_timeout"),
            Some(RpcErrorCode::ExecSessionLockTimeout)
        );
    }

    #[test]
    fn rpc_error_code_accepts_legacy_internal_alias() {
        assert_eq!(
            RpcErrorCode::from_wire_value("internal"),
            Some(RpcErrorCode::Internal)
        );
    }

    #[test]
    fn rpc_error_code_unknown_wire_value_returns_none() {
        assert_eq!(RpcErrorCode::from_wire_value("future_error_code"), None);
    }
}
