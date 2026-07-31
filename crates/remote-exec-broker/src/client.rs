use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::Context;
use rmcp::{
    ClientHandler, RoleClient, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResult, ClientInfo, ContentBlock, PaginatedRequestParams,
    },
    service::RunningService,
    transport::StreamableHttpClientTransport,
    transport::streamable_http_client::StreamableHttpClientTransportConfig,
};
use serde_json::{Map, Value};

use crate::tools::registry::BrokerTool;

#[derive(Debug, Clone)]
pub enum Connection {
    Config { config_path: PathBuf },
    StreamableHttp { url: String },
}

pub struct RemoteExecClient {
    transport: ClientTransport,
}

enum ClientTransport {
    Direct(DirectBrokerClient),
    Mcp(McpBrokerClient),
}

struct DirectBrokerClient {
    state: crate::BrokerState,
}

struct McpBrokerClient {
    service: RunningService<RoleClient, RemoteExecClientHandler>,
    tool_names: BTreeSet<String>,
}

impl RemoteExecClient {
    pub async fn connect(connection: Connection) -> anyhow::Result<Self> {
        let transport = match connection {
            Connection::Config { config_path } => {
                ClientTransport::Direct(connect_direct(&config_path).await?)
            }
            Connection::StreamableHttp { url } => {
                ClientTransport::Mcp(connect_streamable_http(&url).await?)
            }
        };

        Ok(Self { transport })
    }

    pub async fn call_tool<T>(&self, name: &str, arguments: &T) -> anyhow::Result<ToolResponse>
    where
        T: serde::Serialize + ?Sized,
    {
        let arguments = serde_json::to_value(arguments)
            .with_context(|| format!("serializing arguments for `{name}`"))?;
        let arguments = arguments
            .as_object()
            .cloned()
            .with_context(|| format!("tool `{name}` arguments must serialize to a JSON object"))?;

        let result = match &self.transport {
            ClientTransport::Direct(client) => client.call_tool(name, arguments).await,
            ClientTransport::Mcp(client) => {
                let mcp_name = client.tool_name(name);
                let result = client
                    .service
                    .call_tool(
                        CallToolRequestParams::new(mcp_name.clone()).with_arguments(arguments),
                    )
                    .await
                    .with_context(|| format!("calling `{mcp_name}`"))?;
                ToolResponse::from_call_tool_result(result)
            }
        };

        Ok(result)
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ToolResponse {
    pub is_error: bool,
    pub text_output: String,
    pub structured_content: serde_json::Value,
    pub raw_content: Vec<serde_json::Value>,
}

impl ToolResponse {
    fn from_call_tool_result(result: CallToolResult) -> Self {
        let text_output = result
            .content
            .iter()
            .filter_map(|content| content.as_text().map(|text| text.text.as_str()))
            .collect::<Vec<_>>()
            .join("\n");
        let raw_content = result.content.iter().map(normalize_content).collect();

        Self {
            is_error: result.is_error.unwrap_or(false),
            text_output,
            structured_content: result.structured_content.unwrap_or(serde_json::Value::Null),
            raw_content,
        }
    }

    pub fn first_image_url(&self) -> Option<String> {
        self.structured_content
            .get("image_url")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| {
                self.raw_content.iter().find_map(|content| {
                    content
                        .get("type")
                        .and_then(serde_json::Value::as_str)
                        .filter(|kind| *kind == "input_image")
                        .and_then(|_| content.get("image_url"))
                        .and_then(serde_json::Value::as_str)
                        .map(ToOwned::to_owned)
                })
            })
    }
}

#[derive(Debug, Clone, Default)]
struct RemoteExecClientHandler;

const CLIENT_NAME: &str = "remote-exec";

impl ClientHandler for RemoteExecClientHandler {
    fn get_info(&self) -> ClientInfo {
        let mut info = ClientInfo::default();
        info.client_info.name = CLIENT_NAME.to_string();
        info.client_info.version = env!("CARGO_PKG_VERSION").to_string();
        info
    }
}

async fn connect_direct(config_path: &Path) -> anyhow::Result<DirectBrokerClient> {
    crate::install_crypto_provider()?;
    let config = crate::config::BrokerConfig::load(config_path).await?;
    let state = crate::build_state(config).await?;
    Ok(DirectBrokerClient { state })
}

async fn connect_streamable_http(url: &str) -> anyhow::Result<McpBrokerClient> {
    crate::broker_tls::ensure_broker_url_supported(url)?;
    crate::install_crypto_provider()?;
    let client = reqwest::Client::builder()
        .build()
        .context("building streamable HTTP reqwest client")?;
    let transport = StreamableHttpClientTransport::with_client(
        client,
        StreamableHttpClientTransportConfig::with_uri(url.to_string()),
    );
    let service = RemoteExecClientHandler
        .serve(transport)
        .await
        .context("connecting to broker over streamable HTTP")?;
    let tools = service
        .list_tools(Some(PaginatedRequestParams::default()))
        .await
        .context("listing broker MCP tools")?;

    Ok(McpBrokerClient {
        service,
        tool_names: tools
            .tools
            .into_iter()
            .map(|tool| tool.name.into_owned())
            .collect(),
    })
}

impl DirectBrokerClient {
    async fn call_tool(&self, name: &str, arguments: Map<String, Value>) -> ToolResponse {
        ToolResponse::from_call_tool_result(
            BrokerTool::call_direct(
                &self.state,
                name,
                arguments,
                !self.state.disable_structured_content,
            )
            .await,
        )
    }
}

impl McpBrokerClient {
    fn tool_name(&self, name: &str) -> String {
        resolve_mcp_tool_name(&self.tool_names, name)
    }
}

fn resolve_mcp_tool_name(tool_names: &BTreeSet<String>, name: &str) -> String {
    if tool_names.contains(name) {
        return name.to_string();
    }

    let prefixed = format!("remote_{name}");
    if tool_names.contains(&prefixed) {
        prefixed
    } else {
        name.to_string()
    }
}

fn normalize_content(content: &ContentBlock) -> serde_json::Value {
    if let Some(text) = content.as_text() {
        return serde_json::json!({
            "type": "text",
            "text": text.text,
        });
    }

    if let Some(image) = content.as_image() {
        return serde_json::json!({
            "type": "input_image",
            "image_url": format!("data:{};base64,{}", image.mime_type, image.data),
        });
    }

    serde_json::to_value(content).unwrap_or_else(|err| {
        tracing::warn!(error = %err, "failed to serialize raw MCP content");
        serde_json::json!({
            "type": "unsupported_content",
            "error": err.to_string(),
        })
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::resolve_mcp_tool_name;

    #[test]
    fn resolves_prefixed_and_legacy_mcp_tool_names() {
        let prefixed = BTreeSet::from(["remote_exec_command".to_string()]);
        assert_eq!(
            resolve_mcp_tool_name(&prefixed, "exec_command"),
            "remote_exec_command"
        );
        assert_eq!(
            resolve_mcp_tool_name(&prefixed, "remote_exec_command"),
            "remote_exec_command"
        );

        let legacy = BTreeSet::from(["exec_command".to_string()]);
        assert_eq!(
            resolve_mcp_tool_name(&legacy, "exec_command"),
            "exec_command"
        );
    }
}
