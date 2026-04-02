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
    if targets.is_empty() {
        return "No configured targets.".to_string();
    }

    format!("Configured targets:\n- {}", targets.join("\n- "))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use remote_exec_proto::public::ListTargetsInput;

    use super::list_targets;
    use crate::{BrokerState, session_store::SessionStore};

    #[tokio::test]
    async fn list_targets_returns_empty_text_and_array_for_empty_state() {
        let state = BrokerState {
            sessions: SessionStore::default(),
            targets: BTreeMap::new(),
        };

        let result = list_targets(&state, ListTargetsInput {}).await.unwrap();
        let call_result = result.into_call_tool_result();
        let text = call_result
            .content
            .iter()
            .filter_map(|content| content.raw.as_text().map(|text| text.text.as_str()))
            .collect::<Vec<_>>()
            .join("\n");

        assert_eq!(text, "No configured targets.");
        assert_eq!(
            call_result.structured_content,
            Some(serde_json::json!({ "targets": [] }))
        );
    }
}
