use std::collections::BTreeMap;

use remote_exec_proto::rpc::TargetInfoResponse;

use crate::{
    BrokerState, config,
    daemon_client::{DaemonClient, DaemonClientError},
    local::backend::LocalDaemonClient,
    port_forward,
    session_store::SessionStore,
    state::LOCAL_TARGET_NAME,
    target::{TargetBackend, TargetHandle, ensure_expected_daemon_name},
};

pub async fn run(config: config::ValidatedBrokerConfig) -> anyhow::Result<()> {
    crate::install_crypto_provider()?;
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

pub async fn build_state(config: config::ValidatedBrokerConfig) -> anyhow::Result<BrokerState> {
    let config = config.into_inner();
    let host_sandbox = compile_host_sandbox(&config)?;
    let mut targets = BTreeMap::new();

    insert_local_target(&config, &mut targets).await?;
    insert_remote_targets(&config.targets, &mut targets).await?;

    Ok(BrokerState {
        enable_transfer_compression: config.enable_transfer_compression,
        transfer_limits: config.transfer_limits,
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
) -> anyhow::Result<Option<remote_exec_host::sandbox::CompiledFilesystemSandbox>> {
    Ok(config
        .host_sandbox
        .as_ref()
        .map(remote_exec_host::sandbox::compile_filesystem_sandbox)
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
        LOCAL_TARGET_NAME.to_string(),
        TargetHandle::verified(
            TargetBackend::Local(client),
            Some(LOCAL_TARGET_NAME.to_string()),
            &info,
        ),
    );
    Ok(())
}

async fn insert_remote_targets(
    target_configs: &BTreeMap<String, config::TargetConfig>,
    targets: &mut BTreeMap<String, TargetHandle>,
) -> anyhow::Result<()> {
    let probes = target_configs
        .iter()
        .map(|(name, target_config)| async move {
            (
                name.clone(),
                build_remote_target_handle(name, target_config).await,
            )
        });

    for (name, handle) in futures_util::future::join_all(probes).await {
        let handle = handle?;
        targets.insert(name.clone(), handle);
    }
    Ok(())
}

async fn build_remote_target_handle(
    name: &str,
    target_config: &config::TargetConfig,
) -> anyhow::Result<TargetHandle> {
    let client = DaemonClient::new(name.to_string(), target_config).await?;
    match tokio::time::timeout(
        target_config.timeouts.startup_probe_timeout(),
        client.target_info(),
    )
    .await
    {
        Err(_) => {
            log_remote_target_startup_probe_timeout(name, target_config);
            Ok(TargetHandle::unavailable(
                TargetBackend::Remote(client),
                target_config.expected_daemon_name.clone(),
            ))
        }
        Ok(Ok(info)) => {
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
        Ok(Err(DaemonClientError::Transport(err))) => {
            log_remote_target_unavailable(name, target_config, &err);
            Ok(TargetHandle::unavailable(
                TargetBackend::Remote(client),
                target_config.expected_daemon_name.clone(),
            ))
        }
        Ok(Err(err)) => Err(err.into()),
    }
}

fn log_local_target_enabled(info: &TargetInfoResponse) {
    tracing::info!(
        target = LOCAL_TARGET_NAME,
        daemon_instance_id = %info.daemon_instance_id,
        platform = %info.identity.platform,
        arch = %info.identity.arch,
        hostname = %info.identity.hostname,
        supports_pty = info.capabilities.supports_pty,
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
        platform = %info.identity.platform,
        arch = %info.identity.arch,
        hostname = %info.identity.hostname,
        supports_pty = info.capabilities.supports_pty,
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
        base_url = %target_config.base_url,
        http_auth_enabled = target_config.http_auth.is_some(),
        ?err,
        "target unavailable during broker startup"
    );
}

fn log_remote_target_startup_probe_timeout(name: &str, target_config: &config::TargetConfig) {
    tracing::warn!(
        target = %name,
        base_url = %target_config.base_url,
        http_auth_enabled = target_config.http_auth.is_some(),
        timeout_ms = target_config.timeouts.startup_probe_ms,
        "target unavailable during broker startup: startup probe timed out"
    );
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
    use std::time::Duration;

    use tokio::io::AsyncReadExt;

    use crate::config::{BrokerConfig, LocalTargetConfig, TargetConfig, TargetTimeoutConfig};

    use super::build_state;

    #[tokio::test]
    async fn build_state_rejects_unusable_local_default_shell() {
        let tempdir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        let missing_shell = "/definitely/missing/remote-exec-shell";
        #[cfg(windows)]
        let missing_shell = r"C:\definitely\missing\remote-exec-shell.exe";

        let err = match build_state(
            BrokerConfig {
                mcp: Default::default(),
                host_sandbox: None,
                enable_transfer_compression: true,
                transfer_limits: remote_exec_proto::transfer::TransferLimits::default(),
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
                    transfer_limits: remote_exec_proto::transfer::TransferLimits::default(),
                    port_forward_limits: remote_exec_host::HostPortForwardLimits::default(),
                    experimental_apply_patch_target_encoding_autodetect: false,
                }),
            }
            .into_validated()
            .unwrap(),
        )
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

    async fn spawn_hung_target_info_server(delay: Duration) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = match listener.accept().await {
                    Ok(value) => value,
                    Err(_) => return,
                };
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let _ = stream.read(&mut buf).await;
                    tokio::time::sleep(delay).await;
                });
            }
        });
        addr
    }

    fn remote_http_target(addr: std::net::SocketAddr, startup_probe_ms: u64) -> TargetConfig {
        TargetConfig {
            base_url: format!("http://{addr}"),
            http_auth: None,
            timeouts: TargetTimeoutConfig {
                startup_probe_ms,
                request_ms: 5_000,
                ..TargetTimeoutConfig::default()
            },
            ca_pem: None,
            client_cert_pem: None,
            client_key_pem: None,
            allow_insecure_http: true,
            skip_server_name_verification: false,
            pinned_server_cert_pem: None,
            expected_daemon_name: None,
        }
    }

    #[tokio::test]
    async fn remote_startup_probes_are_parallel_and_bounded() {
        let mut targets = BTreeMap::new();
        for index in 0..4 {
            let addr = spawn_hung_target_info_server(Duration::from_secs(5)).await;
            targets.insert(format!("slow-{index}"), remote_http_target(addr, 400));
        }

        let started = std::time::Instant::now();
        let state = build_state(
            BrokerConfig {
                mcp: Default::default(),
                host_sandbox: None,
                enable_transfer_compression: true,
                transfer_limits: remote_exec_proto::transfer::TransferLimits::default(),
                disable_structured_content: false,
                port_forward_limits: Default::default(),
                targets,
                local: None,
            }
            .into_validated()
            .unwrap(),
        )
        .await
        .unwrap();

        assert!(
            started.elapsed() < Duration::from_millis(1_200),
            "startup probes did not run concurrently: {:?}",
            started.elapsed()
        );
        assert_eq!(state.targets.len(), 4);
        for handle in state.targets.values() {
            assert_eq!(handle.cached_daemon_info().await, None);
        }
    }

    #[cfg(not(feature = "broker-tls"))]
    #[tokio::test]
    async fn build_state_rejects_https_targets_when_broker_tls_feature_disabled() {
        let mut targets = BTreeMap::new();
        targets.insert(
            "builder-a".to_string(),
            TargetConfig {
                base_url: "https://127.0.0.1:9443".to_string(),
                http_auth: None,
                timeouts: TargetTimeoutConfig::default(),
                ca_pem: Some("/tmp/ca.pem".into()),
                client_cert_pem: Some("/tmp/broker.pem".into()),
                client_key_pem: Some("/tmp/broker.key".into()),
                allow_insecure_http: false,
                skip_server_name_verification: false,
                pinned_server_cert_pem: None,
                expected_daemon_name: None,
            },
        );

        let err = match build_state(
            BrokerConfig {
                mcp: Default::default(),
                host_sandbox: None,
                enable_transfer_compression: true,
                transfer_limits: remote_exec_proto::transfer::TransferLimits::default(),
                disable_structured_content: false,
                port_forward_limits: Default::default(),
                targets,
                local: None,
            }
            .into_validated()
            .unwrap(),
        )
        .await
        {
            Ok(_) => panic!("expected HTTPS target construction to fail without broker-tls"),
            Err(err) => err,
        };

        assert!(
            err.to_string().contains(
                "https:// support requires the remote-exec-broker `broker-tls` Cargo feature"
            ),
            "unexpected error: {err}"
        );
    }
}
