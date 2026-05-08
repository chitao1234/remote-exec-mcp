use std::collections::BTreeMap;

use remote_exec_proto::{
    path::{PathPolicy, linux_path_policy, windows_path_policy},
    rpc::TargetInfoResponse,
    sandbox::{CompiledFilesystemSandbox, compile_filesystem_sandbox},
};

use crate::{
    BrokerState, config,
    daemon_client::{DaemonClient, DaemonClientError},
    local_backend::LocalDaemonClient,
    port_forward,
    session_store::SessionStore,
    target::{TargetBackend, TargetHandle, ensure_expected_daemon_name},
};

pub async fn run(config: config::BrokerConfig) -> anyhow::Result<()> {
    crate::install_crypto_provider();
    let mcp = config.mcp.clone();
    tracing::info!(
        configured_targets = config.targets.len(),
        local_target_enabled = config.local.is_some(),
        disable_structured_content = config.disable_structured_content,
        mcp_transport = mcp_transport_name(&mcp),
        "starting broker"
    );
    let state = build_state(config).await?;
    tracing::info!(configured_targets = state.targets.len(), "broker ready");
    crate::mcp_server::serve(state, &mcp).await
}

pub async fn build_state(mut config: config::BrokerConfig) -> anyhow::Result<BrokerState> {
    config.normalize_paths();
    config.validate()?;
    let host_sandbox = compile_host_sandbox(&config)?;
    let mut targets = BTreeMap::new();

    insert_local_target(&config, &mut targets).await?;
    insert_remote_targets(&config.targets, &mut targets).await?;

    Ok(BrokerState {
        enable_transfer_compression: config.enable_transfer_compression,
        disable_structured_content: config.disable_structured_content,
        port_forward_limits: config.port_forward_limits,
        host_sandbox,
        sessions: SessionStore::default(),
        port_forwards: port_forward::PortForwardStore::default(),
        targets,
    })
}

fn compile_host_sandbox(
    config: &config::BrokerConfig,
) -> anyhow::Result<Option<CompiledFilesystemSandbox>> {
    Ok(config
        .host_sandbox
        .as_ref()
        .map(|sandbox| compile_filesystem_sandbox(host_path_policy(), sandbox))
        .transpose()?)
}

async fn insert_local_target(
    config: &config::BrokerConfig,
    targets: &mut BTreeMap<String, TargetHandle>,
) -> anyhow::Result<()> {
    let Some(local_config) = &config.local else {
        return Ok(());
    };

    let client = LocalDaemonClient::new(
        local_config,
        config.host_sandbox.clone(),
        config.enable_transfer_compression,
    )?;
    let info = client.target_info().await?;
    log_local_target_enabled(&info);
    targets.insert(
        "local".to_string(),
        TargetHandle::verified(
            TargetBackend::Local(client),
            Some("local".to_string()),
            &info,
        ),
    );
    Ok(())
}

async fn insert_remote_targets(
    target_configs: &BTreeMap<String, config::TargetConfig>,
    targets: &mut BTreeMap<String, TargetHandle>,
) -> anyhow::Result<()> {
    for (name, target_config) in target_configs {
        let handle = build_remote_target_handle(name, target_config).await?;
        targets.insert(name.clone(), handle);
    }
    Ok(())
}

async fn build_remote_target_handle(
    name: &str,
    target_config: &config::TargetConfig,
) -> anyhow::Result<TargetHandle> {
    let client = DaemonClient::new(name.to_string(), target_config).await?;
    match client.target_info().await {
        Ok(info) => {
            ensure_expected_daemon_name(
                name,
                target_config.expected_daemon_name.as_deref(),
                &info.target,
            )?;
            log_remote_target_available(name, target_config, &info);
            Ok(TargetHandle::verified(
                TargetBackend::Remote(client),
                target_config.expected_daemon_name.clone(),
                &info,
            ))
        }
        Err(DaemonClientError::Transport(err)) => {
            log_remote_target_unavailable(name, target_config, &err);
            Ok(TargetHandle::unavailable(
                TargetBackend::Remote(client),
                target_config.expected_daemon_name.clone(),
            ))
        }
        Err(err) => Err(err.into()),
    }
}

fn log_local_target_enabled(info: &TargetInfoResponse) {
    tracing::info!(
        target = "local",
        daemon_instance_id = %info.daemon_instance_id,
        platform = %info.platform,
        arch = %info.arch,
        hostname = %info.hostname,
        supports_pty = info.supports_pty,
        supports_transfer_compression = info.supports_transfer_compression,
        "enabled embedded local target"
    );
}

fn log_remote_target_available(
    name: &str,
    target_config: &config::TargetConfig,
    info: &TargetInfoResponse,
) {
    tracing::info!(
        target = %name,
        base_url = %target_config.base_url,
        http_auth_enabled = target_config.http_auth.is_some(),
        daemon_name = %info.target,
        daemon_instance_id = %info.daemon_instance_id,
        platform = %info.platform,
        arch = %info.arch,
        hostname = %info.hostname,
        supports_pty = info.supports_pty,
        supports_transfer_compression = info.supports_transfer_compression,
        "target available during broker startup"
    );
}

fn log_remote_target_unavailable(
    name: &str,
    target_config: &config::TargetConfig,
    err: &anyhow::Error,
) {
    tracing::warn!(
        target = %name,
        http_auth_enabled = target_config.http_auth.is_some(),
        ?err,
        "target unavailable during broker startup"
    );
}

fn host_path_policy() -> PathPolicy {
    if cfg!(windows) {
        windows_path_policy()
    } else {
        linux_path_policy()
    }
}

fn mcp_transport_name(config: &config::McpServerConfig) -> &'static str {
    match config {
        config::McpServerConfig::Stdio => "stdio",
        config::McpServerConfig::StreamableHttp { .. } => "streamable_http",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::config::{BrokerConfig, LocalTargetConfig};

    use super::build_state;

    #[tokio::test]
    async fn build_state_rejects_unusable_local_default_shell() {
        let tempdir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        let missing_shell = "/definitely/missing/remote-exec-shell";
        #[cfg(windows)]
        let missing_shell = r"C:\definitely\missing\remote-exec-shell.exe";

        let err = match build_state(BrokerConfig {
            mcp: Default::default(),
            host_sandbox: None,
            enable_transfer_compression: true,
            disable_structured_content: false,
            port_forward_limits: Default::default(),
            targets: BTreeMap::new(),
            local: Some(LocalTargetConfig {
                default_workdir: tempdir.path().to_path_buf(),
                windows_posix_root: None,
                allow_login_shell: true,
                pty: remote_exec_host::PtyMode::Auto,
                default_shell: Some(missing_shell.to_string()),
                yield_time: remote_exec_host::YieldTimeConfig::default(),
                port_forward_limits: remote_exec_host::HostPortForwardLimits::default(),
                experimental_apply_patch_target_encoding_autodetect: false,
            }),
        })
        .await
        {
            Ok(_) => panic!("expected local default shell validation to fail"),
            Err(err) => err,
        };

        assert!(
            err.to_string().contains("not found") || err.to_string().contains("usable"),
            "unexpected error: {err}"
        );
    }
}
