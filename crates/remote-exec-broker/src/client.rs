use std::path::{Path, PathBuf};

use anyhow::Context;
use rmcp::{
    ClientHandler, RoleClient, ServiceExt,
    model::{CallToolRequestParams, CallToolResult, ClientInfo},
    service::RunningService,
    transport::{StreamableHttpClientTransport, TokioChildProcess},
};

#[derive(Debug, Clone)]
pub enum Connection {
    Stdio {
        broker_bin: PathBuf,
        config_path: PathBuf,
    },
    StreamableHttp {
        url: String,
    },
}

pub struct RemoteExecClient {
    service: RunningService<RoleClient, RemoteExecClientHandler>,
}

impl RemoteExecClient {
    pub async fn connect(connection: Connection) -> anyhow::Result<Self> {
        let service = match connection {
            Connection::Stdio {
                broker_bin,
                config_path,
            } => connect_stdio(&broker_bin, &config_path).await?,
            Connection::StreamableHttp { url } => connect_streamable_http(&url).await?,
        };

        Ok(Self { service })
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
        let result = self
            .service
            .call_tool(CallToolRequestParams::new(name.to_string()).with_arguments(arguments))
            .await
            .with_context(|| format!("calling `{name}`"))?;

        Ok(ToolResponse::from_call_tool_result(result))
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
            .filter_map(|content| content.raw.as_text().map(|text| text.text.as_str()))
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

impl ClientHandler for RemoteExecClientHandler {
    fn get_info(&self) -> ClientInfo {
        let mut info = ClientInfo::default();
        info.client_info.name = "remote-exec".to_string();
        info.client_info.version = env!("CARGO_PKG_VERSION").to_string();
        info
    }
}

async fn connect_stdio(
    broker_bin: &Path,
    config_path: &Path,
) -> anyhow::Result<RunningService<RoleClient, RemoteExecClientHandler>> {
    let mut command = tokio::process::Command::new(broker_bin);
    command.arg(config_path);
    let transport = TokioChildProcess::new(command).context("starting broker child transport")?;
    RemoteExecClientHandler
        .serve(transport)
        .await
        .context("connecting to broker over stdio")
}

async fn connect_streamable_http(
    url: &str,
) -> anyhow::Result<RunningService<RoleClient, RemoteExecClientHandler>> {
    RemoteExecClientHandler
        .serve(StreamableHttpClientTransport::from_uri(url.to_string()))
        .await
        .context("connecting to broker over streamable HTTP")
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

    serde_json::to_value(content).expect("serializing raw MCP content")
}
