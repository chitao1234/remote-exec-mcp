use crate::config::BrokerToolsConfig;

use anyhow::Context;
use rmcp::{
    ErrorData as McpError,
    handler::server::{tool::schema_for_type, wrapper::Parameters},
    model::{CallToolResult, JsonObject, Tool, ToolAnnotations},
};
use serde::de::DeserializeOwned;
use serde_json::Value;

use remote_exec_proto::public::{
    ApplyPatchInput, EditInput, ExecCommandInput, ForwardPortsInput, ListTargetsInput, ReadInput,
    TransferFilesInput, ViewImageInput, WriteInput, WriteStdinInput,
};

macro_rules! broker_tools {
    ($macro:ident) => {
        $macro! {
            ListTargets {
                name = "list_targets",
                description = "List configured target names.",
                input = ListTargetsInput,
                handler = crate::tools::targets::list_targets,
                read_only = true,
                enabled = always,
            }
            ExecCommand {
                name = "exec_command",
                description = "Run a command on a configured target machine.",
                input = ExecCommandInput,
                handler = crate::tools::exec::exec_command,
                read_only = false,
                enabled = always,
            }
            WriteStdin {
                name = "write_stdin",
                description = "Write to or poll an existing exec_command session.",
                input = WriteStdinInput,
                handler = crate::tools::exec::write_stdin,
                read_only = false,
                enabled = always,
            }
            ApplyPatch {
                name = "apply_patch",
                description = "Apply a patch on a configured target machine.",
                input = ApplyPatchInput,
                handler = crate::tools::patch::apply_patch,
                read_only = false,
                enabled = always,
            }
            ViewImage {
                name = "view_image",
                description = "Read an image from a configured target machine.",
                input = ViewImageInput,
                handler = crate::tools::image::view_image,
                read_only = true,
                enabled = always,
            }
            TransferFiles {
                name = "transfer_files",
                description = "Transfer one file or one directory tree between broker-local and configured target filesystems.",
                input = TransferFilesInput,
                handler = crate::tools::transfer::transfer_files,
                read_only = false,
                enabled = always,
            }
            ForwardPorts {
                name = "forward_ports",
                description = "Open, list, or close TCP/UDP port forwards between broker-local and configured target machines.",
                input = ForwardPortsInput,
                handler = crate::tools::port_forward::forward_ports,
                read_only = false,
                enabled = always,
            }
            Read {
                name = "read",
                description = "Read a text file from a configured target machine.",
                input = ReadInput,
                handler = crate::tools::file::read,
                read_only = true,
                enabled = file_read,
            }
            Write {
                name = "write",
                description = "Overwrite or create a text file on a configured target machine.",
                input = WriteInput,
                handler = crate::tools::file::write,
                read_only = false,
                enabled = file_write,
            }
            Edit {
                name = "edit",
                description = "Replace text in a file on a configured target machine.",
                input = EditInput,
                handler = crate::tools::file::edit,
                read_only = false,
                enabled = file_edit,
            }
        }
    };
}

macro_rules! define_broker_tool_enum {
    ($(
        $variant:ident {
            name = $name:literal,
            description = $description:literal,
            input = $input:ty,
            handler = $handler:path,
            read_only = $read_only:tt,
            enabled = $enabled:ident,
        }
    )*) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub(crate) enum BrokerTool {
            $($variant,)*
        }
    };
}

broker_tools!(define_broker_tool_enum);

macro_rules! broker_tool_read_only_hint {
    (true) => {
        Some(true)
    };
    (false) => {
        None
    };
}

macro_rules! broker_tool_enabled {
    ($tools:expr, always) => {
        true
    };
    ($tools:expr, file_read) => {
        $tools.file.read
    };
    ($tools:expr, file_write) => {
        $tools.file.write
    };
    ($tools:expr, file_edit) => {
        $tools.file.edit
    };
}

macro_rules! define_broker_tool_impl {
    ($(
        $variant:ident {
            name = $name:literal,
            description = $description:literal,
            input = $input:ty,
            handler = $handler:path,
            read_only = $read_only:tt,
            enabled = $enabled:ident,
        }
    )*) => {
        pub(crate) const ALL: &'static [Self] = &[
            $(Self::$variant,)*
        ];

        pub(crate) fn from_name(name: &str) -> Option<Self> {
            match name {
                $($name => Some(Self::$variant),)*
                _ => None,
            }
        }

        pub(crate) async fn call_direct(
            state: &crate::BrokerState,
            name: &str,
            arguments: JsonObject,
            include_structured_content: bool,
        ) -> CallToolResult {
            let Some(tool) = Self::from_name(name) else {
                return crate::mcp_server::tool_error_result(format!("unknown tool `{name}`"));
            };
            if !tool.enabled_by_config(&state.tools) {
                return crate::mcp_server::tool_error_result(format!("unknown tool `{name}`"));
            }

            tool.call_direct_inner(state, arguments, include_structured_content)
                .await
        }

        pub(crate) async fn call_mcp(
            self,
            state: &crate::BrokerState,
            mcp_name: String,
            arguments: JsonObject,
            include_structured_content: bool,
        ) -> Result<CallToolResult, McpError> {
            match self {
                $(Self::$variant => {
                    let input = deserialize_mcp_arguments::<$input>(arguments)?;
                    Ok(crate::mcp_server::finish_scoped_tool_call_named(
                        mcp_name,
                        include_structured_content,
                        Box::pin($handler(state, input)),
                    )
                    .await)
                },)*
            }
        }

        async fn call_direct_inner(
            self,
            state: &crate::BrokerState,
            arguments: JsonObject,
            include_structured_content: bool,
        ) -> CallToolResult {
            match self {
                $(Self::$variant => {
                    crate::mcp_server::finish_scoped_tool_call(
                        self,
                        include_structured_content,
                        Box::pin(async move {
                            let input = deserialize_direct_arguments::<$input>(self.name(), arguments)?;
                            $handler(state, input).await
                        }),
                    )
                    .await
                },)*
            }
        }

        pub(crate) const fn name(self) -> &'static str {
            match self {
                $(Self::$variant => $name,)*
            }
        }

        pub(crate) fn mcp_name(self, prepend_tool_names: bool) -> String {
            if prepend_tool_names {
                format!("remote_{}", self.name())
            } else {
                self.name().to_string()
            }
        }

        pub(crate) const fn description(self) -> &'static str {
            match self {
                $(Self::$variant => $description,)*
            }
        }

        pub(crate) const fn read_only_hint(self) -> Option<bool> {
            match self {
                $(Self::$variant => broker_tool_read_only_hint!($read_only),)*
            }
        }

        pub(crate) fn enabled_by_config(self, tools: &BrokerToolsConfig) -> bool {
            match self {
                $(Self::$variant => broker_tool_enabled!(tools, $enabled),)*
            }
        }

        pub(crate) fn mcp_tool(self, mcp_name: String) -> Tool {
            let mut tool = Tool::new_with_raw(
                mcp_name,
                Some(self.description().into()),
                self.input_schema(),
            );
            if let Some(read_only_hint) = self.read_only_hint() {
                tool = tool.with_annotations(ToolAnnotations::from_raw(
                    None,
                    Some(read_only_hint),
                    None,
                    None,
                    None,
                ));
            }
            tool
        }

        fn input_schema(self) -> std::sync::Arc<JsonObject> {
            match self {
                $(Self::$variant => schema_for_type::<Parameters<$input>>(),)*
            }
        }
    };
}

impl BrokerTool {
    broker_tools!(define_broker_tool_impl);
}

fn deserialize_direct_arguments<T>(name: &str, arguments: JsonObject) -> anyhow::Result<T>
where
    T: DeserializeOwned,
{
    serde_json::from_value(Value::Object(arguments))
        .with_context(|| format!("deserializing arguments for `{name}`"))
}

fn deserialize_mcp_arguments<T>(arguments: JsonObject) -> Result<T, McpError>
where
    T: DeserializeOwned,
{
    serde_json::from_value(Value::Object(arguments)).map_err(|error| {
        McpError::invalid_params(format!("failed to deserialize parameters: {error}"), None)
    })
}

#[cfg(test)]
mod tests {
    use super::BrokerTool;

    #[test]
    fn all_tool_names_round_trip_through_registry() {
        for tool in BrokerTool::ALL {
            let name = tool.name();
            assert_eq!(
                BrokerTool::from_name(name).expect("registered tool should parse"),
                *tool
            );
        }
    }
}
