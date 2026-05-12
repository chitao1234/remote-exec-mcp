use std::io::{self, ErrorKind};

use serde::Serialize;
use serde::de::DeserializeOwned;
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
