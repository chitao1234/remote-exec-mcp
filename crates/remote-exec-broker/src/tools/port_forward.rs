use remote_exec_proto::port_forward::ForwardId;
use remote_exec_proto::public::{
    ForwardPortEntry, ForwardPortPhase, ForwardPortStatus, ForwardPortsAction, ForwardPortsInput,
    ForwardPortsResult,
};

use crate::mcp_server::ToolCallOutput;
use crate::port_forward::{PortForwardFilter, open_forward};

pub async fn forward_ports(
    state: &crate::BrokerState,
    input: ForwardPortsInput,
) -> anyhow::Result<ToolCallOutput> {
    crate::request_context::set_current_targets(input_targets(&input));
    match input {
        ForwardPortsInput::Open {
            listen_side,
            connect_side,
            forwards,
        } => open_forwards(state, listen_side, connect_side, forwards).await,
        ForwardPortsInput::List {
            listen_side,
            connect_side,
            forward_ids,
        } => list_forwards(state, listen_side, connect_side, forward_ids).await,
        ForwardPortsInput::Close { forward_ids } => close_forwards(state, forward_ids).await,
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
        "port forwards open requested"
    );

    let listen_side = state.forwarding_side(&listen_side_name).await?;
    let connect_side = state.forwarding_side(&connect_side_name).await?;
    let mut reservations = state
        .port_forwards
        .reserve_open_batch(
            &listen_side_name,
            &connect_side_name,
            forwards.len(),
            state.port_forward_limits,
        )
        .await?;
    let mut opened = Vec::with_capacity(forwards.len());

    for spec in &forwards {
        let reservation = reservations
            .pop()
            .expect("reservation count should match requested forwards");
        match open_forward(
            state.port_forwards.clone(),
            reservation.clone(),
            state.port_forward_limits.public_summary(),
            listen_side.clone(),
            connect_side.clone(),
            spec,
        )
        .await
        {
            Ok(forward) => opened.push(forward),
            Err(err) => {
                state
                    .port_forwards
                    .release_open_reservation(reservation)
                    .await;
                state
                    .port_forwards
                    .release_open_reservations(reservations)
                    .await;
                for forward in opened {
                    let _ = forward
                        .close_unregistered(state.port_forwards.clone())
                        .await;
                }
                return Err(err);
            }
        }
    }

    let mut result_entries = Vec::with_capacity(opened.len());
    let mut registered_ids = Vec::with_capacity(opened.len());
    let mut opened = opened.into_iter();
    while let Some(forward) = opened.next() {
        result_entries.push(forward.entry().clone());
        let forward_id = forward.entry().forward_id.clone();
        if let Err(err) = forward
            .register_and_start(state.port_forwards.clone())
            .await
        {
            for remaining in opened {
                let _ = remaining
                    .close_unregistered(state.port_forwards.clone())
                    .await;
            }
            if !registered_ids.is_empty() {
                let _ = state.port_forwards.close(&registered_ids).await;
            }
            return Err(err);
        }
        registered_ids.push(forward_id);
    }

    tracing::info!(
        tool = "forward_ports",
        action = "open",
        opened_forwards = result_entries.len(),
        "port forwards opened"
    );

    finish_forward_ports(ForwardPortsAction::Open, result_entries)
}

async fn list_forwards(
    state: &crate::BrokerState,
    listen_side: Option<String>,
    connect_side: Option<String>,
    forward_ids: Vec<ForwardId>,
) -> anyhow::Result<ToolCallOutput> {
    tracing::info!(
        tool = "forward_ports",
        action = "list",
        "port forwards list requested"
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
        "port forwards listed"
    );
    finish_forward_ports(ForwardPortsAction::List, entries)
}

async fn close_forwards(
    state: &crate::BrokerState,
    forward_ids: Vec<ForwardId>,
) -> anyhow::Result<ToolCallOutput> {
    anyhow::ensure!(
        !forward_ids.is_empty(),
        "`forward_ids` must contain at least one entry"
    );
    tracing::info!(
        tool = "forward_ports",
        action = "close",
        forward_count = forward_ids.len(),
        "port forwards close requested"
    );
    let entries = state.port_forwards.close(&forward_ids).await?;
    tracing::info!(
        tool = "forward_ports",
        action = "close",
        closed_forwards = entries.len(),
        "port forwards closed"
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

    if result.action == ForwardPortsAction::Close {
        return format_entry_section("Closed port forwards", &result.forwards);
    }

    let (ready, not_ready): (Vec<_>, Vec<_>) =
        result.forwards.iter().partition(|entry| is_ready(entry));
    let mut sections = Vec::new();
    if !ready.is_empty() {
        sections.push(format_entry_section("Ready port forwards", &ready));
    }
    if !not_ready.is_empty() {
        sections.push(format_entry_section("Not ready", &not_ready));
    }
    sections.join("\n")
}

fn format_entry_section<T>(heading: &str, entries: &[T]) -> String
where
    T: std::borrow::Borrow<ForwardPortEntry>,
{
    let lines = entries
        .iter()
        .map(|entry| {
            let entry = entry.borrow();
            format!(
                "- {}: {} on `{}` -> {} on `{}` ({}){}",
                entry.forward_id,
                entry.listen_endpoint,
                entry.listen_side,
                entry.connect_endpoint,
                entry.connect_side,
                format_protocol(entry.protocol),
                failed_error_suffix(entry)
            )
        })
        .collect::<Vec<_>>();
    format!("{heading}:\n{}", lines.join("\n"))
}

fn is_ready(entry: &ForwardPortEntry) -> bool {
    entry.status == ForwardPortStatus::Open && entry.phase == ForwardPortPhase::Ready
}

fn failed_error_suffix(entry: &ForwardPortEntry) -> String {
    if entry.status != ForwardPortStatus::Failed {
        return String::new();
    }
    entry
        .last_error
        .as_ref()
        .map(|err| format!(", error={err}"))
        .unwrap_or_default()
}

fn format_protocol(protocol: remote_exec_proto::public::ForwardPortProtocol) -> &'static str {
    match protocol {
        remote_exec_proto::public::ForwardPortProtocol::Tcp => "tcp",
        remote_exec_proto::public::ForwardPortProtocol::Udp => "udp",
    }
}
