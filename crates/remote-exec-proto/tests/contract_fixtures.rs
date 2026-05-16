use std::{collections::BTreeMap, sync::OnceLock};

use remote_exec_proto::{
    port_tunnel::{
        FrameType, HEADER_LEN, MAX_DATA_LEN, MAX_META_LEN, PREFACE, TUNNEL_PROTOCOL_VERSION,
        TUNNEL_PROTOCOL_VERSION_HEADER, UPGRADE_TOKEN,
    },
    rpc::{
        PortForwardProtocolVersion, TRANSFER_COMPRESSION_HEADER, TRANSFER_CREATE_PARENT_HEADER,
        TRANSFER_DESTINATION_PATH_HEADER, TRANSFER_OVERWRITE_HEADER, TRANSFER_SOURCE_TYPE_HEADER,
        TRANSFER_SYMLINK_MODE_HEADER, transfer_export_header_pairs, transfer_import_header_pairs,
    },
    transfer::{
        TransferCompression, TransferExportMetadata, TransferImportMetadata, TransferOverwrite,
        TransferSourceType, TransferSymlinkMode,
    },
};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct PortTunnelContract {
    preface_ascii: String,
    header_length: usize,
    max_meta_length: usize,
    max_data_length: usize,
    protocol_version_header: String,
    protocol_version_value: String,
    protocol_version_number: u32,
    upgrade_token: String,
    frame_types: BTreeMap<String, u8>,
}

#[derive(Debug, Deserialize)]
struct TransferHeaderContract {
    headers: BTreeMap<String, String>,
}

fn port_tunnel_contract() -> &'static PortTunnelContract {
    static CONTRACT: OnceLock<PortTunnelContract> = OnceLock::new();
    CONTRACT.get_or_init(|| {
        serde_json::from_str(include_str!(
            "../../../tests/contracts/port_tunnel/contract.json"
        ))
        .expect("valid port tunnel contract fixture")
    })
}

fn transfer_header_contract() -> &'static TransferHeaderContract {
    static CONTRACT: OnceLock<TransferHeaderContract> = OnceLock::new();
    CONTRACT.get_or_init(|| {
        serde_json::from_str(include_str!(
            "../../../tests/contracts/transfer_headers/contract.json"
        ))
        .expect("valid transfer header contract fixture")
    })
}

#[test]
fn port_tunnel_constants_match_shared_contract_fixture() {
    let contract = port_tunnel_contract();

    assert_eq!(PREFACE, contract.preface_ascii.as_bytes());
    assert_eq!(HEADER_LEN, contract.header_length);
    assert_eq!(MAX_META_LEN, contract.max_meta_length);
    assert_eq!(MAX_DATA_LEN, contract.max_data_length);
    assert_eq!(
        TUNNEL_PROTOCOL_VERSION_HEADER,
        contract.protocol_version_header
    );
    assert_eq!(TUNNEL_PROTOCOL_VERSION, contract.protocol_version_value);
    assert_eq!(
        PortForwardProtocolVersion::v4().get(),
        contract.protocol_version_number
    );
    assert_eq!(UPGRADE_TOKEN, contract.upgrade_token);
}

#[test]
fn port_tunnel_frame_type_mapping_matches_shared_contract_fixture() {
    let expected = BTreeMap::from([
        ("Close".to_string(), FrameType::Close as u8),
        ("Error".to_string(), FrameType::Error as u8),
        ("ForwardDrop".to_string(), FrameType::ForwardDrop as u8),
        (
            "ForwardRecovered".to_string(),
            FrameType::ForwardRecovered as u8,
        ),
        (
            "ForwardRecovering".to_string(),
            FrameType::ForwardRecovering as u8,
        ),
        ("SessionOpen".to_string(), FrameType::SessionOpen as u8),
        ("SessionReady".to_string(), FrameType::SessionReady as u8),
        ("SessionResume".to_string(), FrameType::SessionResume as u8),
        (
            "SessionResumed".to_string(),
            FrameType::SessionResumed as u8,
        ),
        ("TcpAccept".to_string(), FrameType::TcpAccept as u8),
        ("TcpConnect".to_string(), FrameType::TcpConnect as u8),
        ("TcpConnectOk".to_string(), FrameType::TcpConnectOk as u8),
        ("TcpData".to_string(), FrameType::TcpData as u8),
        ("TcpEof".to_string(), FrameType::TcpEof as u8),
        ("TcpListen".to_string(), FrameType::TcpListen as u8),
        ("TcpListenOk".to_string(), FrameType::TcpListenOk as u8),
        ("TunnelClose".to_string(), FrameType::TunnelClose as u8),
        ("TunnelClosed".to_string(), FrameType::TunnelClosed as u8),
        (
            "TunnelHeartbeat".to_string(),
            FrameType::TunnelHeartbeat as u8,
        ),
        (
            "TunnelHeartbeatAck".to_string(),
            FrameType::TunnelHeartbeatAck as u8,
        ),
        ("TunnelOpen".to_string(), FrameType::TunnelOpen as u8),
        ("TunnelReady".to_string(), FrameType::TunnelReady as u8),
        ("UdpBind".to_string(), FrameType::UdpBind as u8),
        ("UdpBindOk".to_string(), FrameType::UdpBindOk as u8),
        ("UdpDatagram".to_string(), FrameType::UdpDatagram as u8),
    ]);

    assert_eq!(port_tunnel_contract().frame_types, expected);
}

#[test]
fn transfer_header_constants_match_shared_contract_fixture() {
    let expected = BTreeMap::from([
        (
            "compression".to_string(),
            TRANSFER_COMPRESSION_HEADER.to_string(),
        ),
        (
            "create_parent".to_string(),
            TRANSFER_CREATE_PARENT_HEADER.to_string(),
        ),
        (
            "destination_path".to_string(),
            TRANSFER_DESTINATION_PATH_HEADER.to_string(),
        ),
        (
            "overwrite".to_string(),
            TRANSFER_OVERWRITE_HEADER.to_string(),
        ),
        (
            "source_type".to_string(),
            TRANSFER_SOURCE_TYPE_HEADER.to_string(),
        ),
        (
            "symlink_mode".to_string(),
            TRANSFER_SYMLINK_MODE_HEADER.to_string(),
        ),
    ]);

    assert_eq!(transfer_header_contract().headers, expected);
}

#[test]
fn transfer_header_renderers_use_shared_contract_fixture_names() {
    let headers = &transfer_header_contract().headers;

    let import_pairs: Vec<_> = transfer_import_header_pairs(&TransferImportMetadata {
        destination_path: "/tmp/output".to_string(),
        overwrite: TransferOverwrite::Replace,
        create_parent: true,
        source_type: TransferSourceType::Directory,
        compression: TransferCompression::Zstd,
        symlink_mode: TransferSymlinkMode::Follow,
    })
    .into_iter()
    .map(|(name, _)| name.to_string())
    .collect();
    assert_eq!(
        import_pairs,
        vec![
            headers["destination_path"].clone(),
            headers["overwrite"].clone(),
            headers["create_parent"].clone(),
            headers["source_type"].clone(),
            headers["compression"].clone(),
            headers["symlink_mode"].clone(),
        ]
    );

    let export_pairs: Vec<_> = transfer_export_header_pairs(&TransferExportMetadata {
        source_type: TransferSourceType::Multiple,
        compression: TransferCompression::None,
    })
    .into_iter()
    .map(|(name, _)| name.to_string())
    .collect();
    assert_eq!(
        export_pairs,
        vec![
            headers["source_type"].clone(),
            headers["compression"].clone(),
        ]
    );
}
