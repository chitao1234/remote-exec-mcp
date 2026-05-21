use remote_exec_proto::rpc::RpcErrorCode;

#[derive(Debug)]
pub enum DaemonClientError {
    Transport(anyhow::Error),
    Rpc {
        status: reqwest::StatusCode,
        code: Option<DaemonRpcCode>,
        message: String,
    },
    Decode(anyhow::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonRpcCode {
    Known(RpcErrorCode),
    Unknown(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpcToolErrorMode {
    Full,
    MessageOnly,
}

impl DaemonRpcCode {
    pub fn from_wire_value(value: impl Into<String>) -> Self {
        let value = value.into();
        match RpcErrorCode::from_wire_value(&value) {
            Some(code) => Self::Known(code),
            None => Self::Unknown(value),
        }
    }

    pub fn as_wire_value(&self) -> &str {
        match self {
            Self::Known(code) => code.wire_value(),
            Self::Unknown(value) => value.as_str(),
        }
    }

    pub fn known(&self) -> Option<RpcErrorCode> {
        match self {
            Self::Known(code) => Some(*code),
            Self::Unknown(_) => None,
        }
    }
}

impl DaemonClientError {
    pub fn rpc_code(&self) -> Option<&str> {
        match self {
            Self::Rpc { code, .. } => code.as_ref().map(DaemonRpcCode::as_wire_value),
            _ => None,
        }
    }

    pub fn rpc_error_code(&self) -> Option<RpcErrorCode> {
        match self {
            Self::Rpc { code, .. } => code.as_ref().and_then(DaemonRpcCode::known),
            _ => None,
        }
    }

    pub fn is_rpc_error_code(&self, expected: RpcErrorCode) -> bool {
        self.rpc_error_code() == Some(expected)
    }

    pub fn is_transport(&self) -> bool {
        matches!(self, Self::Transport(_))
    }

    pub fn into_tool_error(self, rpc_mode: RpcToolErrorMode) -> anyhow::Error {
        match (self, rpc_mode) {
            (Self::Rpc { message, .. }, RpcToolErrorMode::MessageOnly) => {
                anyhow::Error::msg(message)
            }
            (other, _) => other.into(),
        }
    }
}

impl std::fmt::Display for DaemonClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(err) => write!(f, "daemon transport error: {err:#}"),
            Self::Decode(err) => {
                let message = err.to_string();
                if message.starts_with("daemon returned malformed exec response: ") {
                    f.write_str(&message)
                } else {
                    write!(f, "daemon decode error: {message}")
                }
            }
            Self::Rpc {
                status,
                code,
                message,
            } => match code {
                Some(code) => write!(f, "{}: {message} ({status})", code.as_wire_value()),
                None => write!(f, "daemon returned {status}: {message}"),
            },
        }
    }
}

impl std::error::Error for DaemonClientError {}

pub(crate) fn normalize_tool_error(
    err: anyhow::Error,
    rpc_mode: RpcToolErrorMode,
) -> anyhow::Error {
    match err.downcast::<DaemonClientError>() {
        Ok(other) => other.into_tool_error(rpc_mode),
        Err(other) => other,
    }
}

pub(crate) fn normalize_tool_result<T>(
    result: Result<T, DaemonClientError>,
    rpc_mode: RpcToolErrorMode,
) -> anyhow::Result<T> {
    result.map_err(|err| err.into_tool_error(rpc_mode))
}
