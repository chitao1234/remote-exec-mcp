use std::collections::BTreeMap;
use std::io::{self, Read};
use std::sync::Arc;

use bytes::{Bytes, BytesMut};
use remote_exec_proto::public::TransferEndpoint;
use remote_exec_proto::rpc::{
    TransferExportRequest, TransferImportRequest, TransferImportResponse,
};
use remote_exec_proto::transfer::{
    TransferCompression, TransferOverwrite, TransferSourceType, TransferSymlinkMode,
};
use tokio::io::AsyncReadExt;
use tokio::sync::{Semaphore, mpsc};

use super::backend::{TransferArchiveStream, TransferBackend, backend_for_planned_endpoint};
use super::endpoints::{PlannedEndpoint, PlannedSource};
use super::plan::{TransferExecutionOptions, TransferPlan};

const MAX_PARALLEL_SOURCE_EXPORTS: usize = 4;
const MAX_PARALLEL_EXPORTS_PER_TARGET: usize = 1;
const SOURCE_STREAM_BUFFER_SIZE: usize = 256 * 1024;
const SOURCE_STREAM_CHANNEL_DEPTH: usize = 4;
const REPACK_PIPE_BUFFER_SIZE: usize = 1024 * 1024;

struct StreamingSourceArchive {
    source: PlannedSource,
    source_type: TransferSourceType,
    receiver: mpsc::Receiver<SourceArchiveStreamItem>,
}

enum SourceArchiveStreamItem {
    Data(Bytes),
    Complete,
    Error(io::Error),
}

struct SourceArchiveReader {
    receiver: mpsc::Receiver<SourceArchiveStreamItem>,
    pending: Bytes,
    offset: usize,
    complete: bool,
}

pub(super) async fn execute_transfer_plan(
    state: &crate::BrokerState,
    plan: &TransferPlan,
) -> anyhow::Result<(TransferSourceType, TransferImportResponse)> {
    let options = plan.execution_options();
    match plan.sources.as_slice() {
        [source] => transfer_single_source(state, source, &plan.destination, options).await,
        _ => transfer_multiple_sources(state, &plan.sources, &plan.destination, options).await,
    }
}

async fn transfer_single_source(
    state: &crate::BrokerState,
    source: &PlannedSource,
    destination: &PlannedEndpoint,
    options: TransferExecutionOptions<'_>,
) -> anyhow::Result<(TransferSourceType, TransferImportResponse)> {
    let export_request = build_export_request(
        &source.endpoint,
        options.compression,
        options.exclude,
        options.symlink_mode,
    );
    let exported = export_single_source(state, source, &export_request).await?;
    let source_type = *exported.source_type();
    let request = build_import_request(
        destination.endpoint(),
        options.overwrite,
        source_type,
        options.compression,
        options.symlink_mode,
        options.create_parent,
    );
    let summary = import_single_source(state, destination, &request, exported).await?;
    Ok((source_type, summary))
}

async fn transfer_multiple_sources(
    state: &crate::BrokerState,
    sources: &[PlannedSource],
    destination: &PlannedEndpoint,
    options: TransferExecutionOptions<'_>,
) -> anyhow::Result<(TransferSourceType, TransferImportResponse)> {
    let exported_sources = start_streaming_source_exports(
        state,
        sources,
        options.compression,
        options.exclude,
        options.symlink_mode,
    )
    .await?;

    let source_type = TransferSourceType::Multiple;
    let request = build_import_request(
        destination.endpoint(),
        options.overwrite,
        source_type,
        options.compression,
        options.symlink_mode,
        options.create_parent,
    );
    let summary = import_streaming_multi_source(
        state,
        destination,
        &request,
        exported_sources,
        *options.compression,
    )
    .await?;
    Ok((source_type, summary))
}

fn build_export_request(
    endpoint: &TransferEndpoint,
    compression: &TransferCompression,
    exclude: &[String],
    symlink_mode: &TransferSymlinkMode,
) -> TransferExportRequest {
    TransferExportRequest {
        path: endpoint.path.clone(),
        compression: *compression,
        symlink_mode: *symlink_mode,
        exclude: exclude.to_vec(),
    }
}

async fn export_single_source(
    state: &crate::BrokerState,
    source: &PlannedSource,
    request: &TransferExportRequest,
) -> anyhow::Result<TransferArchiveStream> {
    let endpoint = PlannedEndpoint::new_for_source(source);
    backend_for_planned_endpoint(state, &endpoint)
        .await?
        .export_stream(request)
        .await
}

async fn import_single_source(
    state: &crate::BrokerState,
    destination: &PlannedEndpoint,
    request: &TransferImportRequest,
    exported: TransferArchiveStream,
) -> anyhow::Result<TransferImportResponse> {
    backend_for_planned_endpoint(state, destination)
        .await?
        .import_stream(request, exported)
        .await
}

async fn start_streaming_source_exports(
    state: &crate::BrokerState,
    sources: &[PlannedSource],
    compression: &TransferCompression,
    exclude: &[String],
    symlink_mode: &TransferSymlinkMode,
) -> anyhow::Result<Vec<StreamingSourceArchive>> {
    let global_limit = Arc::new(Semaphore::new(MAX_PARALLEL_SOURCE_EXPORTS));
    let target_limits = sources
        .iter()
        .map(|source| {
            (
                source.endpoint.target.clone(),
                Arc::new(Semaphore::new(MAX_PARALLEL_EXPORTS_PER_TARGET)),
            )
        })
        .collect::<BTreeMap<_, _>>();

    let mut exports = Vec::with_capacity(sources.len());
    for source in sources {
        let target_limit = target_limits
            .get(&source.endpoint.target)
            .expect("source target has a concurrency limit")
            .clone();
        let request = build_export_request(&source.endpoint, compression, exclude, symlink_mode);
        let exported = export_single_source(state, source, &request).await?;
        let source_type = *exported.source_type();
        let (sender, receiver) = mpsc::channel(SOURCE_STREAM_CHANNEL_DEPTH);
        tokio::spawn(pump_source_archive(
            exported.into_async_read(),
            sender,
            global_limit.clone(),
            target_limit,
        ));
        exports.push(StreamingSourceArchive {
            source: source.clone(),
            source_type,
            receiver,
        });
    }
    Ok(exports)
}

async fn pump_source_archive(
    mut reader: Box<dyn tokio::io::AsyncRead + Send + Unpin + 'static>,
    sender: mpsc::Sender<SourceArchiveStreamItem>,
    global_limit: Arc<Semaphore>,
    target_limit: Arc<Semaphore>,
) {
    let Ok(_global_permit) = global_limit.acquire_owned().await else {
        return;
    };
    let Ok(_target_permit) = target_limit.acquire_owned().await else {
        return;
    };
    loop {
        let mut buffer = BytesMut::with_capacity(SOURCE_STREAM_BUFFER_SIZE);
        match reader.read_buf(&mut buffer).await {
            Ok(0) => {
                let _ = sender.send(SourceArchiveStreamItem::Complete).await;
                return;
            }
            Ok(_) => {
                if sender
                    .send(SourceArchiveStreamItem::Data(buffer.freeze()))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            Err(err) => {
                let _ = sender.send(SourceArchiveStreamItem::Error(err)).await;
                return;
            }
        }
    }
}

async fn import_streaming_multi_source(
    state: &crate::BrokerState,
    destination: &PlannedEndpoint,
    request: &TransferImportRequest,
    sources: Vec<StreamingSourceArchive>,
    compression: TransferCompression,
) -> anyhow::Result<TransferImportResponse> {
    let (writer, reader) = tokio::io::duplex(REPACK_PIPE_BUFFER_SIZE);
    let runtime = tokio::runtime::Handle::current();
    let repack = tokio::task::spawn_blocking(move || {
        let reader_sources = sources
            .into_iter()
            .map(
                |source| remote_exec_host::transfer::archive::BundledArchiveReaderSource {
                    source_path: std::path::PathBuf::from(
                        source
                            .source
                            .policy
                            .normalize_for_system(&source.source.endpoint.path),
                    ),
                    source_policy: source.source.policy,
                    source_type: source.source_type,
                    compression,
                    reader: Box::new(SourceArchiveReader::new(source.receiver)),
                },
            )
            .collect();
        let writer = tokio_util::io::SyncIoBridge::new_with_handle(writer, runtime);
        remote_exec_host::transfer::archive::bundle_archives_to_writer(
            reader_sources,
            writer,
            compression,
        )
        .map_err(anyhow::Error::from)
    });

    let archive = TransferArchiveStream::from_async_read(TransferSourceType::Multiple, reader);
    let import_result = import_single_source(state, destination, request, archive).await;
    let repack_result = repack.await?;
    repack_result?;
    import_result
}

impl SourceArchiveReader {
    fn new(receiver: mpsc::Receiver<SourceArchiveStreamItem>) -> Self {
        Self {
            receiver,
            pending: Bytes::new(),
            offset: 0,
            complete: false,
        }
    }
}

impl Read for SourceArchiveReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }

        loop {
            if self.offset < self.pending.len() {
                let available = &self.pending[self.offset..];
                let count = available.len().min(output.len());
                output[..count].copy_from_slice(&available[..count]);
                self.offset += count;
                return Ok(count);
            }
            if self.complete {
                return Ok(0);
            }

            match self.receiver.blocking_recv() {
                Some(SourceArchiveStreamItem::Data(bytes)) => {
                    self.pending = bytes;
                    self.offset = 0;
                }
                Some(SourceArchiveStreamItem::Complete) => self.complete = true,
                Some(SourceArchiveStreamItem::Error(err)) => {
                    return Err(err);
                }
                None => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "source archive stream ended before terminal state",
                    ));
                }
            }
        }
    }
}

fn build_import_request(
    endpoint: &TransferEndpoint,
    overwrite: &TransferOverwrite,
    source_type: TransferSourceType,
    compression: &TransferCompression,
    symlink_mode: &TransferSymlinkMode,
    create_parent: bool,
) -> TransferImportRequest {
    TransferImportRequest {
        destination_path: endpoint.path.clone(),
        overwrite: *overwrite,
        create_parent,
        source_type,
        compression: *compression,
        symlink_mode: *symlink_mode,
    }
}
