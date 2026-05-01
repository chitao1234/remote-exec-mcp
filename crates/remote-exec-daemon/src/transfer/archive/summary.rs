use std::io::{Read, Write};
use std::path::Path;

use remote_exec_proto::rpc::TransferWarning;
use serde::{Deserialize, Serialize};

use super::TRANSFER_SUMMARY_ENTRY;

#[derive(Debug, Default, Deserialize, Serialize)]
struct TransferArchiveSummary {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<TransferWarning>,
}

pub(super) fn is_transfer_summary_path(path: &Path) -> bool {
    path == Path::new(TRANSFER_SUMMARY_ENTRY)
}

pub(super) fn append_transfer_summary<W: Write>(
    builder: &mut tar::Builder<W>,
    warnings: &[TransferWarning],
) -> anyhow::Result<()> {
    if warnings.is_empty() {
        return Ok(());
    }

    let summary = TransferArchiveSummary {
        warnings: warnings.to_vec(),
    };
    let body = serde_json::to_vec(&summary)?;
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Regular);
    header.set_mode(0o600);
    header.set_size(body.len() as u64);
    header.set_cksum();
    builder.append_data(&mut header, TRANSFER_SUMMARY_ENTRY, body.as_slice())?;
    Ok(())
}

pub(super) fn read_transfer_summary<R: Read>(
    entry: &mut tar::Entry<R>,
) -> anyhow::Result<Vec<TransferWarning>> {
    anyhow::ensure!(
        entry.header().entry_type().is_file(),
        "transfer summary archive entry is not a regular file"
    );
    let mut body = Vec::new();
    entry.read_to_end(&mut body)?;
    let summary = serde_json::from_slice::<TransferArchiveSummary>(&body)?;
    Ok(summary.warnings)
}
