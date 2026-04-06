use remote_exec_proto::public::{
    ListTargetDaemonInfo, ListTargetEntry, ListTargetsInput, ListTargetsResult,
};

use crate::mcp_server::ToolCallOutput;

pub async fn list_targets(
    state: &crate::BrokerState,
    _input: ListTargetsInput,
) -> anyhow::Result<ToolCallOutput> {
    tracing::info!(tool = "list_targets", "broker tool started");
    let mut targets = Vec::with_capacity(state.targets.len());
    for (name, handle) in &state.targets {
        let daemon_info = handle
            .cached_daemon_info()
            .await
            .map(|info| ListTargetDaemonInfo {
                daemon_version: info.daemon_version,
                hostname: info.hostname,
                platform: info.platform,
                arch: info.arch,
                supports_pty: info.supports_pty,
            });
        targets.push(ListTargetEntry {
            name: name.clone(),
            daemon_info,
        });
    }
    let text = format_targets_text(&targets);
    let reachable = targets
        .iter()
        .filter(|target| target.daemon_info.is_some())
        .count();
    tracing::info!(
        tool = "list_targets",
        configured_targets = targets.len(),
        reachable_targets = reachable,
        "broker tool completed"
    );

    Ok(ToolCallOutput::text_and_structured(
        text,
        serde_json::to_value(ListTargetsResult { targets })?,
    ))
}

fn format_targets_text(targets: &[ListTargetEntry]) -> String {
    if targets.is_empty() {
        return "No configured targets.".to_string();
    }

    let lines = targets
        .iter()
        .map(|target| match &target.daemon_info {
            Some(info) => format!(
                "- {}: {}/{}, host={}, version={}, pty={}",
                target.name,
                info.platform,
                info.arch,
                info.hostname,
                info.daemon_version,
                if info.supports_pty { "yes" } else { "no" },
            ),
            None => format!("- {}", target.name),
        })
        .collect::<Vec<_>>();

    format!("Configured targets:\n{}", lines.join("\n"))
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
            enable_transfer_compression: true,
            disable_structured_content: false,
            host_sandbox: None,
            sessions: SessionStore::default(),
            targets: BTreeMap::new(),
        };

        let result = list_targets(&state, ListTargetsInput {}).await.unwrap();
        let call_result = result.into_call_tool_result(true);
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
