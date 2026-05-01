use std::io::{Read, Write};
use std::path::Path;

use remote_exec_proto::rpc::TransferCompression;

pub(super) fn open_archive_writer(
    archive_path: &Path,
    compression: &TransferCompression,
) -> anyhow::Result<Box<dyn Write>> {
    let file = std::fs::File::create(archive_path)?;
    wrap_archive_writer(file, compression)
}

pub(super) fn wrap_archive_writer<W>(
    writer: W,
    compression: &TransferCompression,
) -> anyhow::Result<Box<dyn Write>>
where
    W: Write + 'static,
{
    match compression {
        TransferCompression::None => Ok(Box::new(writer)),
        TransferCompression::Zstd => {
            let encoder = zstd::stream::write::Encoder::new(writer, 0)?;
            Ok(Box::new(encoder.auto_finish()))
        }
    }
}

pub(super) fn open_archive_reader(
    archive_path: &Path,
    compression: &TransferCompression,
) -> anyhow::Result<Box<dyn Read>> {
    let file = std::fs::File::open(archive_path)?;
    wrap_archive_reader(file, compression)
}

pub(super) fn wrap_archive_reader<R>(
    reader: R,
    compression: &TransferCompression,
) -> anyhow::Result<Box<dyn Read>>
where
    R: Read + 'static,
{
    match compression {
        TransferCompression::None => Ok(Box::new(reader)),
        TransferCompression::Zstd => Ok(Box::new(zstd::stream::read::Decoder::new(reader)?)),
    }
}

pub(super) fn with_archive_builder<F>(
    archive_path: &Path,
    compression: &TransferCompression,
    build: F,
) -> anyhow::Result<()>
where
    F: FnOnce(&mut tar::Builder<Box<dyn Write>>) -> anyhow::Result<()>,
{
    let writer = open_archive_writer(archive_path, compression)?;
    let mut builder = tar::Builder::new(writer);
    build(&mut builder)?;
    finish_archive_builder(builder)
}

pub(super) fn with_archive_writer<W, F>(
    writer: W,
    compression: &TransferCompression,
    build: F,
) -> anyhow::Result<()>
where
    W: Write + 'static,
    F: FnOnce(&mut tar::Builder<Box<dyn Write>>) -> anyhow::Result<()>,
{
    let writer = wrap_archive_writer(writer, compression)?;
    let mut builder = tar::Builder::new(writer);
    build(&mut builder)?;
    finish_archive_builder(builder)
}

fn finish_archive_builder<W: Write>(mut builder: tar::Builder<W>) -> anyhow::Result<()> {
    builder.finish()?;
    drop(builder.into_inner()?);
    Ok(())
}
