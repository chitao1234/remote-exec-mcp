use std::collections::BTreeMap;
use std::time::SystemTime;

use anyhow::Context;
use remote_exec_proto::rpc::TargetInfoResponse;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::{
    BrokerState, config,
    daemon_client::{DaemonClient, DaemonClientError},
    local,
    local::backend::LocalDaemonClient,
    port_forward,
    reverse_transport::ReverseTransportManager,
    session_store::SessionStore,
    state::{BrokerStateInit, TargetHealthRefreshIntervals},
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
    let target_refresh = PeriodicTargetRefreshTask::spawn(&state);
    tracing::info!(
        configured_targets = state.configured_target_count(),
        "broker ready"
    );
    let serve_result = crate::mcp_server::serve(state, &mcp).await;
    let refresh_result = target_refresh.shutdown().await;

    if let Err(err) = &refresh_result {
        if serve_result.is_err() {
            tracing::warn!(
                error = %err,
                "periodic target refresh task shutdown failed after broker serve error"
            );
        }
    }

    serve_result?;
    refresh_result
}

pub async fn build_state(config: config::ValidatedBrokerConfig) -> anyhow::Result<BrokerState> {
    let config = config.into_inner();
    let host_sandbox = compile_host_sandbox(&config)?;
    let reverse_transport = ReverseTransportManager::start(&config).await?;
    let mut targets = BTreeMap::new();

    insert_local_target(&config, &mut targets).await?;
    insert_remote_targets(&config.targets, reverse_transport.as_ref(), &mut targets).await?;

    Ok(BrokerState::new(BrokerStateInit {
        enable_transfer_compression: config.enable_transfer_compression,
        transfer_limits: config.transfer_limits,
        disable_structured_content: config.disable_structured_content,
        health_refresh_intervals: TargetHealthRefreshIntervals {
            healthy: config.health_refresh.healthy_interval(),
            unhealthy: config.health_refresh.unhealthy_interval(),
        },
        tools: config.tools,
        port_forward_limits: config.port_forward_limits,
        host_sandbox,
        host_filesystem: crate::state::BrokerHostFilesystemConfig::from_local_config(
            config.local.as_ref(),
        ),
        sessions: SessionStore::default(),
        port_forwards: port_forward::PortForwardStore::default(),
        targets,
        reverse_transport,
    }))
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
        local::TARGET_NAME.to_string(),
        TargetHandle::verified(
            TargetBackend::local(client),
            Some(local::TARGET_NAME.to_string()),
            &info,
        ),
    );
    Ok(())
}

async fn insert_remote_targets(
    target_configs: &BTreeMap<String, config::TargetConfig>,
    reverse_transport: Option<&ReverseTransportManager>,
    targets: &mut BTreeMap<String, TargetHandle>,
) -> anyhow::Result<()> {
    let probes = target_configs
        .iter()
        .map(|(name, target_config)| async move {
            (
                name.clone(),
                build_remote_target_handle(name, target_config, reverse_transport).await,
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
    reverse_transport: Option<&ReverseTransportManager>,
) -> anyhow::Result<TargetHandle> {
    let reverse_connection =
        reverse_transport.and_then(|transport| transport.target_connection(name));
    let client = DaemonClient::new(name.to_string(), target_config, reverse_connection).await?;
    match tokio::time::timeout(
        target_config.timeouts.startup_probe_timeout(),
        client.target_info(),
    )
    .await
    {
        Err(_) => {
            let _ = client
                .recover_connection_after_timeout("startup identity probe", None)
                .await;
            log_remote_target_startup_probe_timeout(name, target_config);
            Ok(TargetHandle::unavailable(
                TargetBackend::remote(client),
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
                TargetBackend::remote(client),
                target_config.expected_daemon_name.clone(),
                &info,
            ))
        }
        Ok(Err(DaemonClientError::Transport(err))) => {
            log_remote_target_unavailable(name, target_config, &err);
            Ok(TargetHandle::unavailable(
                TargetBackend::remote(client),
                target_config.expected_daemon_name.clone(),
            ))
        }
        Ok(Err(err)) => Err(err.into()),
    }
}

fn log_local_target_enabled(info: &TargetInfoResponse) {
    tracing::info!(
        target = local::TARGET_NAME,
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

struct PeriodicTargetRefreshTask {
    cancel: CancellationToken,
    handle: JoinHandle<()>,
}

impl PeriodicTargetRefreshTask {
    fn spawn(state: &BrokerState) -> Self {
        let cancel = CancellationToken::new();
        let handle = tokio::spawn(periodic_target_refresh_loop(state.clone(), cancel.clone()));
        Self { cancel, handle }
    }

    async fn shutdown(self) -> anyhow::Result<()> {
        self.cancel.cancel();
        self.handle
            .await
            .context("waiting for periodic target refresh task to stop")?;
        Ok(())
    }
}

async fn periodic_target_refresh_loop(state: BrokerState, cancel: CancellationToken) {
    let mut interval = tokio::time::interval(state.health_refresh_intervals.shortest());
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            _ = interval.tick() => {}
        }

        let names = state
            .remote_targets_due_for_health_refresh(SystemTime::now())
            .await;
        for name in names {
            if cancel.is_cancelled() {
                return;
            }

            let refresh =
                state.refresh_remote_target_health_and_dependents_with_configured_timeout(&name);
            let result = tokio::select! {
                biased;
                _ = cancel.cancelled() => return,
                result = refresh => result,
            };

            match result {
                Ok(()) => {}
                Err(err) => {
                    tracing::debug!(
                        target = %name,
                        error = %err,
                        "periodic target refresh did not update cached daemon metadata"
                    );
                }
            }
        }
    }

    tracing::debug!("periodic target refresh task stopped");
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::Duration;

    use tokio::io::AsyncReadExt;

    use crate::config::{
        BrokerConfig, BrokerHealthRefreshConfig, LocalTargetConfig, TargetConfig,
        TargetTimeoutConfig,
    };
    #[cfg(not(feature = "broker-tls"))]
    use remote_exec_test_support::test_helpers::DEFAULT_TEST_TARGET;

    use super::{PeriodicTargetRefreshTask, build_state};

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
                tools: Default::default(),
                port_forward_limits: Default::default(),
                health_refresh: Default::default(),
                reverse: None,
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

    async fn spawn_hung_http_server(delay: Duration) -> std::net::SocketAddr {
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
            let addr = spawn_hung_http_server(Duration::from_secs(5)).await;
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
                tools: Default::default(),
                port_forward_limits: Default::default(),
                health_refresh: Default::default(),
                reverse: None,
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
        assert_eq!(state.configured_target_count(), 4);
        for snapshot in state.target_status_snapshots().await {
            assert_eq!(snapshot.daemon_info, None);
        }
    }

    #[tokio::test]
    async fn build_state_uses_local_windows_posix_root_for_broker_host_filesystem() {
        let tempdir = tempfile::tempdir().unwrap();
        let windows_posix_root = tempdir.path().join("msys64");
        std::fs::create_dir(&windows_posix_root).unwrap();

        let state = build_state(
            BrokerConfig {
                mcp: Default::default(),
                host_sandbox: None,
                enable_transfer_compression: true,
                transfer_limits: remote_exec_proto::transfer::TransferLimits::default(),
                disable_structured_content: false,
                tools: Default::default(),
                port_forward_limits: Default::default(),
                health_refresh: Default::default(),
                reverse: None,
                targets: BTreeMap::new(),
                local: Some(LocalTargetConfig {
                    default_workdir: tempdir.path().to_path_buf(),
                    windows_posix_root: Some(windows_posix_root.clone()),
                    allow_login_shell: true,
                    pty: remote_exec_host::PtyMode::Auto,
                    default_shell: None,
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
        .unwrap();

        assert_eq!(
            state.host_filesystem.windows_posix_root(),
            Some(windows_posix_root.as_path())
        );
    }

    #[tokio::test]
    async fn periodic_target_refresh_task_stops_when_cancelled() {
        let state = build_state(
            BrokerConfig {
                mcp: Default::default(),
                host_sandbox: None,
                enable_transfer_compression: true,
                transfer_limits: remote_exec_proto::transfer::TransferLimits::default(),
                disable_structured_content: false,
                tools: Default::default(),
                port_forward_limits: Default::default(),
                health_refresh: BrokerHealthRefreshConfig {
                    healthy_interval_ms: 50,
                    unhealthy_interval_ms: 50,
                },
                targets: BTreeMap::new(),
                local: None,
                reverse: None,
            }
            .into_validated()
            .unwrap(),
        )
        .await
        .unwrap();

        let refresh = PeriodicTargetRefreshTask::spawn(&state);
        tokio::time::timeout(Duration::from_secs(1), refresh.shutdown())
            .await
            .expect("refresh task should stop promptly")
            .unwrap();
    }

    #[tokio::test]
    async fn periodic_target_refresh_shutdown_cancels_in_flight_health_probe() {
        let addr = spawn_hung_http_server(Duration::from_secs(30)).await;
        let mut targets = BTreeMap::new();
        targets.insert("refresh-target".to_string(), remote_http_target(addr, 200));

        let state = build_state(
            BrokerConfig {
                mcp: Default::default(),
                host_sandbox: None,
                enable_transfer_compression: true,
                transfer_limits: remote_exec_proto::transfer::TransferLimits::default(),
                disable_structured_content: false,
                tools: Default::default(),
                port_forward_limits: Default::default(),
                health_refresh: BrokerHealthRefreshConfig {
                    healthy_interval_ms: 1,
                    unhealthy_interval_ms: 1,
                },
                targets,
                local: None,
                reverse: None,
            }
            .into_validated()
            .unwrap(),
        )
        .await
        .unwrap();

        let refresh = PeriodicTargetRefreshTask::spawn(&state);
        tokio::time::sleep(Duration::from_millis(20)).await;

        let started = std::time::Instant::now();
        tokio::time::timeout(Duration::from_secs(1), refresh.shutdown())
            .await
            .expect("refresh shutdown should not wait for daemon request timeout")
            .unwrap();
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "refresh shutdown took too long: {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn reverse_http_target_recovers_after_daemon_restart() {
        use crate::config::{HttpAuthConfig, ReverseListenerConfig, ReverseTransport};
        use remote_exec_daemon::config::{
            DaemonConfig, DaemonConnectionMode, DaemonTransport, ReverseConnectionConfig,
        };

        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let reverse_addr = probe.local_addr().unwrap();
        drop(probe);
        let token = "reverse-test-secret".to_string();
        let auth = HttpAuthConfig {
            bearer_token: token.clone(),
        };
        let mut targets = BTreeMap::new();
        targets.insert(
            "reverse-target".to_string(),
            TargetConfig {
                base_url: "reverse://".to_string(),
                http_auth: Some(auth.clone()),
                timeouts: TargetTimeoutConfig {
                    startup_probe_ms: 50,
                    ..Default::default()
                },
                ca_pem: None,
                client_cert_pem: None,
                client_key_pem: None,
                allow_insecure_http: false,
                skip_server_name_verification: false,
                pinned_server_cert_pem: None,
                expected_daemon_name: Some("reverse-target".to_string()),
            },
        );
        let state = build_state(
            BrokerConfig {
                mcp: Default::default(),
                targets,
                local: None,
                host_sandbox: None,
                enable_transfer_compression: true,
                transfer_limits: Default::default(),
                disable_structured_content: false,
                tools: Default::default(),
                port_forward_limits: Default::default(),
                health_refresh: Default::default(),
                reverse: Some(ReverseListenerConfig {
                    listen: reverse_addr,
                    transport: ReverseTransport::Http,
                    allow_insecure_http: true,
                    tls: None,
                    registration_timeout_ms: 1_000,
                    lane_wait_timeout_ms: 1_000,
                    max_connections: 16,
                }),
            }
            .into_validated()
            .unwrap(),
        )
        .await
        .unwrap();

        let workdir = tempfile::tempdir().unwrap();
        let daemon = DaemonConfig {
            target: "reverse-target".to_string(),
            listen: "127.0.0.1:0".parse().unwrap(),
            connection_mode: DaemonConnectionMode::Reverse,
            reverse: Some(ReverseConnectionConfig {
                broker_addr: reverse_addr.to_string(),
                transport: DaemonTransport::Http,
                allow_insecure_http: true,
                bearer_token: Some(token),
                min_idle_connections: 2,
                max_connections: 8,
                reconnect_min_ms: 10,
                reconnect_max_ms: 100,
                registration_timeout_ms: 1_000,
                tls: None,
            }),
            default_workdir: workdir.path().to_path_buf(),
            windows_posix_root: None,
            transport: DaemonTransport::Http,
            http_auth: Some(auth),
            sandbox: None,
            enable_transfer_compression: true,
            transfer_limits: Default::default(),
            max_open_sessions: remote_exec_host::config::DEFAULT_MAX_OPEN_SESSIONS,
            allow_login_shell: true,
            pty: remote_exec_daemon::config::PtyMode::None,
            default_shell: None,
            yield_time: Default::default(),
            port_forward_limits: Default::default(),
            experimental_apply_patch_target_encoding_autodetect: false,
            process_environment: remote_exec_daemon::config::ProcessEnvironment::capture_current(),
            tls: None,
            request_timeout_ms: 30_000,
        }
        .into_validated()
        .unwrap();
        let cancel = tokio_util::sync::CancellationToken::new();
        let daemon_cancel = cancel.clone();
        let first_daemon = daemon.clone();
        let daemon_task = tokio::spawn(async move {
            remote_exec_daemon::run_until(first_daemon, daemon_cancel.cancelled()).await
        });

        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if state
                    .refresh_remote_target_health("reverse-target")
                    .await
                    .is_ok()
                    && state
                        .target_status_snapshots()
                        .await
                        .into_iter()
                        .any(|target| target.name == "reverse-target" && target.healthy)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("reverse target should become healthy");
        let first_daemon_instance_id = state
            .target_status_snapshots()
            .await
            .into_iter()
            .find(|target| target.name == "reverse-target")
            .and_then(|target| target.daemon_info)
            .expect("reverse target info should be cached")
            .daemon_instance_id;

        cancel.cancel();
        daemon_task.await.unwrap().unwrap();

        let restart_cancel = tokio_util::sync::CancellationToken::new();
        let restarted_daemon_cancel = restart_cancel.clone();
        let restarted_daemon_task = tokio::spawn(async move {
            remote_exec_daemon::run_until(daemon, restarted_daemon_cancel.cancelled()).await
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let refreshed = state
                    .refresh_remote_target_health_and_dependents("reverse-target")
                    .await
                    .is_ok();
                let restarted = state
                    .target_status_snapshots()
                    .await
                    .into_iter()
                    .find(|target| target.name == "reverse-target")
                    .is_some_and(|target| {
                        target.healthy
                            && target.daemon_info.is_some_and(|info| {
                                info.daemon_instance_id != first_daemon_instance_id
                            })
                    });
                if refreshed && restarted {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("reverse target should recover after daemon restart");
        restart_cancel.cancel();
        restarted_daemon_task.await.unwrap().unwrap();
    }

    #[cfg(feature = "broker-tls")]
    #[tokio::test]
    async fn reverse_tls_target_uses_role_specific_certificates() {
        use crate::config::{ReverseListenerConfig, ReverseListenerTlsConfig, ReverseTransport};
        use remote_exec_daemon::config::{
            DaemonConfig, DaemonConnectionMode, DaemonTransport, ReverseClientTlsConfig,
            ReverseConnectionConfig,
        };
        use remote_exec_pki::{DaemonCertSpec, DevInitSpec};

        let tempdir = tempfile::tempdir().unwrap();
        let bundle = remote_exec_pki::build_dev_init_bundle(&DevInitSpec {
            ca_common_name: "reverse-test-ca".to_string(),
            broker_common_name: "localhost".to_string(),
            daemon_specs: vec![DaemonCertSpec::localhost("reverse-tls")],
        })
        .unwrap();
        let manifest = remote_exec_pki::write_dev_init_bundle(
            &DevInitSpec {
                ca_common_name: "reverse-test-ca".to_string(),
                broker_common_name: "localhost".to_string(),
                daemon_specs: vec![DaemonCertSpec::localhost("reverse-tls")],
            },
            &bundle,
            tempdir.path(),
            true,
        )
        .unwrap();
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let reverse_addr = probe.local_addr().unwrap();
        drop(probe);
        let mut targets = BTreeMap::new();
        targets.insert(
            "reverse-tls".to_string(),
            TargetConfig {
                base_url: "reverse://".to_string(),
                http_auth: None,
                timeouts: TargetTimeoutConfig {
                    startup_probe_ms: 50,
                    ..Default::default()
                },
                ca_pem: None,
                client_cert_pem: None,
                client_key_pem: None,
                allow_insecure_http: false,
                skip_server_name_verification: false,
                pinned_server_cert_pem: None,
                expected_daemon_name: Some("reverse-tls".to_string()),
            },
        );
        let state = build_state(
            BrokerConfig {
                mcp: Default::default(),
                targets,
                local: None,
                host_sandbox: None,
                enable_transfer_compression: true,
                transfer_limits: Default::default(),
                disable_structured_content: false,
                tools: Default::default(),
                port_forward_limits: Default::default(),
                health_refresh: Default::default(),
                reverse: Some(ReverseListenerConfig {
                    listen: reverse_addr,
                    transport: ReverseTransport::Tls,
                    allow_insecure_http: false,
                    tls: Some(ReverseListenerTlsConfig {
                        cert_pem: manifest.reverse_broker.cert_pem.clone(),
                        key_pem: manifest.reverse_broker.key_pem.clone(),
                        ca_pem: manifest.ca.cert_pem.clone(),
                    }),
                    registration_timeout_ms: 2_000,
                    lane_wait_timeout_ms: 2_000,
                    max_connections: 16,
                }),
            }
            .into_validated()
            .unwrap(),
        )
        .await
        .unwrap();

        let reverse_daemon = &manifest.reverse_daemons["reverse-tls"];
        let daemon = DaemonConfig {
            target: "reverse-tls".to_string(),
            listen: "127.0.0.1:0".parse().unwrap(),
            connection_mode: DaemonConnectionMode::Reverse,
            reverse: Some(ReverseConnectionConfig {
                broker_addr: reverse_addr.to_string(),
                transport: DaemonTransport::Tls,
                allow_insecure_http: false,
                bearer_token: None,
                min_idle_connections: 2,
                max_connections: 8,
                reconnect_min_ms: 10,
                reconnect_max_ms: 100,
                registration_timeout_ms: 2_000,
                tls: Some(ReverseClientTlsConfig {
                    cert_pem: reverse_daemon.cert_pem.clone(),
                    key_pem: reverse_daemon.key_pem.clone(),
                    ca_pem: manifest.ca.cert_pem.clone(),
                    server_name: "localhost".to_string(),
                    pinned_server_cert_pem: None,
                }),
            }),
            default_workdir: tempdir.path().to_path_buf(),
            windows_posix_root: None,
            transport: DaemonTransport::Http,
            http_auth: None,
            sandbox: None,
            enable_transfer_compression: true,
            transfer_limits: Default::default(),
            max_open_sessions: remote_exec_host::config::DEFAULT_MAX_OPEN_SESSIONS,
            allow_login_shell: true,
            pty: remote_exec_daemon::config::PtyMode::None,
            default_shell: None,
            yield_time: Default::default(),
            port_forward_limits: Default::default(),
            experimental_apply_patch_target_encoding_autodetect: false,
            process_environment: remote_exec_daemon::config::ProcessEnvironment::capture_current(),
            tls: None,
            request_timeout_ms: 30_000,
        }
        .into_validated()
        .unwrap();
        let cancel = tokio_util::sync::CancellationToken::new();
        let daemon_cancel = cancel.clone();
        let daemon_task = tokio::spawn(async move {
            remote_exec_daemon::run_until(daemon, daemon_cancel.cancelled()).await
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if state
                    .refresh_remote_target_health("reverse-tls")
                    .await
                    .is_ok()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("reverse TLS target should become healthy");
        cancel.cancel();
        daemon_task.await.unwrap().unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cpp_reverse_http_target_connects_after_broker_startup() {
        use crate::config::{HttpAuthConfig, ReverseListenerConfig, ReverseTransport};

        let binary = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../remote-exec-daemon-cpp/build/remote-exec-daemon-cpp");
        if !binary.exists() {
            return;
        }
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let reverse_addr = probe.local_addr().unwrap();
        drop(probe);
        let token = "cpp-reverse-secret".to_string();
        let auth = HttpAuthConfig {
            bearer_token: token.clone(),
        };
        let mut targets = BTreeMap::new();
        targets.insert(
            "cpp-reverse".to_string(),
            TargetConfig {
                base_url: "reverse://".to_string(),
                http_auth: Some(auth),
                timeouts: TargetTimeoutConfig {
                    startup_probe_ms: 50,
                    ..Default::default()
                },
                ca_pem: None,
                client_cert_pem: None,
                client_key_pem: None,
                allow_insecure_http: false,
                skip_server_name_verification: false,
                pinned_server_cert_pem: None,
                expected_daemon_name: Some("cpp-reverse".to_string()),
            },
        );
        let state = build_state(
            BrokerConfig {
                mcp: Default::default(),
                targets,
                local: None,
                host_sandbox: None,
                enable_transfer_compression: true,
                transfer_limits: Default::default(),
                disable_structured_content: false,
                tools: Default::default(),
                port_forward_limits: Default::default(),
                health_refresh: Default::default(),
                reverse: Some(ReverseListenerConfig {
                    listen: reverse_addr,
                    transport: ReverseTransport::Http,
                    allow_insecure_http: true,
                    tls: None,
                    registration_timeout_ms: 1_000,
                    lane_wait_timeout_ms: 1_000,
                    max_connections: 16,
                }),
            }
            .into_validated()
            .unwrap(),
        )
        .await
        .unwrap();

        let tempdir = tempfile::tempdir().unwrap();
        let config_path = tempdir.path().join("daemon.ini");
        std::fs::write(
            &config_path,
            format!(
                "target=cpp-reverse\nconnection_mode=reverse\nreverse_broker_host=127.0.0.1\nreverse_broker_port={}\nreverse_bearer_token={}\nreverse_min_idle_connections=2\nreverse_max_connections=8\nreverse_reconnect_ms=20\ndefault_workdir={}\nhttp_auth_bearer_token={}\n",
                reverse_addr.port(),
                token,
                tempdir.path().display(),
                token,
            ),
        )
        .unwrap();
        let mut child = tokio::process::Command::new(binary)
            .arg(&config_path)
            .env("REMOTE_EXEC_LOG", "off")
            .kill_on_drop(true)
            .spawn()
            .unwrap();

        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if state
                    .refresh_remote_target_health("cpp-reverse")
                    .await
                    .is_ok()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("C++ reverse target should become healthy");
        child.kill().await.unwrap();
        let _ = child.wait().await;
    }

    #[cfg(not(feature = "broker-tls"))]
    #[tokio::test]
    async fn build_state_rejects_https_targets_when_broker_tls_feature_disabled() {
        let mut targets = BTreeMap::new();
        targets.insert(
            DEFAULT_TEST_TARGET.to_string(),
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
                tools: Default::default(),
                port_forward_limits: Default::default(),
                health_refresh: Default::default(),
                reverse: None,
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
