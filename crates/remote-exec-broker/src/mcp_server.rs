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

    fn finish_tool_call(&self, result: anyhow::Result<ToolCallOutput>) -> CallToolResult {
        match result {
            Ok(output) => output.into_call_tool_result(self.include_structured_content()),
            Err(err) => format_tool_error(err),
        }
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
        Ok(self.finish_tool_call(crate::tools::targets::list_targets(&self.state, input).await))
    }

    #[tool(
        name = "exec_command",
        description = "Run a command on a configured target machine."
    )]
    async fn exec_command(
        &self,
        Parameters(input): Parameters<remote_exec_proto::public::ExecCommandInput>,
    ) -> Result<CallToolResult, McpError> {
        Ok(self.finish_tool_call(crate::tools::exec::exec_command(&self.state, input).await))
    }

    #[tool(
        name = "write_stdin",
        description = "Write to or poll an existing exec_command session."
    )]
    async fn write_stdin(
        &self,
        Parameters(input): Parameters<remote_exec_proto::public::WriteStdinInput>,
    ) -> Result<CallToolResult, McpError> {
        Ok(self.finish_tool_call(crate::tools::exec::write_stdin(&self.state, input).await))
    }

    #[tool(
        name = "apply_patch",
        description = "Apply a patch on a configured target machine."
    )]
    async fn apply_patch(
        &self,
        Parameters(input): Parameters<remote_exec_proto::public::ApplyPatchInput>,
    ) -> Result<CallToolResult, McpError> {
        Ok(self.finish_tool_call(crate::tools::patch::apply_patch(&self.state, input).await))
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
        Ok(self.finish_tool_call(crate::tools::image::view_image(&self.state, input).await))
    }

    #[tool(
        name = "transfer_files",
        description = "Transfer one file or one directory tree between broker-local and configured target filesystems."
    )]
    async fn transfer_files(
        &self,
        Parameters(input): Parameters<remote_exec_proto::public::TransferFilesInput>,
    ) -> Result<CallToolResult, McpError> {
        Ok(self.finish_tool_call(crate::tools::transfer::transfer_files(&self.state, input).await))
    }

    #[tool(
        name = "forward_ports",
        description = "Open, list, or close TCP/UDP port forwards between broker-local and configured target machines."
    )]
    async fn forward_ports(
        &self,
        Parameters(input): Parameters<remote_exec_proto::public::ForwardPortsInput>,
    ) -> Result<CallToolResult, McpError> {
        Ok(self
            .finish_tool_call(crate::tools::port_forward::forward_ports(&self.state, input).await))
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
    let server = BrokerServer::new(state.clone());
    let result = server
        .serve(rmcp::transport::stdio())
        .await
        .context("starting broker MCP service")?
        .waiting()
        .await
        .context("waiting for broker MCP service");
    crate::port_forward::close_all(&state.port_forwards).await;
    result?;
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
            sse_keep_alive,
            sse_retry,
        } => {
            serve_streamable_http(
                state,
                *listen,
                path,
                *stateful,
                sse_keep_alive.as_duration(),
                sse_retry.as_duration(),
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
    write_test_bound_addr_file(local_addr).await?;

    tracing::info!(
        listen = %local_addr,
        path,
        stateful,
        sse_keep_alive_ms = sse_keep_alive.map(|duration| duration.as_millis() as u64),
        sse_retry_ms = sse_retry.map(|duration| duration.as_millis() as u64),
        "starting broker MCP streamable HTTP service"
    );

    let shutdown_token = cancellation_token.clone();
    let result = axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            wait_for_shutdown_signal().await;
            shutdown_token.cancel();
        })
        .await
        .context("running broker MCP streamable HTTP service");
    crate::port_forward::close_all(&state.port_forwards).await;
    result?;

    tracing::info!("broker MCP streamable HTTP service stopped");
    Ok(())
}

async fn write_test_bound_addr_file(local_addr: std::net::SocketAddr) -> anyhow::Result<()> {
    let Some(path) = std::env::var_os("REMOTE_EXEC_BROKER_TEST_BOUND_ADDR_FILE") else {
        return Ok(());
    };
    tokio::fs::write(&path, format!("{local_addr}\n"))
        .await
        .with_context(|| {
            format!(
                "writing broker test bound address file {}",
                std::path::Path::new(&path).display()
            )
        })
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
