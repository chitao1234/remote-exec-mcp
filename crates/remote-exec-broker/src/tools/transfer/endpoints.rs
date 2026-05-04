use anyhow::Context;
use remote_exec_proto::path::{
    PathPolicy, basename_for_policy, is_absolute_for_policy, join_for_policy, linux_path_policy,
    same_path_for_policy, windows_path_policy,
};
use remote_exec_proto::public::{TransferDestinationMode, TransferEndpoint};
use remote_exec_proto::rpc::{
    TransferCompression as RpcTransferCompression, TransferPathInfoRequest,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EndpointTargetContext {
    Local {
        policy: PathPolicy,
    },
    Remote {
        policy: PathPolicy,
        accepts_single_slash_windows_absolute: bool,
        supports_transfer_compression: bool,
    },
}

pub(super) async fn verified_remote_target<'a>(
    state: &'a crate::BrokerState,
    target_name: &'a str,
) -> anyhow::Result<&'a crate::TargetHandle> {
    let target = state.target(target_name)?;
    target.ensure_identity_verified(target_name).await?;
    Ok(target)
}

async fn verified_remote_daemon_info(
    state: &crate::BrokerState,
    target_name: &str,
) -> anyhow::Result<crate::CachedDaemonInfo> {
    verified_remote_target(state, target_name)
        .await?
        .cached_daemon_info()
        .await
        .context("target info missing after identity verification")
}

async fn endpoint_target_context(
    state: &crate::BrokerState,
    target_name: &str,
) -> anyhow::Result<EndpointTargetContext> {
    if target_name == "local" {
        return Ok(EndpointTargetContext::local());
    }

    Ok(EndpointTargetContext::remote(
        verified_remote_daemon_info(state, target_name).await?,
    ))
}

pub(super) async fn endpoint_policy(
    state: &crate::BrokerState,
    endpoint: &TransferEndpoint,
) -> anyhow::Result<PathPolicy> {
    Ok(endpoint_target_context(state, &endpoint.target)
        .await?
        .policy())
}

pub(super) async fn ensure_absolute(
    state: &crate::BrokerState,
    endpoint: &TransferEndpoint,
) -> anyhow::Result<()> {
    let context = endpoint_target_context(state, &endpoint.target).await?;
    anyhow::ensure!(
        context.is_absolute_path(&endpoint.path),
        "transfer endpoint path `{}` is not absolute",
        endpoint.path
    );
    Ok(())
}

pub(super) async fn ensure_distinct_endpoints(
    state: &crate::BrokerState,
    source: &TransferEndpoint,
    destination: &TransferEndpoint,
) -> anyhow::Result<()> {
    if source.target != destination.target {
        return Ok(());
    }

    let policy = endpoint_policy(state, source).await?;
    anyhow::ensure!(
        !same_path_for_policy(policy, &source.path, &destination.path),
        "source and destination must differ"
    );
    Ok(())
}

pub(super) async fn ensure_multi_source_basenames_are_unique(
    state: &crate::BrokerState,
    sources: &[TransferEndpoint],
    destination: &TransferEndpoint,
) -> anyhow::Result<()> {
    if sources.len() <= 1 {
        return Ok(());
    }

    let destination_policy = endpoint_policy(state, destination).await?;
    let mut seen_paths: Vec<String> = Vec::with_capacity(sources.len());
    for source in sources {
        let source_policy = endpoint_policy(state, source).await?;
        let basename = basename_for_policy(source_policy, &source.path).ok_or_else(|| {
            anyhow::anyhow!(
                "transfer source path `{}` has no usable basename for multi-source transfer",
                source.path
            )
        })?;
        let candidate = join_for_policy(destination_policy, &destination.path, &basename);
        anyhow::ensure!(
            !seen_paths.iter().any(|existing| same_path_for_policy(
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
    sources: &[TransferEndpoint],
    destination: &TransferEndpoint,
    destination_mode: &TransferDestinationMode,
) -> anyhow::Result<TransferEndpoint> {
    let resolved_path = match destination_mode {
        TransferDestinationMode::Exact => destination.path.clone(),
        TransferDestinationMode::IntoDirectory => {
            resolve_into_directory_destination(state, sources, destination).await?
        }
        TransferDestinationMode::Auto => {
            let context = endpoint_target_context(state, &destination.target).await?;
            if sources.len() == 1
                && (path_looks_like_directory(context, &destination.path)
                    || existing_destination_is_directory(state, destination).await?)
            {
                resolve_into_directory_destination(state, sources, destination).await?
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

async fn resolve_into_directory_destination(
    state: &crate::BrokerState,
    sources: &[TransferEndpoint],
    destination: &TransferEndpoint,
) -> anyhow::Result<String> {
    let destination_context = endpoint_target_context(state, &destination.target).await?;
    let destination_policy = destination_context.policy();
    let mut candidates: Vec<String> = Vec::with_capacity(sources.len());
    for source in sources {
        let source_policy = endpoint_policy(state, source).await?;
        let basename = basename_for_policy(source_policy, &source.path).ok_or_else(|| {
            anyhow::anyhow!(
                "transfer source path `{}` has no usable basename for destination directory mode",
                source.path
            )
        })?;
        let candidate = join_child_for_context(destination_context, &destination.path, &basename);
        anyhow::ensure!(
            !candidates.iter().any(|existing| same_path_for_policy(
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

fn join_child_for_context(context: EndpointTargetContext, base: &str, child: &str) -> String {
    if matches!(
        context,
        EndpointTargetContext::Remote {
            accepts_single_slash_windows_absolute: true,
            ..
        }
    ) && base.starts_with('/')
        && !base.starts_with("//")
        && !is_absolute_for_policy(context.policy(), base)
    {
        let trimmed_base = base.trim_end_matches('/');
        if trimmed_base.is_empty() {
            format!("/{child}")
        } else {
            format!("{trimmed_base}/{child}")
        }
    } else {
        join_for_policy(context.policy(), base, child)
    }
}

fn path_looks_like_directory(context: EndpointTargetContext, path: &str) -> bool {
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
    let result = match destination.target.as_str() {
        "local" => crate::local_transfer::path_info(&destination.path, state.host_sandbox.as_ref()),
        target_name => {
            let target = verified_remote_target(state, target_name).await?;
            target
                .transfer_path_info(&TransferPathInfoRequest {
                    path: destination.path.clone(),
                })
                .await
        }
    };

    match result {
        Ok(info) => Ok(info.exists && info.is_directory),
        Err(err) if path_info_missing_or_unsupported(&err) => Ok(false),
        Err(err) => Err(normalize_path_info_error(err)),
    }
}

fn normalize_path_info_error(err: crate::daemon_client::DaemonClientError) -> anyhow::Error {
    match err {
        crate::daemon_client::DaemonClientError::Rpc { message, .. } => anyhow::Error::msg(message),
        other => other.into(),
    }
}

fn path_info_missing_or_unsupported(err: &crate::daemon_client::DaemonClientError) -> bool {
    match err {
        crate::daemon_client::DaemonClientError::Rpc { status, code, .. } => {
            *status == reqwest::StatusCode::NOT_FOUND
                || *status == reqwest::StatusCode::METHOD_NOT_ALLOWED
                || matches!(
                    code.as_deref(),
                    Some("not_found") | Some("unknown_endpoint")
                )
        }
        _ => false,
    }
}

pub(super) async fn negotiate_transfer_compression(
    state: &crate::BrokerState,
    sources: &[TransferEndpoint],
    destination: &TransferEndpoint,
) -> anyhow::Result<RpcTransferCompression> {
    if !state.enable_transfer_compression {
        return Ok(RpcTransferCompression::None);
    }

    let mut has_remote_endpoint = false;
    for endpoint in sources.iter().chain(std::iter::once(destination)) {
        let context = endpoint_target_context(state, &endpoint.target).await?;
        let Some(supports_transfer_compression) = context.supports_transfer_compression() else {
            continue;
        };

        has_remote_endpoint = true;
        if !supports_transfer_compression {
            return Ok(RpcTransferCompression::None);
        }
    }

    if has_remote_endpoint {
        Ok(RpcTransferCompression::Zstd)
    } else {
        Ok(RpcTransferCompression::None)
    }
}

fn local_policy() -> PathPolicy {
    if cfg!(windows) {
        windows_path_policy()
    } else {
        linux_path_policy()
    }
}

impl EndpointTargetContext {
    fn local() -> Self {
        Self::Local {
            policy: local_policy(),
        }
    }

    fn remote(info: crate::CachedDaemonInfo) -> Self {
        let accepts_single_slash_windows_absolute = info.platform.eq_ignore_ascii_case("windows");
        Self::Remote {
            policy: remote_policy(&info.platform),
            accepts_single_slash_windows_absolute,
            supports_transfer_compression: info.supports_transfer_compression,
        }
    }

    fn policy(self) -> PathPolicy {
        match self {
            Self::Local { policy } | Self::Remote { policy, .. } => policy,
        }
    }

    fn is_absolute_path(self, path: &str) -> bool {
        is_absolute_for_policy(self.policy(), path)
            || matches!(
                self,
                Self::Remote {
                    accepts_single_slash_windows_absolute: true,
                    ..
                } if path.starts_with('/') && !path.starts_with("//")
            )
    }

    fn supports_transfer_compression(self) -> Option<bool> {
        match self {
            Self::Local { .. } => None,
            Self::Remote {
                supports_transfer_compression,
                ..
            } => Some(supports_transfer_compression),
        }
    }
}

fn remote_policy(platform: &str) -> PathPolicy {
    if platform.eq_ignore_ascii_case("windows") {
        windows_path_policy()
    } else {
        linux_path_policy()
    }
}
