use std::path::{Path, PathBuf};

use anyhow::Context;
use rmcp::{
    ClientHandler, RoleClient, ServiceExt,
    model::{CallToolRequestParams, CallToolResult, ClientInfo},
    service::RunningService,
    transport::StreamableHttpClientTransport,
    transport::streamable_http_client::StreamableHttpClientTransportConfig,
};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use remote_exec_proto::public::{
    ApplyPatchInput, ExecCommandInput, ForwardPortsInput, ListTargetsInput, TransferFilesInput,
    ViewImageInput, WriteStdinInput,
};

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
    Mcp(RunningService<RoleClient, RemoteExecClientHandler>),
}

struct DirectBrokerClient {
    state: crate::BrokerState,
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
            ClientTransport::Mcp(service) => {
                let result = service
                    .call_tool(
                        CallToolRequestParams::new(name.to_string()).with_arguments(arguments),
                    )
                    .await
                    .with_context(|| format!("calling `{name}`"))?;
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

async fn connect_direct(config_path: &Path) -> anyhow::Result<DirectBrokerClient> {
    crate::install_crypto_provider();
    let config = crate::config::BrokerConfig::load(config_path).await?;
    let state = crate::build_state(config).await?;
    Ok(DirectBrokerClient { state })
}

async fn connect_streamable_http(
    url: &str,
) -> anyhow::Result<RunningService<RoleClient, RemoteExecClientHandler>> {
    crate::broker_tls::ensure_broker_url_supported(url)?;
    crate::install_crypto_provider();
    let client = reqwest::Client::builder()
        .build()
        .context("building streamable HTTP reqwest client")?;
    let transport = StreamableHttpClientTransport::with_client(
        client,
        StreamableHttpClientTransportConfig::with_uri(url.to_string()),
    );
    RemoteExecClientHandler
        .serve(transport)
        .await
        .context("connecting to broker over streamable HTTP")
}

impl DirectBrokerClient {
    async fn call_tool(&self, name: &str, arguments: Map<String, Value>) -> ToolResponse {
        ToolResponse::from_call_tool_result(call_direct_tool(&self.state, name, arguments).await)
    }
}

async fn call_direct_tool(
    state: &crate::BrokerState,
    name: &str,
    arguments: Map<String, Value>,
) -> CallToolResult {
    macro_rules! invoke_tool {
        ($input:ty, $handler:path) => {{
            let input = match deserialize_tool_arguments::<$input>(name, arguments) {
                Ok(input) => input,
                Err(err) => return crate::mcp_server::tool_error_result(err.to_string()),
            };

            match $handler(state, input).await {
                Ok(output) => output.into_call_tool_result(!state.disable_structured_content),
                Err(err) => crate::mcp_server::format_tool_error(err),
            }
        }};
    }

    match name {
        "list_targets" => invoke_tool!(ListTargetsInput, crate::tools::targets::list_targets),
        "exec_command" => invoke_tool!(ExecCommandInput, crate::tools::exec::exec_command),
        "write_stdin" => invoke_tool!(WriteStdinInput, crate::tools::exec::write_stdin),
        "apply_patch" => invoke_tool!(ApplyPatchInput, crate::tools::patch::apply_patch),
        "view_image" => invoke_tool!(ViewImageInput, crate::tools::image::view_image),
        "transfer_files" => {
            invoke_tool!(TransferFilesInput, crate::tools::transfer::transfer_files)
        }
        "forward_ports" => {
            invoke_tool!(ForwardPortsInput, crate::tools::port_forward::forward_ports)
        }
        _ => crate::mcp_server::tool_error_result(format!("unknown tool `{name}`")),
    }
}

fn deserialize_tool_arguments<T>(name: &str, arguments: Map<String, Value>) -> anyhow::Result<T>
where
    T: DeserializeOwned,
{
    serde_json::from_value(Value::Object(arguments))
        .with_context(|| format!("deserializing arguments for `{name}`"))
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
