use std::io::{Read, Write};
use std::path::Path;

use remote_exec_proto::rpc::TransferCompression;

pub(super) fn open_archive_writer(
    archive_path: &Path,
    compression: &TransferCompression,
) -> anyhow::Result<Box<dyn Write>> {
    let file = std::fs::File::create(archive_path)?;
    match compression {
        TransferCompression::None => Ok(Box::new(file)),
        TransferCompression::Zstd => {
            let encoder = zstd::stream::write::Encoder::new(file, 0)?;
            Ok(Box::new(encoder.auto_finish()))
        }
    }
}

pub(super) fn open_archive_reader(
    archive_path: &Path,
    compression: &TransferCompression,
) -> anyhow::Result<Box<dyn Read>> {
    let file = std::fs::File::open(archive_path)?;
    match compression {
        TransferCompression::None => Ok(Box::new(file)),
        TransferCompression::Zstd => Ok(Box::new(zstd::stream::read::Decoder::new(file)?)),
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

fn finish_archive_builder<W: Write>(mut builder: tar::Builder<W>) -> anyhow::Result<()> {
    builder.finish()?;
    drop(builder.into_inner()?);
    Ok(())
}
