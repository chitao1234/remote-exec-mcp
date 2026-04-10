use anyhow::Context;
use axum::Router;
use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Content, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
    transport::{StreamableHttpServerConfig, StreamableHttpService},
};
use tokio_util::sync::CancellationToken;

pub struct ToolCallOutput {
    pub content: Vec<Content>,
    pub structured: Option<serde_json::Value>,
}

impl ToolCallOutput {
    pub fn text_and_structured(text: String, structured: serde_json::Value) -> Self {
        Self {
            content: vec![Content::text(text)],
            structured: Some(structured),
        }
    }

    pub fn text(text: String) -> Self {
        Self {
            content: vec![Content::text(text)],
            structured: None,
        }
    }

    pub fn content_and_structured(content: Vec<Content>, structured: serde_json::Value) -> Self {
        Self {
            content,
            structured: Some(structured),
        }
    }

    pub fn into_call_tool_result(self, include_structured_content: bool) -> CallToolResult {
        let mut result = CallToolResult::success(self.content);
        if include_structured_content {
            result.structured_content = self.structured;
        }
        result
    }
}

pub fn tool_error_result(text: String) -> CallToolResult {
    CallToolResult::error(vec![Content::text(text)])
}

pub fn format_tool_error(err: anyhow::Error) -> CallToolResult {
    tracing::warn!(error = %err, "broker tool returned error");
    tool_error_result(err.to_string())
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

    fn include_structured_content(&self) -> bool {
        !self.state.disable_structured_content
    }
}

#[tool_router]
impl BrokerServer {
    #[tool(
        name = "list_targets",
        description = "List configured target names.",
        annotations(read_only_hint = true)
    )]
    async fn list_targets(
        &self,
        Parameters(input): Parameters<remote_exec_proto::public::ListTargetsInput>,
    ) -> Result<CallToolResult, McpError> {
        Ok(
            match crate::tools::targets::list_targets(&self.state, input).await {
                Ok(output) => output.into_call_tool_result(self.include_structured_content()),
                Err(err) => format_tool_error(err),
            },
        )
    }

    #[tool(
        name = "exec_command",
        description = "Run a command on a configured target machine."
    )]
    async fn exec_command(
        &self,
        Parameters(input): Parameters<remote_exec_proto::public::ExecCommandInput>,
    ) -> Result<CallToolResult, McpError> {
        Ok(
            match crate::tools::exec::exec_command(&self.state, input).await {
                Ok(output) => output.into_call_tool_result(self.include_structured_content()),
                Err(err) => format_tool_error(err),
            },
        )
    }

    #[tool(
        name = "write_stdin",
        description = "Write to or poll an existing exec_command session."
    )]
    async fn write_stdin(
        &self,
        Parameters(input): Parameters<remote_exec_proto::public::WriteStdinInput>,
    ) -> Result<CallToolResult, McpError> {
        Ok(
            match crate::tools::exec::write_stdin(&self.state, input).await {
                Ok(output) => output.into_call_tool_result(self.include_structured_content()),
                Err(err) => format_tool_error(err),
            },
        )
    }

    #[tool(
        name = "apply_patch",
        description = "Apply a patch on a configured target machine."
    )]
    async fn apply_patch(
        &self,
        Parameters(input): Parameters<remote_exec_proto::public::ApplyPatchInput>,
    ) -> Result<CallToolResult, McpError> {
        Ok(
            match crate::tools::patch::apply_patch(&self.state, input).await {
                Ok(output) => output.into_call_tool_result(self.include_structured_content()),
                Err(err) => format_tool_error(err),
            },
        )
    }

    #[tool(
        name = "view_image",
        description = "Read an image from a configured target machine.",
        annotations(read_only_hint = true)
    )]
    async fn view_image(
        &self,
        Parameters(input): Parameters<remote_exec_proto::public::ViewImageInput>,
    ) -> Result<CallToolResult, McpError> {
        Ok(
            match crate::tools::image::view_image(&self.state, input).await {
                Ok(output) => output.into_call_tool_result(self.include_structured_content()),
                Err(err) => format_tool_error(err),
            },
        )
    }

    #[tool(
        name = "transfer_files",
        description = "Transfer one file or one directory tree between broker-local and configured target filesystems."
    )]
    async fn transfer_files(
        &self,
        Parameters(input): Parameters<remote_exec_proto::public::TransferFilesInput>,
    ) -> Result<CallToolResult, McpError> {
        Ok(
            match crate::tools::transfer::transfer_files(&self.state, input).await {
                Ok(output) => output.into_call_tool_result(self.include_structured_content()),
                Err(err) => format_tool_error(err),
            },
        )
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for BrokerServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions("Remote exec MCP broker")
    }
}

pub async fn serve_stdio(state: crate::BrokerState) -> anyhow::Result<()> {
    tracing::info!("starting broker MCP stdio service");
    let server = BrokerServer::new(state);
    server
        .serve(rmcp::transport::stdio())
        .await
        .context("starting broker MCP service")?
        .waiting()
        .await
        .context("waiting for broker MCP service")?;
    tracing::info!("broker MCP stdio service stopped");
    Ok(())
}

pub async fn serve(
    state: crate::BrokerState,
    config: &crate::config::McpServerConfig,
) -> anyhow::Result<()> {
    match config {
        crate::config::McpServerConfig::Stdio => serve_stdio(state).await,
        crate::config::McpServerConfig::StreamableHttp {
            listen,
            path,
            stateful,
            sse_keep_alive_ms,
            sse_retry_ms,
        } => {
            serve_streamable_http(
                state,
                *listen,
                path,
                *stateful,
                duration_from_millis(*sse_keep_alive_ms),
                duration_from_millis(*sse_retry_ms),
            )
            .await
        }
    }
}

async fn serve_streamable_http(
    state: crate::BrokerState,
    listen: std::net::SocketAddr,
    path: &str,
    stateful: bool,
    sse_keep_alive: Option<std::time::Duration>,
    sse_retry: Option<std::time::Duration>,
) -> anyhow::Result<()> {
    let cancellation_token = CancellationToken::new();
    let server_state = state.clone();
    let service: StreamableHttpService<
        _,
        rmcp::transport::streamable_http_server::session::local::LocalSessionManager,
    > = StreamableHttpService::new(
        move || Ok(BrokerServer::new(server_state.clone())),
        Default::default(),
        StreamableHttpServerConfig::default()
            .with_sse_keep_alive(sse_keep_alive)
            .with_sse_retry(sse_retry)
            .with_stateful_mode(stateful)
            .with_cancellation_token(cancellation_token.child_token()),
    );
    let router = Router::new().nest_service(path, service);
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .with_context(|| format!("binding broker MCP streamable HTTP listener on {listen}"))?;
    let local_addr = listener
        .local_addr()
        .context("reading broker listener address")?;

    tracing::info!(
        listen = %local_addr,
        path,
        stateful,
        sse_keep_alive_ms = sse_keep_alive.map(|duration| duration.as_millis() as u64),
        sse_retry_ms = sse_retry.map(|duration| duration.as_millis() as u64),
        "starting broker MCP streamable HTTP service"
    );

    let shutdown_token = cancellation_token.clone();
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            wait_for_shutdown_signal().await;
            shutdown_token.cancel();
        })
        .await
        .context("running broker MCP streamable HTTP service")?;

    tracing::info!("broker MCP streamable HTTP service stopped");
    Ok(())
}

fn duration_from_millis(value: Option<u64>) -> Option<std::time::Duration> {
    match value {
        Some(0) => None,
        Some(value) => Some(std::time::Duration::from_millis(value)),
        None => None,
    }
}

async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        match signal(SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = terminate.recv() => {}
                }
            }
            Err(err) => {
                tracing::warn!(
                    ?err,
                    "failed to install SIGTERM handler; falling back to ctrl-c"
                );
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }

    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

pub fn format_command_text(
    cmd: &str,
    response: &remote_exec_proto::rpc::ExecResponse,
    session_id: Option<&str>,
) -> String {
    let original = response
        .original_token_count
        .map(|count| format!("\nOriginal token count: {count}"))
        .unwrap_or_default();

    format!(
        "Command: {cmd}\nChunk ID: {}\nWall time: {:.3} seconds\n{}{original}\nOutput:\n{}",
        response
            .chunk_id
            .clone()
            .unwrap_or_else(|| "n/a".to_string()),
        response.wall_time_seconds,
        match (response.exit_code, session_id) {
            (Some(code), _) => format!("Process exited with code {code}"),
            (None, Some(id)) => format!("Process running with session ID {id}"),
            (None, None) => "Process running".to_string(),
        },
        response.output,
    )
}

pub fn format_poll_text(
    cmd: Option<&str>,
    response: &remote_exec_proto::rpc::ExecResponse,
    session_id: Option<&str>,
) -> String {
    let command = cmd
        .map(|cmd| format!("Command: {cmd}\n"))
        .unwrap_or_default();
    let original = response
        .original_token_count
        .map(|count| format!("\nOriginal token count: {count}"))
        .unwrap_or_default();
    let status = match (response.exit_code, session_id) {
        (Some(code), _) => format!("Process exited with code {code}"),
        (None, Some(id)) => format!("Process running with session ID {id}"),
        (None, None) => "Process running".to_string(),
    };

    format!(
        "{command}Chunk ID: {}\nWall time: {:.3} seconds\n{status}{original}\nOutput:\n{}",
        response
            .chunk_id
            .clone()
            .unwrap_or_else(|| "n/a".to_string()),
        response.wall_time_seconds,
        response.output
    )
}

pub fn format_intercepted_patch_text(output: &str) -> String {
    format!("Wall time: 0.0000 seconds\nProcess exited with code 0\nOutput:\n{output}")
}

pub fn prepend_warning_text(
    text: String,
    warnings: &[remote_exec_proto::rpc::ExecWarning],
) -> String {
    if warnings.is_empty() {
        return text;
    }

    let warning_text = if warnings.len() == 1 {
        format!("Warning: {}", warnings[0].message)
    } else {
        format!(
            "Warnings:\n{}",
            warnings
                .iter()
                .map(|warning| format!("- {}", warning.message))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };

    format!("{warning_text}\n\n{text}")
}

#[cfg(test)]
mod tests {
    use super::{
        format_command_text, format_intercepted_patch_text, format_poll_text, prepend_warning_text,
    };
    use remote_exec_proto::rpc::{ExecResponse, ExecWarning};

    #[test]
    fn format_command_text_includes_original_token_count_when_present() {
        let text = format_command_text(
            "printf hi",
            &ExecResponse {
                daemon_session_id: None,
                daemon_instance_id: "daemon-instance-1".to_string(),
                running: false,
                chunk_id: Some("abc123".to_string()),
                wall_time_seconds: 0.25,
                exit_code: Some(0),
                original_token_count: Some(6),
                output: "one two three".to_string(),
                warnings: Vec::new(),
            },
            None,
        );

        assert!(text.contains("Original token count: 6"));
    }

    #[test]
    fn format_poll_text_includes_original_token_count_when_present() {
        let text = format_poll_text(
            None,
            &ExecResponse {
                daemon_session_id: None,
                daemon_instance_id: "daemon-instance-1".to_string(),
                running: false,
                chunk_id: Some("abc123".to_string()),
                wall_time_seconds: 0.25,
                exit_code: Some(0),
                original_token_count: Some(6),
                output: "one two three".to_string(),
                warnings: Vec::new(),
            },
            None,
        );

        assert!(text.contains("Original token count: 6"));
    }

    #[test]
    fn format_poll_text_includes_command_when_present() {
        let text = format_poll_text(
            Some("printf hi"),
            &ExecResponse {
                daemon_session_id: None,
                daemon_instance_id: "daemon-instance-1".to_string(),
                running: false,
                chunk_id: Some("abc123".to_string()),
                wall_time_seconds: 0.25,
                exit_code: Some(0),
                original_token_count: Some(6),
                output: "one two three".to_string(),
                warnings: Vec::new(),
            },
            None,
        );

        assert!(text.starts_with("Command: printf hi\n"));
    }

    #[test]
    fn format_intercepted_patch_text_omits_command_and_chunk_metadata() {
        let text =
            format_intercepted_patch_text("Success. Updated the following files:\nA hello.txt\n");

        assert!(text.contains("Wall time: 0.0000 seconds"));
        assert!(text.contains("Process exited with code 0"));
        assert!(text.contains("Output:\nSuccess. Updated the following files:"));
        assert!(!text.contains("Command:"));
        assert!(!text.contains("Chunk ID:"));
    }

    #[test]
    fn prepend_warning_text_prefixes_single_warning() {
        let text = prepend_warning_text(
            "Process exited with code 0".to_string(),
            &[ExecWarning {
                code: "example".to_string(),
                message: "Visible warning".to_string(),
            }],
        );

        assert_eq!(
            text,
            "Warning: Visible warning\n\nProcess exited with code 0"
        );
    }
}
