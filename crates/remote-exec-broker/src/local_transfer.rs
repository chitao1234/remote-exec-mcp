use std::path::{Path, PathBuf};

use remote_exec_proto::path::PathPolicy;
use remote_exec_proto::rpc::{
    TransferCompression, TransferImportRequest, TransferImportResponse, TransferSourceType,
};
use remote_exec_proto::sandbox::CompiledFilesystemSandbox;

pub struct BundledArchiveSource {
    pub source_path: String,
    pub source_policy: PathPolicy,
    pub source_type: TransferSourceType,
    pub compression: TransferCompression,
    pub archive_path: PathBuf,
}

pub async fn export_path_to_archive(
    path: &str,
    archive_path: &Path,
    compression: TransferCompression,
    sandbox: Option<&CompiledFilesystemSandbox>,
) -> anyhow::Result<TransferSourceType> {
    remote_exec_daemon::transfer::archive::export_path_to_file(
        path,
        archive_path,
        compression,
        sandbox,
        None,
    )
    .await
}

pub async fn import_archive_from_file(
    archive_path: &Path,
    request: &TransferImportRequest,
    sandbox: Option<&CompiledFilesystemSandbox>,
) -> anyhow::Result<TransferImportResponse> {
    remote_exec_daemon::transfer::archive::import_archive_from_file(
        archive_path,
        request,
        sandbox,
        None,
    )
    .await
}

pub async fn bundle_archives_to_file(
    sources: Vec<BundledArchiveSource>,
    archive_path: &Path,
    compression: TransferCompression,
) -> anyhow::Result<()> {
    remote_exec_daemon::transfer::archive::bundle_archives_to_file(
        sources
            .into_iter()
            .map(
                |source| remote_exec_daemon::transfer::archive::BundledArchiveSource {
                    source_path: source.source_path,
                    source_policy: source.source_policy,
                    source_type: source.source_type,
                    compression: source.compression,
                    archive_path: source.archive_path,
                },
            )
            .collect(),
        archive_path,
        compression,
    )
    .await
}
