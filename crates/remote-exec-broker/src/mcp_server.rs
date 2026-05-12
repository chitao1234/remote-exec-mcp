use anyhow::Context;
use axum::Router;
use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Content, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
    transport::{StreamableHttpServerConfig, StreamableHttpService},
};
use std::future::Future;
use tokio_util::sync::CancellationToken;

use crate::request_context::RequestContext;
use crate::tools::registry::BrokerTool;

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
    let message = err.to_string();
    if let Some(context) = crate::request_context::current() {
        tracing::warn!(
            request_id = %context.request_id(),
            tool = context.tool(),
            target = context.target().unwrap_or("-"),
            error = %message,
            "broker tool returned error"
        );
        return tool_error_result(format_correlated_error(&message, &context));
    }

    tracing::warn!(error = %message, "broker tool returned error");
    tool_error_result(message)
}

fn format_correlated_error(
    message: &str,
    context: &crate::request_context::RequestContextSnapshot,
) -> String {
    match context.target() {
        Some(target) => format!(
            "request_id={} tool={} target={}: {}",
            context.request_id(),
            context.tool(),
            target,
            message
        ),
        None => format!(
            "request_id={} tool={}: {}",
            context.request_id(),
            context.tool(),
            message
        ),
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

    fn include_structured_content(&self) -> bool {
        !self.state.disable_structured_content
    }

    fn finish_tool_call(&self, result: anyhow::Result<ToolCallOutput>) -> CallToolResult {
        match result {
            Ok(output) => output.into_call_tool_result(self.include_structured_content()),
            Err(err) => format_tool_error(err),
        }
    }

    async fn finish_scoped_tool_call<F>(&self, tool: BrokerTool, future: F) -> CallToolResult
    where
        F: Future<Output = anyhow::Result<ToolCallOutput>>,
    {
        let context = RequestContext::new(tool.name());
        crate::request_context::scope(context.clone(), async {
            tracing::debug!(
                request_id = %context.request_id(),
                tool = context.tool(),
                "broker tool request context created"
            );
            self.finish_tool_call(future.await)
        })
        .await
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
        Ok(self
            .finish_scoped_tool_call(
                BrokerTool::ListTargets,
                crate::tools::targets::list_targets(&self.state, input),
            )
            .await)
    }

    #[tool(
        name = "exec_command",
        description = "Run a command on a configured target machine."
    )]
    async fn exec_command(
        &self,
        Parameters(input): Parameters<remote_exec_proto::public::ExecCommandInput>,
    ) -> Result<CallToolResult, McpError> {
        Ok(self
            .finish_scoped_tool_call(
                BrokerTool::ExecCommand,
                crate::tools::exec::exec_command(&self.state, input),
            )
            .await)
    }

    #[tool(
        name = "write_stdin",
        description = "Write to or poll an existing exec_command session."
    )]
    async fn write_stdin(
        &self,
        Parameters(input): Parameters<remote_exec_proto::public::WriteStdinInput>,
    ) -> Result<CallToolResult, McpError> {
        Ok(self
            .finish_scoped_tool_call(
                BrokerTool::WriteStdin,
                crate::tools::exec::write_stdin(&self.state, input),
            )
            .await)
    }

    #[tool(
        name = "apply_patch",
        description = "Apply a patch on a configured target machine."
    )]
    async fn apply_patch(
        &self,
        Parameters(input): Parameters<remote_exec_proto::public::ApplyPatchInput>,
    ) -> Result<CallToolResult, McpError> {
        Ok(self
            .finish_scoped_tool_call(
                BrokerTool::ApplyPatch,
                crate::tools::patch::apply_patch(&self.state, input),
            )
            .await)
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
        Ok(self
            .finish_scoped_tool_call(
                BrokerTool::ViewImage,
                crate::tools::image::view_image(&self.state, input),
            )
            .await)
    }

    #[tool(
        name = "transfer_files",
        description = "Transfer one file or one directory tree between broker-local and configured target filesystems."
    )]
    async fn transfer_files(
        &self,
        Parameters(input): Parameters<remote_exec_proto::public::TransferFilesInput>,
    ) -> Result<CallToolResult, McpError> {
        Ok(self
            .finish_scoped_tool_call(
                BrokerTool::TransferFiles,
                crate::tools::transfer::transfer_files(&self.state, input),
            )
            .await)
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
            .finish_scoped_tool_call(
                BrokerTool::ForwardPorts,
                crate::tools::port_forward::forward_ports(&self.state, input),
            )
            .await)
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

#[cfg(test)]
mod tests {
    use super::format_tool_error;
    use crate::request_context::RequestContext;

    fn error_text(result: rmcp::model::CallToolResult) -> String {
        result.content[0]
            .raw
            .as_text()
            .expect("text content")
            .text
            .to_string()
    }

    #[tokio::test]
    async fn tool_errors_include_request_context_and_preserve_suffix() {
        let context = RequestContext::new("exec_command");
        context.set_target("builder-a");

        let text = crate::request_context::scope(context, async {
            error_text(format_tool_error(anyhow::anyhow!("daemon unavailable")))
        })
        .await;

        assert!(text.starts_with("request_id=req_"), "{text}");
        assert!(
            text.contains(" tool=exec_command target=builder-a: "),
            "{text}"
        );
        assert!(text.ends_with("daemon unavailable"), "{text}");
    }

    #[tokio::test]
    async fn tool_errors_omit_unknown_target_context() {
        let context = RequestContext::new("list_targets");

        let text = crate::request_context::scope(context, async {
            error_text(format_tool_error(anyhow::anyhow!("bad list")))
        })
        .await;

        assert!(text.starts_with("request_id=req_"), "{text}");
        assert!(text.contains(" tool=list_targets: "), "{text}");
        assert!(!text.contains(" target="), "{text}");
        assert!(text.ends_with("bad list"), "{text}");
    }
}
