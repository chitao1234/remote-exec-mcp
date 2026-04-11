use anyhow::Context;
use remote_exec_proto::path::{
    PathPolicy, basename_for_policy, is_absolute_for_policy, join_for_policy, linux_path_policy,
    same_path_for_policy, windows_path_policy,
};
use remote_exec_proto::public::TransferEndpoint;
use remote_exec_proto::rpc::TransferCompression as RpcTransferCompression;

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

pub(super) async fn endpoint_policy(
    state: &crate::BrokerState,
    endpoint: &TransferEndpoint,
) -> anyhow::Result<PathPolicy> {
    if endpoint.target == "local" {
        return Ok(local_policy());
    }

    let info = verified_remote_daemon_info(state, &endpoint.target).await?;
    Ok(remote_policy(&info.platform))
}

pub(super) async fn ensure_absolute(
    state: &crate::BrokerState,
    endpoint: &TransferEndpoint,
) -> anyhow::Result<()> {
    if endpoint.target == "local" {
        let policy = local_policy();
        anyhow::ensure!(
            is_absolute_for_policy(policy, &endpoint.path),
            "transfer endpoint path `{}` is not absolute",
            endpoint.path
        );
        return Ok(());
    }

    let info = verified_remote_daemon_info(state, &endpoint.target).await?;
    let policy = remote_policy(&info.platform);
    anyhow::ensure!(
        is_absolute_for_policy(policy, &endpoint.path)
            || (info.platform.eq_ignore_ascii_case("windows")
                && endpoint.path.starts_with('/')
                && !endpoint.path.starts_with("//")),
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
        if endpoint.target == "local" {
            continue;
        }

        has_remote_endpoint = true;
        let info = verified_remote_daemon_info(state, &endpoint.target).await?;
        if !info.supports_transfer_compression {
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

fn remote_policy(platform: &str) -> PathPolicy {
    if platform.eq_ignore_ascii_case("windows") {
        windows_path_policy()
    } else {
        linux_path_policy()
    }
}
