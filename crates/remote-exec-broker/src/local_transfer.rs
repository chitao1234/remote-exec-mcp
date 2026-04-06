use std::path::Path;

use remote_exec_proto::rpc::{
    TransferCompression, TransferImportRequest, TransferImportResponse, TransferSourceType,
};
use remote_exec_proto::sandbox::CompiledFilesystemSandbox;

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
    )
    .await
}

pub async fn import_archive_from_file(
    archive_path: &Path,
    request: &TransferImportRequest,
    sandbox: Option<&CompiledFilesystemSandbox>,
) -> anyhow::Result<TransferImportResponse> {
    remote_exec_daemon::transfer::archive::import_archive_from_file(archive_path, request, sandbox)
        .await
}
