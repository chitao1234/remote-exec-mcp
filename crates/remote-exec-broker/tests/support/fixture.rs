use rmcp::{
    ClientHandler, RoleClient,
    model::{CallToolRequestParams, CallToolResult, ClientInfo},
    service::RunningService,
};
use tempfile::TempDir;

use super::stub_daemon::{StubDaemonState, StubImageReadResponse};

pub struct BrokerFixture {
    pub _tempdir: TempDir,
    pub client: RunningService<RoleClient, DummyClientHandler>,
    #[allow(dead_code, reason = "Shared across broker integration test crates")]
    pub(super) stub_state: StubDaemonState,
}

impl BrokerFixture {
    pub async fn call_tool(&self, name: &str, arguments: serde_json::Value) -> ToolResult {
        let result = self.raw_call_tool(name, arguments).await;
        assert!(
            !result.is_error,
            "expected successful tool call, got {}",
            result.text_output
        );
        result
    }

    async fn raw_call_tool(&self, name: &str, arguments: serde_json::Value) -> ToolResult {
        let result = self
            .client
            .call_tool(CallToolRequestParams {
                meta: None,
                name: name.to_string().into(),
                arguments: Some(arguments.as_object().unwrap().clone()),
                task: None,
            })
            .await
            .unwrap();

        ToolResult::from_call_tool_result(result)
    }
}

#[allow(dead_code, reason = "Shared across broker integration test crates")]
impl BrokerFixture {
    pub async fn raw_tool_result(&self, name: &str, arguments: serde_json::Value) -> ToolResult {
        self.raw_call_tool(name, arguments).await
    }

    pub async fn call_tool_error(&self, name: &str, arguments: serde_json::Value) -> String {
        let result = self.raw_call_tool(name, arguments).await;
        assert!(
            result.is_error,
            "expected tool error, text={}, structured={}, raw={}",
            result.text_output,
            result.structured_content,
            serde_json::Value::Array(result.raw_content.clone())
        );
        result.text_output
    }

    pub async fn exec_start_calls(&self) -> usize {
        *self.stub_state.exec_start_calls.lock().await
    }

    pub async fn last_patch_request(&self) -> Option<remote_exec_proto::rpc::PatchApplyRequest> {
        self.stub_state.last_patch_request.lock().await.clone()
    }

    pub async fn set_image_read_response(&self, response: StubImageReadResponse) {
        *self.stub_state.image_read_response.lock().await = response;
    }

    pub async fn set_exec_start_warnings(
        &self,
        warnings: Vec<remote_exec_proto::rpc::ExecWarning>,
    ) {
        *self.stub_state.exec_start_warnings.lock().await = warnings;
    }

    pub async fn set_stub_daemon_instance_id(&self, daemon_instance_id: &str) {
        *self.stub_state.daemon_instance_id.lock().await = daemon_instance_id.to_string();
    }

    pub async fn start_running_session(&self) -> String {
        let result = self
            .call_tool(
                "exec_command",
                serde_json::json!({
                    "target": "builder-a",
                    "cmd": "printf ready; sleep 2",
                    "tty": true,
                    "yield_time_ms": 10
                }),
            )
            .await;

        result.structured_content["session_id"]
            .as_str()
            .expect("running session")
            .to_string()
    }
}

pub struct ToolResult {
    pub is_error: bool,
    pub text_output: String,
    pub structured_content: serde_json::Value,
    pub raw_content: Vec<serde_json::Value>,
    #[allow(dead_code, reason = "Shared across broker integration test crates")]
    pub meta: Option<serde_json::Value>,
}

impl ToolResult {
    fn from_call_tool_result(result: CallToolResult) -> Self {
        let text_output = result
            .content
            .iter()
            .filter_map(|content| content.raw.as_text().map(|text| text.text.as_str()))
            .collect::<Vec<_>>()
            .join("\n");
        let raw_content = result.content.iter().map(normalize_content).collect();

        Self {
            is_error: result.is_error.unwrap_or(false),
            text_output,
            structured_content: result.structured_content.unwrap_or(serde_json::Value::Null),
            raw_content,
            meta: result.meta.map(|meta| serde_json::to_value(meta).unwrap()),
        }
    }
}

fn normalize_content(content: &rmcp::model::Content) -> serde_json::Value {
    if let Some(text) = content.raw.as_text() {
        return serde_json::json!({
            "type": "text",
            "text": text.text,
        });
    }

    if let Some(image) = content.raw.as_image() {
        return serde_json::json!({
            "type": "input_image",
            "image_url": format!("data:{};base64,{}", image.mime_type, image.data),
        });
    }

    serde_json::to_value(content).unwrap()
}

#[derive(Debug, Clone, Default)]
pub struct DummyClientHandler;

impl ClientHandler for DummyClientHandler {
    fn get_info(&self) -> ClientInfo {
        ClientInfo::default()
    }
}
