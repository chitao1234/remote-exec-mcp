use std::io::{self, ErrorKind};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const PREFACE: &[u8; 8] = b"REPFWD1\n";
pub const HEADER_LEN: usize = 16;
pub const MAX_META_LEN: usize = 16 * 1024;
pub const MAX_DATA_LEN: usize = 256 * 1024;
pub const TUNNEL_PROTOCOL_VERSION_HEADER: &str = "x-remote-exec-port-tunnel-version";
pub const TUNNEL_PROTOCOL_VERSION: &str = "2";
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
    TcpListen = 10,
    TcpListenOk = 11,
    TcpAccept = 12,
    TcpConnect = 13,
    TcpConnectOk = 14,
    TcpData = 15,
    TcpEof = 16,
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
            10 => Ok(Self::TcpListen),
            11 => Ok(Self::TcpListenOk),
            12 => Ok(Self::TcpAccept),
            13 => Ok(Self::TcpConnect),
            14 => Ok(Self::TcpConnectOk),
            15 => Ok(Self::TcpData),
            16 => Ok(Self::TcpEof),
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub frame_type: FrameType,
    pub flags: u8,
    pub stream_id: u32,
    pub meta: Vec<u8>,
    pub data: Vec<u8>,
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
    fn target_info_defaults_to_protocol_zero() {
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

        assert_eq!(info.port_forward_protocol_version, 0);
    }
}
