use std::io::{self, ErrorKind};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const PREFACE: &[u8; 8] = b"REPFWD1\n";
pub const HEADER_LEN: usize = 16;
pub const MAX_META_LEN: usize = 16 * 1024;
pub const MAX_DATA_LEN: usize = 256 * 1024;
pub const TUNNEL_PROTOCOL_VERSION_HEADER: &str = "x-remote-exec-port-tunnel-version";
pub const TUNNEL_PROTOCOL_VERSION: &str = "4";
pub const UPGRADE_TOKEN: &str = "remote-exec-port-tunnel";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameType {
    Error = 1,
    Close = 2,
    SessionOpen = 3,
    SessionReady = 4,
    SessionResume = 5,
    SessionResumed = 6,
    TunnelOpen = 7,
    TunnelReady = 8,
    TunnelClose = 9,
    TcpListen = 10,
    TcpListenOk = 11,
    TcpAccept = 12,
    TcpConnect = 13,
    TcpConnectOk = 14,
    TcpData = 15,
    TcpEof = 16,
    TunnelClosed = 17,
    TunnelHeartbeat = 18,
    TunnelHeartbeatAck = 19,
    // Reserved in v4. Public state changes are currently reported through
    // broker-owned forward state, not daemon-emitted recovery frames.
    ForwardRecovering = 20,
    ForwardRecovered = 21,
    ForwardDrop = 22,
    UdpBind = 30,
    UdpBindOk = 31,
    UdpDatagram = 32,
}

impl FrameType {
    fn from_u8(value: u8) -> io::Result<Self> {
        match value {
            1 => Ok(Self::Error),
            2 => Ok(Self::Close),
            3 => Ok(Self::SessionOpen),
            4 => Ok(Self::SessionReady),
            5 => Ok(Self::SessionResume),
            6 => Ok(Self::SessionResumed),
            7 => Ok(Self::TunnelOpen),
            8 => Ok(Self::TunnelReady),
            9 => Ok(Self::TunnelClose),
            10 => Ok(Self::TcpListen),
            11 => Ok(Self::TcpListenOk),
            12 => Ok(Self::TcpAccept),
            13 => Ok(Self::TcpConnect),
            14 => Ok(Self::TcpConnectOk),
            15 => Ok(Self::TcpData),
            16 => Ok(Self::TcpEof),
            17 => Ok(Self::TunnelClosed),
            18 => Ok(Self::TunnelHeartbeat),
            19 => Ok(Self::TunnelHeartbeatAck),
            20 => Ok(Self::ForwardRecovering),
            21 => Ok(Self::ForwardRecovered),
            22 => Ok(Self::ForwardDrop),
            30 => Ok(Self::UdpBind),
            31 => Ok(Self::UdpBindOk),
            32 => Ok(Self::UdpDatagram),
            _ => Err(io::Error::new(
                ErrorKind::InvalidData,
                format!("unknown port tunnel frame type `{value}`"),
            )),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TunnelRole {
    Listen,
    Connect,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TunnelForwardProtocol {
    Tcp,
    Udp,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct TunnelOpenMeta {
    pub forward_id: String,
    pub role: TunnelRole,
    pub side: String,
    pub generation: u64,
    pub protocol: TunnelForwardProtocol,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_session_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct TunnelReadyMeta {
    pub generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_timeout_ms: Option<u64>,
    pub limits: TunnelLimitSummary,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct TunnelLimitSummary {
    pub max_active_tcp_streams: u64,
    pub max_udp_peers: u64,
    pub max_queued_bytes: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct TunnelCloseMeta {
    pub forward_id: String,
    pub generation: u64,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub struct TunnelHeartbeatMeta {
    pub nonce: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ForwardRecoveringMeta {
    pub forward_id: String,
    pub role: TunnelRole,
    pub old_generation: u64,
    pub reason: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ForwardRecoveredMeta {
    pub forward_id: String,
    pub role: TunnelRole,
    pub generation: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ForwardDropKind {
    TcpStream,
    UdpDatagram,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ForwardDropMeta {
    pub kind: ForwardDropKind,
    pub count: u64,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct TunnelErrorMeta {
    pub code: String,
    pub message: String,
    pub fatal: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct EndpointMeta {
    pub endpoint: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct TcpAcceptMeta {
    pub listener_stream_id: u32,
    #[serde(default)]
    pub peer: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct UdpDatagramMeta {
    pub peer: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub frame_type: FrameType,
    pub flags: u8,
    pub stream_id: u32,
    pub meta: Vec<u8>,
    pub data: Vec<u8>,
}

impl Frame {
    pub fn is_stream_frame(&self) -> bool {
        self.stream_id != 0
    }

    pub fn is_data_plane_frame(&self) -> bool {
        self.is_stream_frame() && !self.data.is_empty()
    }

    pub fn data_plane_charge(&self) -> usize {
        if self.is_data_plane_frame() {
            self.wire_len()
        } else {
            0
        }
    }

    pub fn wire_len(&self) -> usize {
        HEADER_LEN
            .saturating_add(self.meta.len())
            .saturating_add(self.data.len())
    }
}

pub fn encode_frame_meta<T: Serialize>(meta: &T) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(meta)
}

pub fn decode_frame_meta<T: DeserializeOwned>(frame: &Frame) -> Result<T, serde_json::Error> {
    serde_json::from_slice(&frame.meta)
}

pub async fn write_preface<W>(writer: &mut W) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    writer.write_all(PREFACE).await
}

pub async fn read_preface<R>(reader: &mut R) -> io::Result<()>
where
    R: AsyncRead + Unpin,
{
    let mut preface = [0; PREFACE.len()];
    reader.read_exact(&mut preface).await?;
    if &preface != PREFACE {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "invalid port tunnel preface",
        ));
    }
    Ok(())
}

pub async fn write_frame<W>(writer: &mut W, frame: &Frame) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    if frame.meta.len() > MAX_META_LEN {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "port tunnel metadata exceeds maximum length",
        ));
    }
    if frame.data.len() > MAX_DATA_LEN {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "port tunnel data exceeds maximum length",
        ));
    }

    let mut header = [0; HEADER_LEN];
    header[0] = frame.frame_type as u8;
    header[1] = frame.flags;
    header[4..8].copy_from_slice(&frame.stream_id.to_be_bytes());
    header[8..12].copy_from_slice(&(frame.meta.len() as u32).to_be_bytes());
    header[12..16].copy_from_slice(&(frame.data.len() as u32).to_be_bytes());

    writer.write_all(&header).await?;
    writer.write_all(&frame.meta).await?;
    writer.write_all(&frame.data).await
}

pub async fn read_frame<R>(reader: &mut R) -> io::Result<Frame>
where
    R: AsyncRead + Unpin,
{
    let mut header = [0; HEADER_LEN];
    reader.read_exact(&mut header).await?;

    if header[2] != 0 || header[3] != 0 {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "port tunnel reserved header bytes must be zero",
        ));
    }

    let frame_type = FrameType::from_u8(header[0])?;
    let flags = header[1];
    let stream_id = u32::from_be_bytes(header[4..8].try_into().expect("header slice length"));
    let meta_len = u32::from_be_bytes(header[8..12].try_into().expect("header slice length"));
    let data_len = u32::from_be_bytes(header[12..16].try_into().expect("header slice length"));

    if meta_len as usize > MAX_META_LEN {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "port tunnel metadata exceeds maximum length",
        ));
    }
    if data_len as usize > MAX_DATA_LEN {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "port tunnel data exceeds maximum length",
        ));
    }

    let mut meta = vec![0; meta_len as usize];
    reader.read_exact(&mut meta).await?;
    let mut data = vec![0; data_len as usize];
    reader.read_exact(&mut data).await?;

    Ok(Frame {
        frame_type,
        flags,
        stream_id,
        meta,
        data,
    })
}

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
                    meta: serde_json::to_vec(&ForwardDropMeta {
                        kind: ForwardDropKind::TcpStream,
                        count: 2,
                        reason: "port_tunnel_limit_exceeded".to_string(),
                        message: Some("port tunnel active tcp stream limit reached".to_string()),
                    })
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
        assert_eq!(meta.reason, "port_tunnel_limit_exceeded");
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

        assert_eq!(info.port_forward_protocol_version, None);
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
            info.port_forward_protocol_version
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
