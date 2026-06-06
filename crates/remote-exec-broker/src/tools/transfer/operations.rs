use remote_exec_proto::public::TransferEndpoint;
use remote_exec_proto::rpc::{
    TransferExportRequest, TransferImportRequest, TransferImportResponse,
};
use remote_exec_proto::transfer::{
    TransferCompression, TransferOverwrite, TransferSourceType, TransferSymlinkMode,
};

use super::backend::{TransferArchiveStream, TransferBackend, backend_for_planned_endpoint};
use super::endpoints::{PlannedEndpoint, PlannedSource};
use super::plan::{TransferExecutionOptions, TransferPlan};

struct ExportedSourceArchive {
    source: PlannedSource,
    source_type: TransferSourceType,
    temp_path: tempfile::TempPath,
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
    let source_type = exported.source_type().clone();
    let request = build_import_request(
        destination.endpoint(),
        options.overwrite,
        source_type.clone(),
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
    let mut exported_sources = Vec::with_capacity(sources.len());
    for source in sources {
        let temp = tempfile::NamedTempFile::new()?;
        let temp_path = temp.into_temp_path();
        let exported = export_endpoint_to_archive(
            state,
            source,
            temp_path.as_ref(),
            options.compression,
            options.exclude,
            options.symlink_mode,
        )
        .await?;
        exported_sources.push(ExportedSourceArchive {
            source: source.clone(),
            source_type: exported.source_type,
            temp_path,
        });
    }

    let bundled = tempfile::NamedTempFile::new()?;
    let bundled_path = bundled.into_temp_path();
    crate::local::transfer::bundle_archives_to_file(
        exported_sources
            .iter()
            .map(|source| crate::local::transfer::BundledArchiveSource {
                source_path: source.source.endpoint.path.clone(),
                source_policy: source.source.policy,
                source_type: source.source_type.clone(),
                compression: options.compression.clone(),
                archive_path: source.temp_path.to_path_buf(),
            })
            .collect(),
        bundled_path.as_ref(),
        options.compression.clone(),
    )
    .await?;

    let source_type = TransferSourceType::Multiple;
    let request = build_import_request(
        destination.endpoint(),
        options.overwrite,
        source_type.clone(),
        options.compression,
        options.symlink_mode,
        options.create_parent,
    );
    let summary =
        import_archive_to_endpoint(state, bundled_path.as_ref(), destination, &request).await?;
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
        compression: compression.clone(),
        symlink_mode: symlink_mode.clone(),
        exclude: exclude.to_vec(),
    }
}

async fn export_endpoint_to_archive(
    state: &crate::BrokerState,
    source: &PlannedSource,
    archive_path: &std::path::Path,
    compression: &TransferCompression,
    exclude: &[String],
    symlink_mode: &TransferSymlinkMode,
) -> anyhow::Result<super::backend::ExportedArchive> {
    let request = build_export_request(&source.endpoint, compression, exclude, symlink_mode);
    let endpoint = PlannedEndpoint::new_for_source(source);
    backend_for_planned_endpoint(state, &endpoint)
        .await?
        .export_to_file(&request, archive_path)
        .await
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

async fn import_archive_to_endpoint(
    state: &crate::BrokerState,
    archive_path: &std::path::Path,
    endpoint: &PlannedEndpoint,
    request: &TransferImportRequest,
) -> anyhow::Result<TransferImportResponse> {
    backend_for_planned_endpoint(state, endpoint)
        .await?
        .import_from_file(archive_path, request)
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
        overwrite: overwrite.clone(),
        create_parent,
        source_type,
        compression: compression.clone(),
        symlink_mode: symlink_mode.clone(),
    }
}
