use remote_exec_proto::public::{
    ListTargetDaemonInfo, ListTargetEntry, ListTargetsInput, ListTargetsResult, TargetHealthStatus,
};

use crate::{CachedDaemonInfo, mcp_server::ToolCallOutput, target::CachedTargetHealthStatus};

pub async fn list_targets(
    state: &crate::BrokerState,
    _input: ListTargetsInput,
) -> anyhow::Result<ToolCallOutput> {
    // list_targets can report multiple configured targets, including unavailable ones,
    // so a synthetic single-target request context would be misleading.
    let snapshots = state.target_status_snapshots().await;
    let mut targets = Vec::with_capacity(snapshots.len());
    for snapshot in snapshots {
        targets.push(ListTargetEntry {
            name: snapshot.name,
            healthy: snapshot.healthy,
            health_status: public_health_status(snapshot.health_status),
            daemon_info: snapshot.daemon_info.map(public_daemon_info),
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
        "list targets summarized"
    );

    Ok(ToolCallOutput::text_and_structured(
        text,
        serde_json::to_value(ListTargetsResult { targets })?,
    ))
}

fn public_daemon_info(info: CachedDaemonInfo) -> ListTargetDaemonInfo {
    ListTargetDaemonInfo::from_identity_and_capabilities(info.identity, &info.capabilities)
}

fn public_health_status(status: Option<CachedTargetHealthStatus>) -> TargetHealthStatus {
    match status {
        None => TargetHealthStatus::Unknown,
        Some(CachedTargetHealthStatus::Healthy) => TargetHealthStatus::Healthy,
        Some(CachedTargetHealthStatus::MaybeUnhealthy) => TargetHealthStatus::MaybeUnhealthy,
        Some(CachedTargetHealthStatus::Unhealthy) => TargetHealthStatus::Unhealthy,
    }
}

fn format_targets_text(targets: &[ListTargetEntry]) -> String {
    if targets.is_empty() {
        return "No configured targets.".to_string();
    }

    let lines = targets
        .iter()
        .map(
            |target| match (&target.health_status, &target.daemon_info) {
                (TargetHealthStatus::Healthy, Some(info)) => format!(
                    "- {}: healthy, {}/{}, host={}, version={}, pty={}, forward_ports={}",
                    target.name,
                    info.identity.platform.as_str(),
                    info.identity.arch.as_str(),
                    info.identity.hostname.as_str(),
                    info.identity.daemon_version.as_str(),
                    if info.supports_pty { "yes" } else { "no" },
                    if info.supports_port_forward {
                        "yes"
                    } else {
                        "no"
                    },
                ),
                (TargetHealthStatus::MaybeUnhealthy, Some(info)) => format!(
                    "- {}: maybe unhealthy, {}/{}, host={}, version={}, pty={}, forward_ports={}",
                    target.name,
                    info.identity.platform.as_str(),
                    info.identity.arch.as_str(),
                    info.identity.hostname.as_str(),
                    info.identity.daemon_version.as_str(),
                    if info.supports_pty { "yes" } else { "no" },
                    if info.supports_port_forward {
                        "yes"
                    } else {
                        "no"
                    },
                ),
                (TargetHealthStatus::MaybeUnhealthy, None) => {
                    format!("- {}: maybe unhealthy (no cached daemon info)", target.name)
                }
                (TargetHealthStatus::Unhealthy, Some(_)) => format!("- {}: unhealthy", target.name),
                (TargetHealthStatus::Unhealthy, None) => {
                    format!("- {}: unavailable (no cached daemon info)", target.name)
                }
                (TargetHealthStatus::Unknown, _) | (TargetHealthStatus::Healthy, None) => {
                    format!("- {}: unavailable (no cached daemon info)", target.name)
                }
            },
        )
        .collect::<Vec<_>>();

    format!("Configured targets:\n{}", lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use remote_exec_proto::public::ListTargetsInput;

    use super::list_targets;
    use crate::{BrokerState, session_store::SessionStore, state::BrokerStateInit};

    #[tokio::test]
    async fn list_targets_returns_empty_text_and_array_for_empty_state() {
        let state = BrokerState::new(BrokerStateInit {
            enable_transfer_compression: true,
            transfer_limits: remote_exec_proto::transfer::TransferLimits::default(),
            disable_structured_content: false,
            prepend_tool_names: true,
            health_refresh_intervals: crate::state::TargetHealthRefreshIntervals {
                healthy: std::time::Duration::from_secs(60),
                unhealthy: std::time::Duration::from_secs(15),
            },
            tools: crate::config::BrokerToolsConfig::default(),
            port_forward_limits: crate::port_forward::BrokerPortForwardLimits::default(),
            host_sandbox: None,
            host_filesystem: Default::default(),
            sessions: SessionStore::default(),
            port_forwards: crate::port_forward::PortForwardStore::default(),
            targets: Default::default(),
            reverse_transport: None,
        });

        let result = list_targets(&state, ListTargetsInput {}).await.unwrap();
        let call_result = result.into_call_tool_result(true);
        let text = call_result
            .content
            .iter()
            .filter_map(|content| content.as_text().map(|text| text.text.as_str()))
            .collect::<Vec<_>>()
            .join("\n");

        assert_eq!(text, "No configured targets.");
        assert_eq!(
            call_result.structured_content,
            Some(serde_json::json!({ "targets": [] }))
        );
    }
}
