use std::collections::BTreeMap;
use std::path::Path;

use remote_exec_host::path_compare;
use remote_exec_proto::path::{PathPolicy, host_policy};
use remote_exec_proto::public::{TransferDestinationMode, TransferEndpoint};
use remote_exec_proto::rpc::{
    RpcErrorCode, TRANSFER_STREAM_PROTOCOL_VERSION, TransferPathInfoRequest,
};
use remote_exec_proto::transfer::TransferCompression;

use crate::daemon_client::{RpcToolErrorMode, normalize_tool_error};
use crate::local::BrokerHostOrTarget;

use super::backend::{TransferBackend, backend_for_endpoint};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TransferPlanningContext {
    targets: BTreeMap<String, EndpointTargetContext>,
}

#[derive(Debug, Clone)]
pub(super) struct PlannedSource {
    pub(super) endpoint: TransferEndpoint,
    pub(super) policy: PathPolicy,
    context: EndpointTargetContext,
}

#[derive(Debug, Clone)]
pub(super) struct PlannedEndpoint {
    endpoint: TransferEndpoint,
    context: EndpointTargetContext,
}

impl PlannedEndpoint {
    pub(super) fn new(
        planning: &TransferPlanningContext,
        endpoint: TransferEndpoint,
    ) -> anyhow::Result<Self> {
        let context = planning.endpoint_context(&endpoint)?;
        Ok(Self { endpoint, context })
    }

    pub(super) fn endpoint(&self) -> &TransferEndpoint {
        &self.endpoint
    }

    pub(super) fn context(&self) -> &EndpointTargetContext {
        &self.context
    }

    pub(super) fn new_for_source(source: &PlannedSource) -> Self {
        Self {
            endpoint: source.endpoint.clone(),
            context: source.context.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum EndpointTargetContext {
    Local {
        policy: PathPolicy,
    },
    Remote {
        policy: PathPolicy,
        accepts_single_slash_windows_absolute: bool,
        supports_transfer_compression: bool,
        daemon_instance_id: String,
    },
}

async fn verified_remote_daemon_info(
    state: &crate::BrokerState,
    target_name: &str,
) -> anyhow::Result<crate::CachedDaemonInfo> {
    state
        .verified_remote_target(target_name)
        .await?
        .cached_daemon_info()
        .await
        .ok_or_else(|| anyhow::anyhow!("target info missing after identity verification"))
}

async fn endpoint_target_context(
    state: &crate::BrokerState,
    target_name: &str,
) -> anyhow::Result<EndpointTargetContext> {
    match BrokerHostOrTarget::from_name(target_name) {
        BrokerHostOrTarget::BrokerHost => Ok(EndpointTargetContext::local()),
        BrokerHostOrTarget::Target(target_name) => EndpointTargetContext::remote(
            target_name,
            verified_remote_daemon_info(state, target_name).await?,
        ),
    }
}

impl TransferPlanningContext {
    pub(super) async fn new(
        state: &crate::BrokerState,
        sources: &[TransferEndpoint],
        destination: &TransferEndpoint,
    ) -> anyhow::Result<Self> {
        let mut targets = BTreeMap::new();
        for target in sources
            .iter()
            .map(|source| source.target.as_str())
            .chain(std::iter::once(destination.target.as_str()))
        {
            if !targets.contains_key(target) {
                targets.insert(
                    target.to_string(),
                    endpoint_target_context(state, target).await?,
                );
            }
        }
        Ok(Self { targets })
    }

    pub(super) fn endpoint_policy(
        &self,
        endpoint: &TransferEndpoint,
    ) -> anyhow::Result<PathPolicy> {
        Ok(self.endpoint_context(endpoint)?.policy())
    }

    pub(super) fn planned_sources(
        &self,
        sources: &[TransferEndpoint],
    ) -> anyhow::Result<Vec<PlannedSource>> {
        sources
            .iter()
            .map(|source| {
                let context = self.endpoint_context(source)?;
                Ok(PlannedSource {
                    endpoint: source.clone(),
                    policy: context.policy(),
                    context,
                })
            })
            .collect()
    }

    pub(super) fn endpoint_context(
        &self,
        endpoint: &TransferEndpoint,
    ) -> anyhow::Result<EndpointTargetContext> {
        self.targets.get(&endpoint.target).cloned().ok_or_else(|| {
            anyhow::anyhow!(
                "missing transfer planning context for target `{}`",
                endpoint.target
            )
        })
    }
}

pub(super) fn ensure_absolute(
    planning: &TransferPlanningContext,
    endpoint: &TransferEndpoint,
) -> anyhow::Result<()> {
    let context = planning.endpoint_context(endpoint)?;
    anyhow::ensure!(
        context.is_absolute_path(&endpoint.path),
        "transfer endpoint path `{}` is not absolute",
        endpoint.path
    );
    Ok(())
}

pub(super) fn ensure_distinct_endpoints(
    planning: &TransferPlanningContext,
    source: &TransferEndpoint,
    destination: &TransferEndpoint,
) -> anyhow::Result<()> {
    if source.target != destination.target {
        return Ok(());
    }

    let policy = planning.endpoint_policy(source)?;
    let context = planning.endpoint_context(source)?;
    anyhow::ensure!(
        !paths_match_for_preflight(&context, policy, &source.path, &destination.path),
        "source and destination must differ"
    );
    Ok(())
}

pub(super) fn ensure_multi_source_basenames_are_unique(
    planning: &TransferPlanningContext,
    sources: &[TransferEndpoint],
    destination: &TransferEndpoint,
) -> anyhow::Result<()> {
    if sources.len() <= 1 {
        return Ok(());
    }

    let destination_context = planning.endpoint_context(destination)?;
    let destination_policy = planning.endpoint_policy(destination)?;
    let mut seen_paths: Vec<String> = Vec::with_capacity(sources.len());
    for source in sources {
        let source_policy = planning.endpoint_policy(source)?;
        let basename = source_policy.basename(&source.path).ok_or_else(|| {
            anyhow::anyhow!(
                "transfer source path `{}` has no usable basename for multi-source transfer",
                source.path
            )
        })?;
        let candidate = destination_policy.join(&destination.path, &basename);
        anyhow::ensure!(
            !seen_paths.iter().any(|existing| paths_match_for_preflight(
                &destination_context,
                destination_policy,
                existing,
                &candidate
            )),
            "multi-source transfer would create duplicate destination entry `{basename}`"
        );
        seen_paths.push(candidate);
    }

    Ok(())
}

pub(super) async fn resolve_destination(
    state: &crate::BrokerState,
    planning: &TransferPlanningContext,
    sources: &[TransferEndpoint],
    destination: &TransferEndpoint,
    destination_mode: &TransferDestinationMode,
) -> anyhow::Result<TransferEndpoint> {
    let resolved_path = match destination_mode {
        TransferDestinationMode::Exact => destination.path.clone(),
        TransferDestinationMode::IntoDirectory => {
            resolve_into_directory_destination(planning, sources, destination)?
        }
        TransferDestinationMode::Auto => {
            let context = planning.endpoint_context(destination)?;
            if sources.len() == 1
                && (path_looks_like_directory(&context, &destination.path)
                    || existing_destination_is_directory(state, destination).await?)
            {
                resolve_into_directory_destination(planning, sources, destination)?
            } else {
                destination.path.clone()
            }
        }
    };

    Ok(TransferEndpoint {
        target: destination.target.clone(),
        path: resolved_path,
    })
}

fn resolve_into_directory_destination(
    planning: &TransferPlanningContext,
    sources: &[TransferEndpoint],
    destination: &TransferEndpoint,
) -> anyhow::Result<String> {
    let destination_context = planning.endpoint_context(destination)?;
    let destination_policy = destination_context.policy();
    let mut candidates: Vec<String> = Vec::with_capacity(sources.len());
    for source in sources {
        let source_policy = planning.endpoint_policy(source)?;
        let basename = source_policy.basename(&source.path).ok_or_else(|| {
            anyhow::anyhow!(
                "transfer source path `{}` has no usable basename for destination directory mode",
                source.path
            )
        })?;
        let candidate = join_child_for_context(&destination_context, &destination.path, &basename);
        anyhow::ensure!(
            !candidates.iter().any(|existing| paths_match_for_preflight(
                &destination_context,
                destination_policy,
                existing,
                &candidate
            )),
            "destination directory mode would create duplicate destination entry `{basename}`"
        );
        candidates.push(candidate);
    }

    match candidates.as_slice() {
        [candidate] => Ok(candidate.clone()),
        _ => Ok(destination.path.clone()),
    }
}

fn paths_match_for_preflight(
    context: &EndpointTargetContext,
    policy: PathPolicy,
    left: &str,
    right: &str,
) -> bool {
    match context {
        EndpointTargetContext::Local { .. } => {
            path_compare::path_eq(Path::new(left), Path::new(right))
        }
        EndpointTargetContext::Remote { .. } => policy.syntax_eq(left, right),
    }
}

fn join_child_for_context(context: &EndpointTargetContext, base: &str, child: &str) -> String {
    if matches!(
        context,
        EndpointTargetContext::Remote {
            accepts_single_slash_windows_absolute: true,
            ..
        }
    ) && base.starts_with('/')
        && !base.starts_with("//")
        && !context.policy().is_absolute(base)
    {
        let trimmed_base = base.trim_end_matches('/');
        if trimmed_base.is_empty() {
            format!("/{child}")
        } else {
            format!("{trimmed_base}/{child}")
        }
    } else {
        context.policy().join(base, child)
    }
}

fn path_looks_like_directory(context: &EndpointTargetContext, path: &str) -> bool {
    if path.ends_with('/') {
        return true;
    }

    matches!(
        context.policy().style,
        remote_exec_proto::path::PathStyle::Windows
    ) && path.ends_with('\\')
}

async fn existing_destination_is_directory(
    state: &crate::BrokerState,
    destination: &TransferEndpoint,
) -> anyhow::Result<bool> {
    let backend = backend_for_endpoint(state, destination).await?;
    let result = backend
        .path_info(&TransferPathInfoRequest {
            path: destination.path.clone(),
        })
        .await;

    match result {
        Ok(info) => Ok(info.exists && info.is_directory),
        Err(err) if path_info_missing_or_unsupported(&err) => Ok(false),
        Err(err) => Err(normalize_path_info_error(err)),
    }
}

fn normalize_path_info_error(err: crate::daemon_client::DaemonClientError) -> anyhow::Error {
    normalize_tool_error(err.into(), RpcToolErrorMode::MessageOnly)
}

fn path_info_missing_or_unsupported(err: &crate::daemon_client::DaemonClientError) -> bool {
    match err {
        crate::daemon_client::DaemonClientError::Rpc { status, .. } => {
            *status == reqwest::StatusCode::NOT_FOUND
                || *status == reqwest::StatusCode::METHOD_NOT_ALLOWED
                || matches!(
                    err.rpc_error_code(),
                    Some(RpcErrorCode::NotFound | RpcErrorCode::UnknownEndpoint)
                )
        }
        _ => false,
    }
}

pub(super) fn negotiate_transfer_compression(
    planning: &TransferPlanningContext,
    enable_transfer_compression: bool,
    sources: &[TransferEndpoint],
    destination: &TransferEndpoint,
) -> anyhow::Result<TransferCompression> {
    if !enable_transfer_compression {
        return Ok(TransferCompression::None);
    }

    let mut has_remote_endpoint = false;
    for endpoint in sources.iter().chain(std::iter::once(destination)) {
        let context = planning.endpoint_context(endpoint)?;
        let Some(supports_transfer_compression) = context.supports_transfer_compression() else {
            continue;
        };

        has_remote_endpoint = true;
        if !supports_transfer_compression {
            return Ok(TransferCompression::None);
        }
    }

    if has_remote_endpoint {
        Ok(TransferCompression::Zstd)
    } else {
        Ok(TransferCompression::None)
    }
}

impl EndpointTargetContext {
    fn local() -> Self {
        Self::Local {
            policy: host_policy(),
        }
    }

    fn remote(target_name: &str, info: crate::CachedDaemonInfo) -> anyhow::Result<Self> {
        let transfer_stream_version = info
            .capabilities
            .transfer_stream_protocol_version
            .map(|version| version.get());
        anyhow::ensure!(
            transfer_stream_version == Some(TRANSFER_STREAM_PROTOCOL_VERSION),
            "target `{target_name}` does not support transfer stream protocol version {TRANSFER_STREAM_PROTOCOL_VERSION}"
        );
        let accepts_single_slash_windows_absolute = info.platform_is_windows();
        Ok(Self::Remote {
            policy: info.path_policy(),
            accepts_single_slash_windows_absolute,
            supports_transfer_compression: info.supports_transfer_compression,
            daemon_instance_id: info.daemon_instance_id,
        })
    }

    pub(super) fn planned_daemon_instance_id(&self) -> Option<&str> {
        match self {
            Self::Local { .. } => None,
            Self::Remote {
                daemon_instance_id, ..
            } => Some(daemon_instance_id),
        }
    }

    fn policy(&self) -> PathPolicy {
        match self {
            Self::Local { policy } | Self::Remote { policy, .. } => *policy,
        }
    }

    fn is_absolute_path(&self, path: &str) -> bool {
        self.policy().is_absolute(path)
            || matches!(
                self,
                Self::Remote {
                    accepts_single_slash_windows_absolute: true,
                    ..
                } if path.starts_with('/') && !path.starts_with("//")
            )
    }

    fn supports_transfer_compression(&self) -> Option<bool> {
        match self {
            Self::Local { .. } => None,
            Self::Remote {
                supports_transfer_compression,
                ..
            } => Some(*supports_transfer_compression),
        }
    }
}
