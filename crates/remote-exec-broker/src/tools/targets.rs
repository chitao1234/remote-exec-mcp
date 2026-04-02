use remote_exec_proto::public::{ListTargetsInput, ListTargetsResult};

use crate::mcp_server::ToolCallOutput;

pub async fn list_targets(
    state: &crate::BrokerState,
    _input: ListTargetsInput,
) -> anyhow::Result<ToolCallOutput> {
    let targets = state.targets.keys().cloned().collect::<Vec<_>>();
    let text = format_targets_text(&targets);

    Ok(ToolCallOutput::text_and_structured(
        text,
        serde_json::to_value(ListTargetsResult { targets })?,
    ))
}

fn format_targets_text(targets: &[String]) -> String {
    format!("Configured targets:\n- {}", targets.join("\n- "))
}
