mod assets;
mod exec;
mod forward_ports;
mod transfer;

pub use assets::{ApplyPatchInput, ViewImageInput, ViewImageResult};
pub use exec::{
    CommandToolResult, ExecCommandInput, ListTargetDaemonInfo, ListTargetEntry, ListTargetsInput,
    ListTargetsResult, WriteStdinInput,
};
pub use forward_ports::{
    ForwardPortEntry, ForwardPortLimitSummary, ForwardPortPhase, ForwardPortProtocol,
    ForwardPortSideHealth, ForwardPortSideRole, ForwardPortSideState, ForwardPortSpec,
    ForwardPortStatus, ForwardPortsAction, ForwardPortsInput, ForwardPortsResult, Timestamp,
};
pub use transfer::{
    TransferDestinationMode, TransferEndpoint, TransferFilesInput, TransferFilesResult,
};

pub use crate::transfer::{TransferOverwrite, TransferSourceType, TransferSymlinkMode};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::port_tunnel::TunnelForwardProtocol;

    #[test]
    fn forward_ports_input_reports_matching_action() {
        assert_eq!(
            ForwardPortsInput::Open {
                listen_side: "local".to_string(),
                connect_side: "builder-a".to_string(),
                forwards: Vec::new(),
            }
            .action(),
            ForwardPortsAction::Open
        );
        assert_eq!(
            ForwardPortsInput::List {
                listen_side: None,
                connect_side: None,
                forward_ids: Vec::new(),
            }
            .action(),
            ForwardPortsAction::List
        );
        assert_eq!(
            ForwardPortsInput::Close {
                forward_ids: vec!["fwd_123".to_string()],
            }
            .action(),
            ForwardPortsAction::Close
        );
    }

    #[test]
    fn forward_port_protocol_round_trips_through_tunnel_protocol() {
        assert_eq!(
            ForwardPortProtocol::from(TunnelForwardProtocol::from(ForwardPortProtocol::Tcp)),
            ForwardPortProtocol::Tcp
        );
        assert_eq!(
            ForwardPortProtocol::from(TunnelForwardProtocol::from(ForwardPortProtocol::Udp)),
            ForwardPortProtocol::Udp
        );
    }

    #[test]
    fn forward_port_entry_serializes_additive_v4_state() {
        let entry = ForwardPortEntry {
            forward_id: "fwd_test".to_string(),
            listen_side: "local".to_string(),
            listen_endpoint: "127.0.0.1:10000".to_string(),
            connect_side: "builder-a".to_string(),
            connect_endpoint: "127.0.0.1:10001".to_string(),
            protocol: ForwardPortProtocol::Tcp,
            status: ForwardPortStatus::Open,
            last_error: None,
            phase: ForwardPortPhase::Reconnecting,
            listen_state: ForwardPortSideState {
                side: "local".to_string(),
                role: ForwardPortSideRole::Listen,
                generation: 2,
                health: ForwardPortSideHealth::Ready,
                last_error: None,
            },
            connect_state: ForwardPortSideState {
                side: "builder-a".to_string(),
                role: ForwardPortSideRole::Connect,
                generation: 3,
                health: ForwardPortSideHealth::Reconnecting,
                last_error: Some("transport loss".to_string()),
            },
            active_tcp_streams: 1,
            dropped_tcp_streams: 2,
            dropped_udp_datagrams: 3,
            reconnect_attempts: 4,
            last_reconnect_at: Some(Timestamp("2026-05-08T00:00:00Z".to_string())),
            limits: ForwardPortLimitSummary {
                max_active_tcp_streams: 256,
                max_udp_peers: 256,
                max_pending_tcp_bytes_per_stream: 262144,
                max_pending_tcp_bytes_per_forward: 2097152,
                max_tunnel_queued_bytes: 8388608,
                max_reconnecting_forwards: 16,
            },
        };

        let value = serde_json::to_value(entry).unwrap();
        assert_eq!(value["phase"], "reconnecting");
        assert_eq!(value["connect_state"]["health"], "reconnecting");
        assert_eq!(value["dropped_tcp_streams"], 2);
        assert_eq!(value["limits"]["max_tunnel_queued_bytes"], 8388608);
        assert_eq!(value["limits"]["max_reconnecting_forwards"], 16);
    }
}
