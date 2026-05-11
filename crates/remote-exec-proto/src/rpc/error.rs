use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RpcErrorBody {
    pub code: String,
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

impl RpcErrorCode {
    pub const fn wire_value(self) -> &'static str {
        match self {
            Self::BadRequest => "bad_request",
            Self::Unauthorized => "unauthorized",
            Self::UnknownSession => "unknown_session",
            Self::ExecSessionLockTimeout => "exec_session_lock_timeout",
            Self::NotFound => "not_found",
            Self::UnknownEndpoint => "unknown_endpoint",
            Self::InvalidPortTunnel => "invalid_port_tunnel",
            Self::PortTunnelUnavailable => "port_tunnel_unavailable",
            Self::PortTunnelLimitExceeded => "port_tunnel_limit_exceeded",
            Self::PortTunnelAlreadyAttached => "port_tunnel_already_attached",
            Self::PortTunnelResumeExpired => "port_tunnel_resume_expired",
            Self::PortTunnelGenerationMismatch => "port_tunnel_generation_mismatch",
            Self::UnknownPortTunnelSession => "unknown_port_tunnel_session",
            Self::PortTunnelClosed => "port_tunnel_closed",
            Self::PortForwardBackpressureExceeded => "port_forward_backpressure_exceeded",
            Self::InvalidPortTunnelMetadata => "invalid_port_tunnel_metadata",
            Self::InvalidEndpoint => "invalid_endpoint",
            Self::PortBindFailed => "port_bind_failed",
            Self::PortAcceptFailed => "port_accept_failed",
            Self::PortConnectFailed => "port_connect_failed",
            Self::PortReadFailed => "port_read_failed",
            Self::PortWriteFailed => "port_write_failed",
            Self::PortConnectionClosed => "port_connection_closed",
            Self::UnknownPortConnection => "unknown_port_connection",
            Self::UnknownPortBind => "unknown_port_bind",
            Self::SandboxDenied => "sandbox_denied",
            Self::StdinClosed => "stdin_closed",
            Self::TtyDisabled => "tty_disabled",
            Self::TtyUnsupported => "tty_unsupported",
            Self::InvalidPtySize => "invalid_pty_size",
            Self::LoginShellUnsupported => "login_shell_unsupported",
            Self::LoginShellDisabled => "login_shell_disabled",
            Self::InvalidDetail => "invalid_detail",
            Self::ImageMissing => "image_missing",
            Self::ImageNotFile => "image_not_file",
            Self::ImageDecodeFailed => "image_decode_failed",
            Self::TransferPathNotAbsolute => "transfer_path_not_absolute",
            Self::TransferDestinationExists => "transfer_destination_exists",
            Self::TransferParentMissing => "transfer_parent_missing",
            Self::TransferDestinationUnsupported => "transfer_destination_unsupported",
            Self::TransferCompressionUnsupported => "transfer_compression_unsupported",
            Self::TransferSourceUnsupported => "transfer_source_unsupported",
            Self::TransferSourceMissing => "transfer_source_missing",
            Self::TransferFailed => "transfer_failed",
            Self::PatchFailed => "patch_failed",
            Self::Internal => "internal_error",
        }
    }

    pub fn from_wire_value(value: &str) -> Option<Self> {
        match value {
            "bad_request" => Some(Self::BadRequest),
            "unauthorized" => Some(Self::Unauthorized),
            "unknown_session" => Some(Self::UnknownSession),
            "exec_session_lock_timeout" => Some(Self::ExecSessionLockTimeout),
            "not_found" => Some(Self::NotFound),
            "unknown_endpoint" => Some(Self::UnknownEndpoint),
            "invalid_port_tunnel" => Some(Self::InvalidPortTunnel),
            "port_tunnel_unavailable" => Some(Self::PortTunnelUnavailable),
            "port_tunnel_limit_exceeded" => Some(Self::PortTunnelLimitExceeded),
            "port_tunnel_already_attached" => Some(Self::PortTunnelAlreadyAttached),
            "port_tunnel_resume_expired" => Some(Self::PortTunnelResumeExpired),
            "port_tunnel_generation_mismatch" => Some(Self::PortTunnelGenerationMismatch),
            "unknown_port_tunnel_session" => Some(Self::UnknownPortTunnelSession),
            "port_tunnel_closed" => Some(Self::PortTunnelClosed),
            "port_forward_backpressure_exceeded" => Some(Self::PortForwardBackpressureExceeded),
            "invalid_port_tunnel_metadata" => Some(Self::InvalidPortTunnelMetadata),
            "invalid_endpoint" => Some(Self::InvalidEndpoint),
            "port_bind_failed" => Some(Self::PortBindFailed),
            "port_accept_failed" => Some(Self::PortAcceptFailed),
            "port_connect_failed" => Some(Self::PortConnectFailed),
            "port_read_failed" => Some(Self::PortReadFailed),
            "port_write_failed" => Some(Self::PortWriteFailed),
            "port_connection_closed" => Some(Self::PortConnectionClosed),
            "unknown_port_connection" => Some(Self::UnknownPortConnection),
            "unknown_port_bind" => Some(Self::UnknownPortBind),
            "sandbox_denied" => Some(Self::SandboxDenied),
            "stdin_closed" => Some(Self::StdinClosed),
            "tty_disabled" => Some(Self::TtyDisabled),
            "tty_unsupported" => Some(Self::TtyUnsupported),
            "invalid_pty_size" => Some(Self::InvalidPtySize),
            "login_shell_unsupported" => Some(Self::LoginShellUnsupported),
            "login_shell_disabled" => Some(Self::LoginShellDisabled),
            "invalid_detail" => Some(Self::InvalidDetail),
            "image_missing" => Some(Self::ImageMissing),
            "image_not_file" => Some(Self::ImageNotFile),
            "image_decode_failed" => Some(Self::ImageDecodeFailed),
            "transfer_path_not_absolute" => Some(Self::TransferPathNotAbsolute),
            "transfer_destination_exists" => Some(Self::TransferDestinationExists),
            "transfer_parent_missing" => Some(Self::TransferParentMissing),
            "transfer_destination_unsupported" => Some(Self::TransferDestinationUnsupported),
            "transfer_compression_unsupported" => Some(Self::TransferCompressionUnsupported),
            "transfer_source_unsupported" => Some(Self::TransferSourceUnsupported),
            "transfer_source_missing" => Some(Self::TransferSourceMissing),
            "transfer_failed" => Some(Self::TransferFailed),
            "patch_failed" => Some(Self::PatchFailed),
            "internal" | "internal_error" => Some(Self::Internal),
            _ => None,
        }
    }
}

impl RpcErrorBody {
    pub fn new(code: RpcErrorCode, message: impl Into<String>) -> Self {
        Self {
            code: code.wire_value().to_string(),
            message: message.into(),
        }
    }

    pub fn code(&self) -> Option<RpcErrorCode> {
        RpcErrorCode::from_wire_value(&self.code)
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
