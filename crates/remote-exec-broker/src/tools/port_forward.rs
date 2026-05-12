use remote_exec_proto::public::{
    ForwardPortEntry, ForwardPortPhase, ForwardPortStatus, ForwardPortsAction, ForwardPortsInput,
    ForwardPortsResult,
};

use crate::mcp_server::ToolCallOutput;
use crate::port_forward::{PortForwardFilter, close_record, open_forward};

pub async fn forward_ports(
    state: &crate::BrokerState,
    input: ForwardPortsInput,
) -> anyhow::Result<ToolCallOutput> {
    let started = std::time::Instant::now();
    crate::request_context::set_current_targets(input_targets(&input));
    match input {
        ForwardPortsInput::Open {
            listen_side,
            connect_side,
            forwards,
        } => open_forwards(state, started, listen_side, connect_side, forwards).await,
        ForwardPortsInput::List {
            listen_side,
            connect_side,
            forward_ids,
        } => list_forwards(state, started, listen_side, connect_side, forward_ids).await,
        ForwardPortsInput::Close { forward_ids } => {
            close_forwards(state, started, forward_ids).await
        }
    }
}

fn input_targets(input: &ForwardPortsInput) -> Vec<&str> {
    let mut targets = Vec::new();
    match input {
        ForwardPortsInput::Open {
            listen_side,
            connect_side,
            ..
        } => {
            targets.push(listen_side.as_str());
            targets.push(connect_side.as_str());
        }
        ForwardPortsInput::List {
            listen_side,
            connect_side,
            ..
        } => {
            if let Some(listen_side) = listen_side {
                targets.push(listen_side.as_str());
            }
            if let Some(connect_side) = connect_side {
                targets.push(connect_side.as_str());
            }
        }
        ForwardPortsInput::Close { .. } => {}
    }
    targets
}

async fn open_forwards(
    state: &crate::BrokerState,
    started: std::time::Instant,
    listen_side_name: String,
    connect_side_name: String,
    forwards: Vec<remote_exec_proto::public::ForwardPortSpec>,
) -> anyhow::Result<ToolCallOutput> {
    anyhow::ensure!(
        !forwards.is_empty(),
        "`forwards` must contain at least one entry"
    );
    anyhow::ensure!(
        !listen_side_name.is_empty(),
        "`listen_side` must not be empty"
    );
    anyhow::ensure!(
        !connect_side_name.is_empty(),
        "`connect_side` must not be empty"
    );

    tracing::info!(
        tool = "forward_ports",
        action = "open",
        listen_side = %listen_side_name,
        connect_side = %connect_side_name,
        forward_count = forwards.len(),
        "broker tool started"
    );

    let listen_side = state.forwarding_side(&listen_side_name).await?;
    let connect_side = state.forwarding_side(&connect_side_name).await?;
    enforce_open_limits(state, &listen_side_name, &connect_side_name, forwards.len()).await?;
    let mut opened = Vec::with_capacity(forwards.len());

    for spec in &forwards {
        match open_forward(
            state.port_forwards.clone(),
            state.port_forward_limits.public_summary(),
            listen_side.clone(),
            connect_side.clone(),
            spec,
        )
        .await
        {
            Ok(forward) => opened.push(forward),
            Err(err) => {
                for forward in opened {
                    let _ = close_record(forward.record).await;
                }
                return Err(err);
            }
        }
    }

    let mut result_entries = Vec::with_capacity(opened.len());
    for forward in opened {
        result_entries.push(forward.entry().clone());
        forward
            .register_and_start(state.port_forwards.clone())
            .await;
    }

    tracing::info!(
        tool = "forward_ports",
        action = "open",
        opened_forwards = result_entries.len(),
        elapsed_ms = started.elapsed().as_millis() as u64,
        "broker tool completed"
    );

    finish_forward_ports(ForwardPortsAction::Open, result_entries)
}

async fn enforce_open_limits(
    state: &crate::BrokerState,
    listen_side: &str,
    connect_side: &str,
    requested_forwards: usize,
) -> anyhow::Result<()> {
    let limits = state.port_forward_limits;
    anyhow::ensure!(
        state.port_forwards.open_count().await + requested_forwards
            <= limits.max_open_forwards_total,
        "port_forward_limit_exceeded: broker open forward limit reached"
    );
    anyhow::ensure!(
        state
            .port_forwards
            .side_pair_count(listen_side, connect_side)
            .await
            + requested_forwards
            <= limits.max_forwards_per_side_pair,
        "port_forward_limit_exceeded: broker side-pair forward limit reached"
    );
    Ok(())
}

async fn list_forwards(
    state: &crate::BrokerState,
    started: std::time::Instant,
    listen_side: Option<String>,
    connect_side: Option<String>,
    forward_ids: Vec<String>,
) -> anyhow::Result<ToolCallOutput> {
    tracing::info!(
        tool = "forward_ports",
        action = "list",
        "broker tool started"
    );
    let entries = state
        .port_forwards
        .list(&PortForwardFilter {
            listen_side,
            connect_side,
            forward_ids,
        })
        .await;
    tracing::info!(
        tool = "forward_ports",
        action = "list",
        forward_count = entries.len(),
        elapsed_ms = started.elapsed().as_millis() as u64,
        "broker tool completed"
    );
    finish_forward_ports(ForwardPortsAction::List, entries)
}

async fn close_forwards(
    state: &crate::BrokerState,
    started: std::time::Instant,
    forward_ids: Vec<String>,
) -> anyhow::Result<ToolCallOutput> {
    anyhow::ensure!(
        !forward_ids.is_empty(),
        "`forward_ids` must contain at least one entry"
    );
    tracing::info!(
        tool = "forward_ports",
        action = "close",
        forward_count = forward_ids.len(),
        "broker tool started"
    );
    let entries = state.port_forwards.close(&forward_ids).await?;
    tracing::info!(
        tool = "forward_ports",
        action = "close",
        closed_forwards = entries.len(),
        elapsed_ms = started.elapsed().as_millis() as u64,
        "broker tool completed"
    );
    finish_forward_ports(ForwardPortsAction::Close, entries)
}

fn finish_forward_ports(
    action: ForwardPortsAction,
    forwards: Vec<ForwardPortEntry>,
) -> anyhow::Result<ToolCallOutput> {
    let result = ForwardPortsResult { action, forwards };
    Ok(ToolCallOutput::text_and_structured(
        format_forward_ports_text(&result),
        serde_json::to_value(result)?,
    ))
}

fn format_forward_ports_text(result: &ForwardPortsResult) -> String {
    if result.forwards.is_empty() {
        return "No port forwards.".to_string();
    }

    let verb = match &result.action {
        ForwardPortsAction::Open => "Opened",
        ForwardPortsAction::List => "Port forwards",
        ForwardPortsAction::Close => "Closed",
    };
    let lines = result
        .forwards
        .iter()
        .map(|entry| {
            let phase_suffix = match entry.phase {
                ForwardPortPhase::Ready | ForwardPortPhase::Closed | ForwardPortPhase::Failed => {
                    String::new()
                }
                phase => format!(", phase={}", format_phase(phase)),
            };
            format!(
                "- {}: {} on `{}` -> {} on `{}` ({}, {}){}{}",
                entry.forward_id,
                entry.listen_endpoint,
                entry.listen_side,
                entry.connect_endpoint,
                entry.connect_side,
                format_protocol(entry.protocol),
                format_status(&entry.status),
                phase_suffix,
                entry
                    .last_error
                    .as_ref()
                    .map(|err| format!(", error={err}"))
                    .unwrap_or_default()
            )
        })
        .collect::<Vec<_>>();
    format!("{verb}:\n{}", lines.join("\n"))
}

fn format_protocol(protocol: remote_exec_proto::public::ForwardPortProtocol) -> &'static str {
    match protocol {
        remote_exec_proto::public::ForwardPortProtocol::Tcp => "tcp",
        remote_exec_proto::public::ForwardPortProtocol::Udp => "udp",
    }
}

fn format_phase(phase: ForwardPortPhase) -> &'static str {
    match phase {
        ForwardPortPhase::Opening => "opening",
        ForwardPortPhase::Ready => "ready",
        ForwardPortPhase::Reconnecting => "reconnecting",
        ForwardPortPhase::Draining => "draining",
        ForwardPortPhase::Closing => "closing",
        ForwardPortPhase::Closed => "closed",
        ForwardPortPhase::Failed => "failed",
    }
}

fn format_status(status: &ForwardPortStatus) -> &'static str {
    match status {
        ForwardPortStatus::Open => "open",
        ForwardPortStatus::Closed => "closed",
        ForwardPortStatus::Failed => "failed",
    }
}
