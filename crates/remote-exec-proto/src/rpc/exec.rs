use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecStartRequest {
    pub cmd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    pub tty: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub yield_time_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub login: Option<bool>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ExecPtySize {
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecWriteRequest {
    pub daemon_session_id: String,
    pub chars: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub yield_time_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pty_size: Option<ExecPtySize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExecOutputResponse {
    pub daemon_instance_id: String,
    pub running: bool,
    pub chunk_id: Option<String>,
    pub wall_time_seconds: f64,
    pub exit_code: Option<i32>,
    pub original_token_count: Option<u32>,
    pub output: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<ExecWarning>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExecRunningResponse {
    pub daemon_session_id: String,
    pub output: ExecOutputResponse,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExecCompletedResponse {
    pub output: ExecOutputResponse,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExecResponse {
    Running(ExecRunningResponse),
    Completed(ExecCompletedResponse),
}

#[derive(Debug, Serialize, Deserialize)]
struct ExecResponseEnvelope {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    daemon_session_id: Option<String>,
    #[serde(flatten)]
    output: ExecOutputResponse,
}

impl From<ExecResponse> for ExecResponseEnvelope {
    fn from(response: ExecResponse) -> Self {
        match response {
            ExecResponse::Running(response) => Self {
                daemon_session_id: Some(response.daemon_session_id),
                output: response.output,
            },
            ExecResponse::Completed(response) => Self {
                daemon_session_id: None,
                output: response.output,
            },
        }
    }
}

impl Serialize for ExecResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ExecResponseEnvelope::from(self.clone()).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ExecResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let envelope = ExecResponseEnvelope::deserialize(deserializer)?;
        let running = envelope.output.running;
        match (running, envelope.daemon_session_id) {
            (true, Some(daemon_session_id)) if !daemon_session_id.is_empty() => {
                Ok(Self::Running(ExecRunningResponse {
                    daemon_session_id,
                    output: envelope.output,
                }))
            }
            (true, _) => Err(serde::de::Error::custom(
                "daemon returned malformed exec response: running response missing daemon_session_id",
            )),
            (false, None) => Ok(Self::Completed(ExecCompletedResponse {
                output: envelope.output,
            })),
            (false, Some(_)) => Err(serde::de::Error::custom(
                "daemon returned malformed exec response: completed response unexpectedly included daemon_session_id",
            )),
        }
    }
}

impl ExecResponse {
    pub fn running(&self) -> bool {
        self.output().running
    }

    pub fn output(&self) -> &ExecOutputResponse {
        match self {
            Self::Running(response) => &response.output,
            Self::Completed(response) => &response.output,
        }
    }

    pub fn daemon_session_id(&self) -> Option<&str> {
        match self {
            Self::Running(response) => Some(response.daemon_session_id.as_str()),
            Self::Completed(_) => None,
        }
    }

    pub fn into_output(self) -> ExecOutputResponse {
        match self {
            Self::Running(response) => response.output,
            Self::Completed(response) => response.output,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExecStartResponse {
    pub daemon_session_id: String,
    pub response: ExecResponse,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExecWriteResponse {
    pub response: ExecResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct ExecWarning {
    pub code: String,
    pub message: String,
}

impl ExecWarning {
    pub fn session_limit_approaching(target: &str) -> Self {
        Self {
            code: super::WarningCode::ExecSessionLimitApproaching
                .wire_value()
                .to_string(),
            message: format!("Target `{target}` now has 60 open exec sessions."),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ExecCompletedResponse, ExecOutputResponse, ExecResponse, ExecStartRequest, ExecWriteRequest,
    };

    #[test]
    fn exec_start_request_omits_none_fields() {
        let request = ExecStartRequest {
            cmd: "echo hi".to_string(),
            workdir: None,
            shell: None,
            tty: false,
            yield_time_ms: None,
            max_output_tokens: None,
            login: None,
        };

        let json = serde_json::to_value(&request).unwrap();

        assert_eq!(
            json,
            serde_json::json!({
                "cmd": "echo hi",
                "tty": false,
            })
        );
    }

    #[test]
    fn exec_write_request_omits_none_fields() {
        let request = ExecWriteRequest {
            daemon_session_id: "daemon-session-1".to_string(),
            chars: String::new(),
            yield_time_ms: None,
            max_output_tokens: None,
            pty_size: None,
        };

        let json = serde_json::to_value(&request).unwrap();

        assert_eq!(
            json,
            serde_json::json!({
                "daemon_session_id": "daemon-session-1",
                "chars": "",
            })
        );
    }

    #[test]
    fn exec_write_request_serializes_pty_size() {
        let request = ExecWriteRequest {
            daemon_session_id: "daemon-session-1".to_string(),
            chars: String::new(),
            yield_time_ms: None,
            max_output_tokens: None,
            pty_size: Some(super::ExecPtySize {
                rows: 33,
                cols: 101,
            }),
        };

        let json = serde_json::to_value(&request).unwrap();

        assert_eq!(
            json,
            serde_json::json!({
                "daemon_session_id": "daemon-session-1",
                "chars": "",
                "pty_size": {
                    "rows": 33,
                    "cols": 101,
                },
            })
        );
    }

    #[test]
    fn running_exec_response_requires_daemon_session_id() {
        let value = serde_json::json!({
            "daemon_instance_id": "inst",
            "running": true,
            "chunk_id": "chunk",
            "wall_time_seconds": 0.1,
            "exit_code": null,
            "original_token_count": 1,
            "output": "hi"
        });

        assert!(serde_json::from_value::<ExecResponse>(value).is_err());
    }

    #[test]
    fn completed_exec_response_rejects_daemon_session_id() {
        let value = serde_json::json!({
            "daemon_session_id": "daemon-session-1",
            "daemon_instance_id": "inst",
            "running": false,
            "chunk_id": null,
            "wall_time_seconds": 0.1,
            "exit_code": 0,
            "original_token_count": 1,
            "output": "done"
        });

        assert!(serde_json::from_value::<ExecResponse>(value).is_err());
    }

    #[test]
    fn completed_exec_response_omits_daemon_session_id() {
        let response = ExecResponse::Completed(ExecCompletedResponse {
            output: ExecOutputResponse {
                daemon_instance_id: "inst".to_string(),
                running: false,
                chunk_id: None,
                wall_time_seconds: 0.1,
                exit_code: Some(0),
                original_token_count: Some(1),
                output: "done".to_string(),
                warnings: Vec::new(),
            },
        });

        let value = serde_json::to_value(response).unwrap();

        assert!(value.get("daemon_session_id").is_none());
    }
}
