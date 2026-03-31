use anyhow::Context;
use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Content, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
};

pub struct ToolCallOutput {
    pub content: Vec<Content>,
    pub structured: serde_json::Value,
}

impl ToolCallOutput {
    pub fn text_and_structured(text: String, structured: serde_json::Value) -> Self {
        Self {
            content: vec![Content::text(text)],
            structured,
        }
    }

    pub fn into_call_tool_result(self) -> CallToolResult {
        CallToolResult {
            content: self.content,
            structured_content: Some(self.structured),
            is_error: Some(false),
            meta: None,
        }
    }
}

#[derive(Clone)]
pub struct BrokerServer {
    pub state: crate::BrokerState,
    tool_router: ToolRouter<Self>,
}

impl BrokerServer {
    pub fn new(state: crate::BrokerState) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl BrokerServer {
    #[tool(
        name = "exec_command",
        description = "Run a command on a configured target machine."
    )]
    async fn exec_command(
        &self,
        Parameters(input): Parameters<remote_exec_proto::public::ExecCommandInput>,
    ) -> Result<CallToolResult, McpError> {
        Ok(match crate::tools::exec::exec_command(&self.state, input).await {
            Ok(output) => output.into_call_tool_result(),
            Err(err) => CallToolResult::error(vec![Content::text(err.to_string())]),
        })
    }

    #[tool(
        name = "write_stdin",
        description = "Write to or poll an existing exec_command session."
    )]
    async fn write_stdin(
        &self,
        Parameters(input): Parameters<remote_exec_proto::public::WriteStdinInput>,
    ) -> Result<CallToolResult, McpError> {
        Ok(match crate::tools::exec::write_stdin(&self.state, input).await {
            Ok(output) => output.into_call_tool_result(),
            Err(err) => CallToolResult::error(vec![Content::text(err.to_string())]),
        })
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for BrokerServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some("Remote exec MCP broker".into()),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

pub async fn serve_stdio(state: crate::BrokerState) -> anyhow::Result<()> {
    let server = BrokerServer::new(state);
    server
        .serve(rmcp::transport::stdio())
        .await
        .context("starting broker MCP service")?
        .waiting()
        .await
        .context("waiting for broker MCP service")?;
    Ok(())
}

pub fn format_command_text(
    cmd: &str,
    response: &remote_exec_proto::rpc::ExecResponse,
    session_id: Option<&str>,
) -> String {
    let status = match (response.exit_code, session_id) {
        (Some(code), _) => format!("Process exited with code {code}"),
        (None, Some(id)) => format!("Process running with session ID {id}"),
        (None, None) => "Process running".to_string(),
    };

    format!(
        "Command: {cmd}\nChunk ID: {}\nWall time: {:.3} seconds\n{status}\nOutput:\n{}",
        response
            .chunk_id
            .clone()
            .unwrap_or_else(|| "n/a".to_string()),
        response.wall_time_seconds,
        response.output
    )
}

pub fn format_poll_text(
    response: &remote_exec_proto::rpc::ExecResponse,
    session_id: Option<&str>,
) -> String {
    let status = match (response.exit_code, session_id) {
        (Some(code), _) => format!("Process exited with code {code}"),
        (None, Some(id)) => format!("Process running with session ID {id}"),
        (None, None) => "Process running".to_string(),
    };

    format!(
        "Chunk ID: {}\nWall time: {:.3} seconds\n{status}\nOutput:\n{}",
        response
            .chunk_id
            .clone()
            .unwrap_or_else(|| "n/a".to_string()),
        response.wall_time_seconds,
        response.output
    )
}
