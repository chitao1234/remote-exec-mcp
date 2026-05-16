mod codec;
mod meta;

pub use codec::{
    Frame, FrameType, HEADER_LEN, MAX_DATA_LEN, MAX_META_LEN, PREFACE, TUNNEL_PROTOCOL_VERSION,
    TUNNEL_PROTOCOL_VERSION_HEADER, UPGRADE_TOKEN, decode_frame_meta, encode_frame_meta,
    read_frame, read_preface, write_frame, write_preface,
};
pub use meta::{
    EndpointMeta, ForwardDropKind, ForwardDropMeta, ForwardRecoveredMeta, ForwardRecoveringMeta,
    TUNNEL_CLOSE_REASON_OPERATOR_CLOSE, TUNNEL_ERROR_CODE_LISTENER_OPEN_FAILED, TcpAcceptMeta,
    TunnelCloseMeta, TunnelErrorMeta, TunnelForwardProtocol, TunnelHeartbeatMeta,
    TunnelLimitSummary, TunnelOpenMeta, TunnelReadyMeta, TunnelRole, UdpDatagramMeta,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn v4_control_frames_round_trip() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        let writer = tokio::spawn(async move {
            write_frame(
                &mut client,
                &Frame {
                    frame_type: FrameType::TunnelOpen,
                    flags: 0,
                    stream_id: 0,
                    meta: serde_json::to_vec(&TunnelOpenMeta {
                        forward_id: "fwd_test".to_string(),
                        role: TunnelRole::Listen,
                        side: "builder-a".to_string(),
                        generation: 4,
                        protocol: TunnelForwardProtocol::Tcp,
                        resume_session_id: Some("sess_test".to_string()),
                    })
                    .unwrap(),
                    data: Vec::new(),
                },
            )
            .await
        });

        let frame = read_frame(&mut server).await.unwrap();
        writer.await.unwrap().unwrap();
        assert_eq!(frame.frame_type, FrameType::TunnelOpen);
        assert_eq!(frame.stream_id, 0);
        let meta: TunnelOpenMeta = serde_json::from_slice(&frame.meta).unwrap();
        assert_eq!(meta.forward_id, "fwd_test");
        assert_eq!(meta.role, TunnelRole::Listen);
        assert_eq!(meta.generation, 4);
        assert_eq!(meta.resume_session_id.as_deref(), Some("sess_test"));
    }

    #[tokio::test]
    async fn forward_drop_frame_round_trip() {
        let (mut client, mut server) = tokio::io::duplex(1024);
        let writer = tokio::spawn(async move {
            write_frame(
                &mut client,
                &Frame {
                    frame_type: FrameType::ForwardDrop,
                    flags: 0,
                    stream_id: 1,
                    meta: serde_json::to_vec(&ForwardDropMeta::new(
                        ForwardDropKind::TcpStream,
                        2,
                        "port_tunnel_limit_exceeded",
                        Some("port tunnel active tcp stream limit reached".to_string()),
                    ))
                    .unwrap(),
                    data: Vec::new(),
                },
            )
            .await
        });

        let frame = read_frame(&mut server).await.unwrap();
        writer.await.unwrap().unwrap();
        assert_eq!(frame.frame_type, FrameType::ForwardDrop);
        assert_eq!(frame.stream_id, 1);
        let meta: ForwardDropMeta = serde_json::from_slice(&frame.meta).unwrap();
        assert_eq!(meta.kind, ForwardDropKind::TcpStream);
        assert_eq!(meta.count, 2);
        assert_eq!(meta.reason(), "port_tunnel_limit_exceeded");
    }

    #[test]
    fn tunnel_protocol_version_is_aligned_to_v4() {
        assert_eq!(TUNNEL_PROTOCOL_VERSION, "4");
        assert_eq!(
            TUNNEL_PROTOCOL_VERSION_HEADER,
            "x-remote-exec-port-tunnel-version"
        );
    }

    #[tokio::test]
    async fn session_control_frames_round_trip() {
        let (mut client, mut server) = tokio::io::duplex(1024);
        let writer = tokio::spawn(async move {
            write_frame(
                &mut client,
                &Frame {
                    frame_type: FrameType::SessionResume,
                    flags: 0,
                    stream_id: 0,
                    meta: br#"{"session_id":"sess_123"}"#.to_vec(),
                    data: Vec::new(),
                },
            )
            .await
        });

        let frame = read_frame(&mut server).await.unwrap();
        writer.await.unwrap().unwrap();
        assert_eq!(frame.frame_type, FrameType::SessionResume);
        assert_eq!(frame.stream_id, 0);
        assert_eq!(frame.meta, br#"{"session_id":"sess_123"}"#);
    }

    #[tokio::test]
    async fn frame_round_trip_preserves_binary_payload() {
        let (mut client, mut server) = tokio::io::duplex(1024);
        let payload = vec![0, 1, 2, 255, b'R', b'\n'];
        let writer = tokio::spawn(async move {
            write_frame(
                &mut client,
                &Frame {
                    frame_type: FrameType::TcpData,
                    flags: 7,
                    stream_id: 3,
                    meta: br#"{"note":"binary"}"#.to_vec(),
                    data: payload,
                },
            )
            .await
        });

        let frame = read_frame(&mut server).await.unwrap();
        writer.await.unwrap().unwrap();
        assert_eq!(frame.frame_type, FrameType::TcpData);
        assert_eq!(frame.flags, 7);
        assert_eq!(frame.stream_id, 3);
        assert_eq!(frame.meta, br#"{"note":"binary"}"#);
        assert_eq!(frame.data, vec![0, 1, 2, 255, b'R', b'\n']);
    }

    #[tokio::test]
    async fn oversized_meta_is_rejected() {
        let mut bytes = Vec::from([FrameType::Error as u8, 0, 0, 0]);
        bytes.extend_from_slice(&1u32.to_be_bytes());
        bytes.extend_from_slice(&((MAX_META_LEN as u32) + 1).to_be_bytes());
        bytes.extend_from_slice(&0u32.to_be_bytes());

        let err = read_frame(&mut bytes.as_slice()).await.unwrap_err();

        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn oversized_data_is_rejected() {
        let mut bytes = Vec::from([FrameType::TcpData as u8, 0, 0, 0]);
        bytes.extend_from_slice(&1u32.to_be_bytes());
        bytes.extend_from_slice(&0u32.to_be_bytes());
        bytes.extend_from_slice(&((MAX_DATA_LEN as u32) + 1).to_be_bytes());

        let err = read_frame(&mut bytes.as_slice()).await.unwrap_err();

        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn target_info_defaults_to_missing_protocol_version() {
        let info: crate::rpc::TargetInfoResponse = serde_json::from_value(serde_json::json!({
            "target": "old",
            "daemon_version": "0",
            "daemon_instance_id": "i",
            "hostname": "h",
            "platform": "linux",
            "arch": "x86_64",
            "supports_pty": false,
            "supports_image_read": false,
            "supports_port_forward": true
        }))
        .unwrap();

        assert_eq!(info.capabilities.port_forward_protocol_version, None);
    }

    #[test]
    fn target_info_parses_protocol_version() {
        let info: crate::rpc::TargetInfoResponse = serde_json::from_value(serde_json::json!({
            "target": "daemon",
            "daemon_version": "0.1.0",
            "daemon_instance_id": "inst",
            "hostname": "host",
            "platform": "linux",
            "arch": "x86_64",
            "supports_pty": true,
            "supports_image_read": true,
            "supports_port_forward": true,
            "port_forward_protocol_version": 4
        }))
        .unwrap();

        assert_eq!(
            info.capabilities
                .port_forward_protocol_version
                .map(crate::rpc::PortForwardProtocolVersion::get),
            Some(4)
        );
    }

    #[test]
    fn frame_helpers_report_stream_scope_and_wire_length() {
        let control = Frame {
            frame_type: FrameType::TunnelHeartbeat,
            flags: 0,
            stream_id: 0,
            meta: vec![1, 2],
            data: Vec::new(),
        };
        assert!(!control.is_stream_frame());
        assert_eq!(control.wire_len(), HEADER_LEN + 2);

        let stream = Frame {
            frame_type: FrameType::TcpData,
            flags: 0,
            stream_id: 7,
            meta: vec![1, 2, 3],
            data: vec![4, 5],
        };
        assert!(stream.is_stream_frame());
        assert!(stream.is_data_plane_frame());
        assert_eq!(stream.data_plane_charge(), HEADER_LEN + 5);
        assert_eq!(stream.wire_len(), HEADER_LEN + 5);

        let stream_control = Frame {
            frame_type: FrameType::TcpEof,
            flags: 0,
            stream_id: 3,
            meta: vec![9],
            data: Vec::new(),
        };
        assert!(stream_control.is_stream_frame());
        assert!(!stream_control.is_data_plane_frame());
        assert_eq!(stream_control.data_plane_charge(), 0);
    }
}
