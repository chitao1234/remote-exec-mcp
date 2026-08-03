#![allow(dead_code)]

use std::path::Path;

use bytes::Bytes;
use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use futures_util::stream::BoxStream;
use remote_exec_host::sandbox::CompiledFilesystemSandbox;
use remote_exec_proto::public::TransferEndpoint;
use remote_exec_proto::rpc::{
    TransferExportRequest, TransferImportRequest, TransferImportResponse, TransferPathInfoRequest,
    TransferPathInfoResponse,
};
use remote_exec_proto::transfer::{TransferLimits, TransferSourceType};

use crate::daemon_client::{
    DaemonClientError, RpcToolErrorMode, TransferExportResponse, normalize_tool_result,
};
use crate::target::RemoteTargetHandle;

use super::endpoints::PlannedEndpoint;

pub(super) type ArchiveStream = BoxStream<'static, Result<Bytes, std::io::Error>>;

pub(super) struct ExportedArchive {
    pub(super) source_type: TransferSourceType,
}

pub(super) struct TransferArchiveStream {
    source_type: TransferSourceType,
    reader: Box<dyn tokio::io::AsyncRead + Send + Unpin + 'static>,
}

impl TransferArchiveStream {
    fn new(
        source_type: TransferSourceType,
        reader: impl tokio::io::AsyncRead + Send + Unpin + 'static,
    ) -> Self {
        Self {
            source_type,
            reader: Box::new(reader),
        }
    }

    pub(super) fn from_async_read(
        source_type: TransferSourceType,
        reader: impl tokio::io::AsyncRead + Send + Unpin + 'static,
    ) -> Self {
        Self::new(source_type, reader)
    }

    pub(super) fn source_type(&self) -> &TransferSourceType {
        &self.source_type
    }

    pub(super) fn into_async_read(self) -> Box<dyn tokio::io::AsyncRead + Send + Unpin + 'static> {
        self.reader
    }

    fn into_archive_stream(self) -> ArchiveStream {
        tokio_util::io::ReaderStream::new(self.reader).boxed()
    }
}

pub(super) trait TransferBackend {
    fn path_info<'a>(
        &'a self,
        request: &'a TransferPathInfoRequest,
    ) -> BoxFuture<'a, Result<TransferPathInfoResponse, DaemonClientError>>;

    fn export_to_file<'a>(
        &'a self,
        request: &'a TransferExportRequest,
        archive_path: &'a Path,
    ) -> BoxFuture<'a, anyhow::Result<ExportedArchive>>;

    fn export_stream<'a>(
        &'a self,
        request: &'a TransferExportRequest,
    ) -> BoxFuture<'a, anyhow::Result<TransferArchiveStream>>;

    fn import_from_file<'a>(
        &'a self,
        archive_path: &'a Path,
        request: &'a TransferImportRequest,
    ) -> BoxFuture<'a, anyhow::Result<TransferImportResponse>>;

    fn import_stream<'a>(
        &'a self,
        request: &'a TransferImportRequest,
        archive: TransferArchiveStream,
    ) -> BoxFuture<'a, anyhow::Result<TransferImportResponse>>;
}

pub(super) enum TransferEndpointBackend<'a> {
    BrokerHost(BrokerHostTransferBackend<'a>),
    Remote(RemoteTransferBackend<'a>),
}

pub(super) async fn backend_for_endpoint<'a>(
    state: &'a crate::BrokerState,
    endpoint: &'a TransferEndpoint,
) -> anyhow::Result<TransferEndpointBackend<'a>> {
    Ok(
        match crate::local::BrokerHostOrTarget::from_transfer_endpoint(endpoint) {
            crate::local::BrokerHostOrTarget::BrokerHost => broker_host_backend(state),
            crate::local::BrokerHostOrTarget::Target(target_name) => {
                let target = state.transfer_remote_target(target_name).await?;
                TransferEndpointBackend::Remote(RemoteTransferBackend {
                    state,
                    target_name,
                    target,
                })
            }
        },
    )
}

pub(super) async fn backend_for_planned_endpoint<'a>(
    state: &'a crate::BrokerState,
    endpoint: &'a PlannedEndpoint,
) -> anyhow::Result<TransferEndpointBackend<'a>> {
    let raw_endpoint = endpoint.endpoint();
    Ok(match endpoint.context().planned_daemon_instance_id() {
        None => broker_host_backend(state),
        Some(planned_daemon_instance_id) => {
            let target = state.transfer_remote_target(&raw_endpoint.target).await?;
            let current = match target.target_info().await {
                Ok(current) => current,
                Err(err) => {
                    state.trigger_remote_target_health_recheck(&raw_endpoint.target);
                    return Err(err.into_tool_error(RpcToolErrorMode::MessageOnly));
                }
            };
            anyhow::ensure!(
                current.daemon_instance_id == planned_daemon_instance_id,
                "target `{}` daemon instance changed during transfer planning",
                raw_endpoint.target
            );
            TransferEndpointBackend::Remote(RemoteTransferBackend {
                state,
                target_name: &raw_endpoint.target,
                target,
            })
        }
    })
}

fn broker_host_backend(state: &crate::BrokerState) -> TransferEndpointBackend<'_> {
    TransferEndpointBackend::BrokerHost(BrokerHostTransferBackend {
        sandbox: state.host_sandbox.as_ref(),
        windows_posix_root: state.host_filesystem.windows_posix_root(),
        limits: state.transfer_limits,
    })
}

pub(super) struct BrokerHostTransferBackend<'a> {
    sandbox: Option<&'a CompiledFilesystemSandbox>,
    windows_posix_root: Option<&'a Path>,
    limits: TransferLimits,
}

pub(super) struct RemoteTransferBackend<'a> {
    state: &'a crate::BrokerState,
    target_name: &'a str,
    target: RemoteTargetHandle<'a>,
}

impl TransferBackend for TransferEndpointBackend<'_> {
    fn path_info<'a>(
        &'a self,
        request: &'a TransferPathInfoRequest,
    ) -> BoxFuture<'a, Result<TransferPathInfoResponse, DaemonClientError>> {
        match self {
            Self::BrokerHost(backend) => backend.path_info(request),
            Self::Remote(backend) => backend.path_info(request),
        }
    }

    fn export_to_file<'a>(
        &'a self,
        request: &'a TransferExportRequest,
        archive_path: &'a Path,
    ) -> BoxFuture<'a, anyhow::Result<ExportedArchive>> {
        match self {
            Self::BrokerHost(backend) => backend.export_to_file(request, archive_path),
            Self::Remote(backend) => backend.export_to_file(request, archive_path),
        }
    }

    fn export_stream<'a>(
        &'a self,
        request: &'a TransferExportRequest,
    ) -> BoxFuture<'a, anyhow::Result<TransferArchiveStream>> {
        match self {
            Self::BrokerHost(backend) => backend.export_stream(request),
            Self::Remote(backend) => backend.export_stream(request),
        }
    }

    fn import_from_file<'a>(
        &'a self,
        archive_path: &'a Path,
        request: &'a TransferImportRequest,
    ) -> BoxFuture<'a, anyhow::Result<TransferImportResponse>> {
        match self {
            Self::BrokerHost(backend) => backend.import_from_file(archive_path, request),
            Self::Remote(backend) => backend.import_from_file(archive_path, request),
        }
    }

    fn import_stream<'a>(
        &'a self,
        request: &'a TransferImportRequest,
        archive: TransferArchiveStream,
    ) -> BoxFuture<'a, anyhow::Result<TransferImportResponse>> {
        match self {
            Self::BrokerHost(backend) => backend.import_stream(request, archive),
            Self::Remote(backend) => backend.import_stream(request, archive),
        }
    }
}

impl TransferBackend for BrokerHostTransferBackend<'_> {
    fn path_info<'a>(
        &'a self,
        request: &'a TransferPathInfoRequest,
    ) -> BoxFuture<'a, Result<TransferPathInfoResponse, DaemonClientError>> {
        Box::pin(async move {
            crate::local::transfer::path_info(&request.path, self.sandbox, self.windows_posix_root)
        })
    }

    fn export_to_file<'a>(
        &'a self,
        request: &'a TransferExportRequest,
        archive_path: &'a Path,
    ) -> BoxFuture<'a, anyhow::Result<ExportedArchive>> {
        Box::pin(async move {
            let exported = crate::local::transfer::export_path_to_archive(
                &request.path,
                archive_path,
                request,
                self.sandbox,
                self.windows_posix_root,
            )
            .await?;
            Ok(ExportedArchive {
                source_type: exported.source_type,
            })
        })
    }

    fn export_stream<'a>(
        &'a self,
        request: &'a TransferExportRequest,
    ) -> BoxFuture<'a, anyhow::Result<TransferArchiveStream>> {
        Box::pin(async move {
            let exported = crate::local::transfer::export_path_to_byte_stream(
                &request.path,
                request,
                self.sandbox,
                self.windows_posix_root,
            )
            .await?;
            let reader = crate::local::transfer::export_byte_stream_reader(exported.receiver);
            Ok(TransferArchiveStream::new(exported.source_type, reader))
        })
    }

    fn import_from_file<'a>(
        &'a self,
        archive_path: &'a Path,
        request: &'a TransferImportRequest,
    ) -> BoxFuture<'a, anyhow::Result<TransferImportResponse>> {
        Box::pin(async move {
            crate::local::transfer::import_archive_from_file(
                archive_path,
                request,
                self.sandbox,
                self.windows_posix_root,
                self.limits,
            )
            .await
        })
    }

    fn import_stream<'a>(
        &'a self,
        request: &'a TransferImportRequest,
        archive: TransferArchiveStream,
    ) -> BoxFuture<'a, anyhow::Result<TransferImportResponse>> {
        Box::pin(async move {
            crate::local::transfer::import_archive_from_async_reader(
                archive.into_async_read(),
                request,
                self.sandbox,
                self.windows_posix_root,
                self.limits,
            )
            .await
        })
    }
}

impl TransferBackend for RemoteTransferBackend<'_> {
    fn path_info<'a>(
        &'a self,
        request: &'a TransferPathInfoRequest,
    ) -> BoxFuture<'a, Result<TransferPathInfoResponse, DaemonClientError>> {
        Box::pin(async move {
            let result = self.target.transfer_path_info(request).await;
            if result.is_err() {
                self.state
                    .trigger_remote_target_health_recheck(self.target_name);
            }
            result
        })
    }

    fn export_to_file<'a>(
        &'a self,
        request: &'a TransferExportRequest,
        archive_path: &'a Path,
    ) -> BoxFuture<'a, anyhow::Result<ExportedArchive>> {
        Box::pin(async move {
            let exported = handle_remote_transfer_result(
                self.state,
                self.target_name,
                self.target,
                self.target
                    .transfer_export_to_file(request, archive_path)
                    .await,
            )
            .await?;
            Ok(exported_archive_from_response(exported))
        })
    }

    fn export_stream<'a>(
        &'a self,
        request: &'a TransferExportRequest,
    ) -> BoxFuture<'a, anyhow::Result<TransferArchiveStream>> {
        Box::pin(async move {
            let exported = handle_remote_transfer_result(
                self.state,
                self.target_name,
                self.target,
                self.target.transfer_export_stream(request).await,
            )
            .await?;
            let source_type = exported.source_type;
            Ok(TransferArchiveStream::new(
                source_type,
                exported.into_async_read(),
            ))
        })
    }

    fn import_from_file<'a>(
        &'a self,
        archive_path: &'a Path,
        request: &'a TransferImportRequest,
    ) -> BoxFuture<'a, anyhow::Result<TransferImportResponse>> {
        Box::pin(async move {
            handle_remote_transfer_result(
                self.state,
                self.target_name,
                self.target,
                self.target
                    .transfer_import_from_file(archive_path, request)
                    .await,
            )
            .await
        })
    }

    fn import_stream<'a>(
        &'a self,
        request: &'a TransferImportRequest,
        archive: TransferArchiveStream,
    ) -> BoxFuture<'a, anyhow::Result<TransferImportResponse>> {
        Box::pin(async move {
            handle_remote_transfer_result(
                self.state,
                self.target_name,
                self.target,
                self.target
                    .transfer_import_from_archive_stream(request, archive.into_archive_stream())
                    .await,
            )
            .await
        })
    }
}

fn exported_archive_from_response(response: TransferExportResponse) -> ExportedArchive {
    ExportedArchive {
        source_type: response.source_type,
    }
}

async fn handle_remote_transfer_result<T>(
    state: &crate::BrokerState,
    target_name: &str,
    target: RemoteTargetHandle<'_>,
    result: Result<T, DaemonClientError>,
) -> anyhow::Result<T> {
    let _ = target;
    if result.is_err() {
        state.trigger_remote_target_health_recheck(target_name);
    }
    normalize_tool_result(result, RpcToolErrorMode::MessageOnly)
}
