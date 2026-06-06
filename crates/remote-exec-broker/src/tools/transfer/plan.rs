use remote_exec_proto::public::{TransferDestinationMode, TransferEndpoint, TransferFilesInput};
use remote_exec_proto::transfer::{TransferCompression, TransferOverwrite, TransferSymlinkMode};

use super::endpoints::{
    PlannedEndpoint, PlannedSource, TransferPlanningContext, ensure_absolute,
    ensure_distinct_endpoints, ensure_multi_source_basenames_are_unique,
    negotiate_transfer_compression, resolve_destination,
};

pub(super) struct TransferPlanRequest {
    pub(super) sources: Vec<TransferEndpoint>,
    pub(super) requested_destination: TransferEndpoint,
    pub(super) overwrite: TransferOverwrite,
    pub(super) destination_mode: TransferDestinationMode,
    pub(super) exclude: Vec<String>,
    pub(super) symlink_mode: TransferSymlinkMode,
    pub(super) create_parent: bool,
}

pub(super) struct TransferPlan {
    pub(super) sources: Vec<PlannedSource>,
    pub(super) requested_destination: TransferEndpoint,
    pub(super) destination: PlannedEndpoint,
    pub(super) overwrite: TransferOverwrite,
    pub(super) destination_mode: TransferDestinationMode,
    pub(super) compression: TransferCompression,
    pub(super) exclude: Vec<String>,
    pub(super) symlink_mode: TransferSymlinkMode,
    pub(super) create_parent: bool,
}

#[derive(Clone, Copy)]
pub(super) struct TransferExecutionOptions<'a> {
    pub(super) overwrite: &'a TransferOverwrite,
    pub(super) compression: &'a TransferCompression,
    pub(super) exclude: &'a [String],
    pub(super) symlink_mode: &'a TransferSymlinkMode,
    pub(super) create_parent: bool,
}

impl TransferPlanRequest {
    pub(super) fn from_input(input: TransferFilesInput) -> anyhow::Result<Self> {
        Ok(Self {
            sources: input.resolved_sources()?,
            requested_destination: input.destination,
            overwrite: input.overwrite,
            destination_mode: input.destination_mode,
            exclude: input.exclude,
            symlink_mode: input.symlink_mode,
            create_parent: input.create_parent,
        })
    }

    pub(super) fn input_targets(&self) -> Vec<&str> {
        let mut targets = Vec::with_capacity(self.sources.len() + 1);
        targets.extend(self.sources.iter().map(|source| source.target.as_str()));
        targets.push(self.requested_destination.target.as_str());
        targets
    }
}

impl TransferPlan {
    pub(super) fn execution_options(&self) -> TransferExecutionOptions<'_> {
        TransferExecutionOptions {
            overwrite: &self.overwrite,
            compression: &self.compression,
            exclude: &self.exclude,
            symlink_mode: &self.symlink_mode,
            create_parent: self.create_parent,
        }
    }

    pub(super) fn first_source_target(&self) -> &str {
        self.sources
            .first()
            .map(|source| source.endpoint.target.as_str())
            .unwrap_or("unknown")
    }

    pub(super) fn first_source_path(&self) -> &str {
        self.sources
            .first()
            .map(|source| source.endpoint.path.as_str())
            .unwrap_or("unknown")
    }
}

pub(super) async fn plan_transfer(
    state: &crate::BrokerState,
    request: TransferPlanRequest,
) -> anyhow::Result<TransferPlan> {
    let planning =
        TransferPlanningContext::new(state, &request.sources, &request.requested_destination)
            .await?;
    let compression = negotiate_transfer_compression(
        &planning,
        state.enable_transfer_compression,
        &request.sources,
        &request.requested_destination,
    )?;

    for source in &request.sources {
        ensure_absolute(&planning, source)?;
    }
    ensure_absolute(&planning, &request.requested_destination)?;
    ensure_multi_source_basenames_are_unique(
        &planning,
        &request.sources,
        &request.requested_destination,
    )?;
    let destination = resolve_destination(
        state,
        &planning,
        &request.sources,
        &request.requested_destination,
        &request.destination_mode,
    )
    .await?;
    for source in &request.sources {
        ensure_distinct_endpoints(&planning, source, &destination)?;
    }
    let sources = planning.planned_sources(&request.sources)?;
    let destination = PlannedEndpoint::new(&planning, destination)?;

    Ok(TransferPlan {
        sources,
        requested_destination: request.requested_destination,
        destination,
        overwrite: request.overwrite,
        destination_mode: request.destination_mode,
        compression,
        exclude: request.exclude,
        symlink_mode: request.symlink_mode,
        create_parent: request.create_parent,
    })
}
