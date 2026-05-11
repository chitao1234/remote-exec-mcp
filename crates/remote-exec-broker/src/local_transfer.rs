use std::path::{Path, PathBuf};

use remote_exec_proto::path::{
    PathPolicy, is_absolute_for_policy, linux_path_policy, normalize_for_system,
    windows_path_policy,
};
use remote_exec_proto::rpc::{
    TransferCompression, TransferImportRequest, TransferImportResponse, TransferPathInfoResponse,
    TransferSourceType, TransferWarning,
};
use remote_exec_proto::sandbox::{CompiledFilesystemSandbox, SandboxAccess, authorize_path};
use remote_exec_proto::transfer::TransferLimits;

use crate::daemon_client::DaemonClientError;

pub struct BundledArchiveSource {
    pub source_path: String,
    pub source_policy: PathPolicy,
    pub source_type: TransferSourceType,
    pub compression: TransferCompression,
    pub archive_path: PathBuf,
}

pub struct ExportedArchive {
    pub source_type: TransferSourceType,
    pub warnings: Vec<TransferWarning>,
}

pub struct ExportedArchiveStream {
    pub source_type: TransferSourceType,
    pub reader: tokio::io::DuplexStream,
}

pub async fn export_path_to_archive(
    path: &str,
    archive_path: &Path,
    request: &remote_exec_proto::rpc::TransferExportRequest,
    sandbox: Option<&CompiledFilesystemSandbox>,
) -> anyhow::Result<ExportedArchive> {
    let exported = remote_exec_host::transfer::archive::export_path_to_file(
        path,
        archive_path,
        request.compression.clone(),
        request.symlink_mode.clone(),
        &request.exclude,
        sandbox,
        None,
    )
    .await?;
    Ok(ExportedArchive {
        source_type: exported.source_type,
        warnings: exported.warnings,
    })
}

pub async fn export_path_to_stream(
    path: &str,
    request: &remote_exec_proto::rpc::TransferExportRequest,
    sandbox: Option<&CompiledFilesystemSandbox>,
) -> anyhow::Result<ExportedArchiveStream> {
    let exported = remote_exec_host::transfer::archive::export_path_to_stream(
        path,
        request.compression.clone(),
        request.symlink_mode.clone(),
        &request.exclude,
        sandbox,
        None,
    )
    .await?;
    Ok(ExportedArchiveStream {
        source_type: exported.source_type,
        reader: exported.reader,
    })
}

pub async fn import_archive_from_file(
    archive_path: &Path,
    request: &TransferImportRequest,
    sandbox: Option<&CompiledFilesystemSandbox>,
    limits: TransferLimits,
) -> anyhow::Result<TransferImportResponse> {
    remote_exec_host::transfer::archive::import_archive_from_file(
        archive_path,
        request,
        sandbox,
        None,
        limits,
    )
    .await
    .map_err(Into::into)
}

pub async fn import_archive_from_async_reader<R>(
    reader: R,
    request: &TransferImportRequest,
    sandbox: Option<&CompiledFilesystemSandbox>,
    limits: TransferLimits,
) -> anyhow::Result<TransferImportResponse>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    remote_exec_host::transfer::archive::import_archive_from_async_reader(
        reader, request, sandbox, None, limits,
    )
    .await
    .map_err(Into::into)
}

pub fn path_info(
    path: &str,
    sandbox: Option<&CompiledFilesystemSandbox>,
) -> Result<TransferPathInfoResponse, DaemonClientError> {
    let policy = host_policy();
    if !is_absolute_for_policy(policy, path) {
        return Err(crate::local_backend::map_local_transfer_error(
            remote_exec_host::TransferError::path_not_absolute(format!(
                "transfer endpoint path `{path}` is not absolute"
            )),
        ));
    }
    let path = PathBuf::from(normalize_for_system(policy, path));
    authorize_path(policy, sandbox, SandboxAccess::Write, &path).map_err(|err| {
        crate::local_backend::map_local_transfer_error(
            remote_exec_host::TransferError::sandbox_denied(err.to_string()),
        )
    })?;

    match std::fs::symlink_metadata(&path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(crate::local_backend::map_local_transfer_error(
                    remote_exec_host::TransferError::destination_unsupported(format!(
                        "destination path contains unsupported symlink `{}`",
                        path.display()
                    )),
                ));
            }
            Ok(TransferPathInfoResponse {
                exists: true,
                is_directory: metadata.is_dir(),
            })
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(TransferPathInfoResponse {
            exists: false,
            is_directory: false,
        }),
        Err(err) => Err(crate::local_backend::map_local_transfer_error(
            remote_exec_host::TransferError::internal(err.to_string()),
        )),
    }
}

fn host_policy() -> PathPolicy {
    if cfg!(windows) {
        windows_path_policy()
    } else {
        linux_path_policy()
    }
}

pub async fn bundle_archives_to_file(
    sources: Vec<BundledArchiveSource>,
    archive_path: &Path,
    compression: TransferCompression,
) -> anyhow::Result<()> {
    remote_exec_host::transfer::archive::bundle_archives_to_file(
        sources
            .into_iter()
            .map(
                |source| remote_exec_host::transfer::archive::BundledArchiveSource {
                    source_path: PathBuf::from(normalize_for_system(
                        source.source_policy,
                        &source.source_path,
                    )),
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
    .map_err(Into::into)
}
