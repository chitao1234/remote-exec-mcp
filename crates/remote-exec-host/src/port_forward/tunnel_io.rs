use std::io::{self, ErrorKind};

use remote_exec_proto::port_tunnel::{
    Frame, HEADER_LEN, PREFACE, decode_frame_header, encode_frame_header, frame_from_parts,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

#[cfg(test)]
pub(super) async fn write_preface<W>(writer: &mut W) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    writer.write_all(PREFACE).await
}

pub(super) async fn read_preface<R>(reader: &mut R) -> io::Result<()>
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

pub(super) async fn write_frame<W>(writer: &mut W, frame: &Frame) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let header = encode_frame_header(frame)?;
    writer.write_all(&header).await?;
    writer.write_all(&frame.meta).await?;
    writer.write_all(&frame.data).await
}

pub(super) async fn read_frame<R>(reader: &mut R) -> io::Result<Frame>
where
    R: AsyncRead + Unpin,
{
    let mut raw_header = [0; HEADER_LEN];
    reader.read_exact(&mut raw_header).await?;
    let header = decode_frame_header(raw_header)?;

    let mut meta = vec![0; header.meta_len];
    reader.read_exact(&mut meta).await?;
    let mut data = vec![0; header.data_len];
    reader.read_exact(&mut data).await?;

    frame_from_parts(header, meta, data)
}
