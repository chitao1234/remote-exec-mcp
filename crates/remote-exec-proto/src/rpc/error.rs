use serde::{Deserialize, Serialize};

use crate::wire;

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

const RPC_ERROR_CODE_WIRE_VALUES: &[(RpcErrorCode, &str)] = &[
    (RpcErrorCode::BadRequest, "bad_request"),
    (RpcErrorCode::Unauthorized, "unauthorized"),
    (RpcErrorCode::UnknownSession, "unknown_session"),
    (
        RpcErrorCode::ExecSessionLockTimeout,
        "exec_session_lock_timeout",
    ),
    (RpcErrorCode::NotFound, "not_found"),
    (RpcErrorCode::UnknownEndpoint, "unknown_endpoint"),
    (RpcErrorCode::InvalidPortTunnel, "invalid_port_tunnel"),
    (
        RpcErrorCode::PortTunnelUnavailable,
        "port_tunnel_unavailable",
    ),
    (
        RpcErrorCode::PortTunnelLimitExceeded,
        "port_tunnel_limit_exceeded",
    ),
    (
        RpcErrorCode::PortTunnelAlreadyAttached,
        "port_tunnel_already_attached",
    ),
    (
        RpcErrorCode::PortTunnelResumeExpired,
        "port_tunnel_resume_expired",
    ),
    (
        RpcErrorCode::PortTunnelGenerationMismatch,
        "port_tunnel_generation_mismatch",
    ),
    (
        RpcErrorCode::UnknownPortTunnelSession,
        "unknown_port_tunnel_session",
    ),
    (RpcErrorCode::PortTunnelClosed, "port_tunnel_closed"),
    (
        RpcErrorCode::PortForwardBackpressureExceeded,
        "port_forward_backpressure_exceeded",
    ),
    (
        RpcErrorCode::InvalidPortTunnelMetadata,
        "invalid_port_tunnel_metadata",
    ),
    (RpcErrorCode::InvalidEndpoint, "invalid_endpoint"),
    (RpcErrorCode::PortBindFailed, "port_bind_failed"),
    (RpcErrorCode::PortAcceptFailed, "port_accept_failed"),
    (RpcErrorCode::PortConnectFailed, "port_connect_failed"),
    (RpcErrorCode::PortReadFailed, "port_read_failed"),
    (RpcErrorCode::PortWriteFailed, "port_write_failed"),
    (RpcErrorCode::PortConnectionClosed, "port_connection_closed"),
    (
        RpcErrorCode::UnknownPortConnection,
        "unknown_port_connection",
    ),
    (RpcErrorCode::UnknownPortBind, "unknown_port_bind"),
    (RpcErrorCode::SandboxDenied, "sandbox_denied"),
    (RpcErrorCode::StdinClosed, "stdin_closed"),
    (RpcErrorCode::TtyDisabled, "tty_disabled"),
    (RpcErrorCode::TtyUnsupported, "tty_unsupported"),
    (RpcErrorCode::InvalidPtySize, "invalid_pty_size"),
    (
        RpcErrorCode::LoginShellUnsupported,
        "login_shell_unsupported",
    ),
    (RpcErrorCode::LoginShellDisabled, "login_shell_disabled"),
    (RpcErrorCode::InvalidDetail, "invalid_detail"),
    (RpcErrorCode::ImageMissing, "image_missing"),
    (RpcErrorCode::ImageNotFile, "image_not_file"),
    (RpcErrorCode::ImageDecodeFailed, "image_decode_failed"),
    (
        RpcErrorCode::TransferPathNotAbsolute,
        "transfer_path_not_absolute",
    ),
    (
        RpcErrorCode::TransferDestinationExists,
        "transfer_destination_exists",
    ),
    (
        RpcErrorCode::TransferParentMissing,
        "transfer_parent_missing",
    ),
    (
        RpcErrorCode::TransferDestinationUnsupported,
        "transfer_destination_unsupported",
    ),
    (
        RpcErrorCode::TransferCompressionUnsupported,
        "transfer_compression_unsupported",
    ),
    (
        RpcErrorCode::TransferSourceUnsupported,
        "transfer_source_unsupported",
    ),
    (
        RpcErrorCode::TransferSourceMissing,
        "transfer_source_missing",
    ),
    (RpcErrorCode::TransferFailed, "transfer_failed"),
    (RpcErrorCode::PatchFailed, "patch_failed"),
    (RpcErrorCode::Internal, "internal_error"),
];

const RPC_ERROR_CODE_WIRE_ALIASES: &[(&str, RpcErrorCode)] =
    &[("internal", RpcErrorCode::Internal)];

impl RpcErrorCode {
    pub fn wire_value(self) -> &'static str {
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
        wire::from_wire_value_with_aliases(
            value,
            RPC_ERROR_CODE_WIRE_VALUES,
            RPC_ERROR_CODE_WIRE_ALIASES,
        )
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
