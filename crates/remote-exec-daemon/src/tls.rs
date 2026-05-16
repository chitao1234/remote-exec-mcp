use std::sync::Arc;

use axum::Router;

use crate::config::{DaemonConfig, DaemonTransport};

#[allow(
    dead_code,
    reason = "Referenced when TLS feature-gated paths reject configuration"
)]
pub(crate) const FEATURE_REQUIRED_MESSAGE: &str =
    "transport = \"tls\" requires the remote-exec-daemon `tls` Cargo feature";

#[cfg(feature = "tls")]
#[path = "tls_enabled.rs"]
mod tls_impl;

#[cfg(not(feature = "tls"))]
#[path = "tls_disabled.rs"]
mod tls_impl;

pub(crate) use tls_impl::{install_crypto_provider, serve_tls_with_shutdown_on_listener};
pub use tls_impl::{serve_tls, serve_tls_with_shutdown};

pub(crate) fn validate_config(config: &DaemonConfig) -> anyhow::Result<()> {
    if matches!(config.transport, DaemonTransport::Http)
        && config
            .tls
            .as_ref()
            .is_some_and(|tls| tls.pinned_client_cert_pem.is_some())
    {
        anyhow::bail!("pinned_client_cert_pem requires transport = \"tls\"");
    }

    tls_impl::validate_config(config)
}

pub async fn serve(app: Router, daemon_config: Arc<DaemonConfig>) -> anyhow::Result<()> {
    serve_with_shutdown(app, daemon_config, std::future::pending::<()>()).await
}

pub async fn serve_with_shutdown(
    app: Router,
    daemon_config: Arc<DaemonConfig>,
    shutdown: impl std::future::Future<Output = ()> + Send,
) -> anyhow::Result<()> {
    let listener = crate::server_transport::bind_listener(daemon_config.listen)?;
    crate::server_transport::serve_with_shutdown_on_listener(app, daemon_config, listener, shutdown)
        .await
}

pub async fn serve_http(app: Router, daemon_config: Arc<DaemonConfig>) -> anyhow::Result<()> {
    serve_http_with_shutdown(app, daemon_config, std::future::pending::<()>()).await
}

pub async fn serve_http_with_shutdown(
    app: Router,
    daemon_config: Arc<DaemonConfig>,
    shutdown: impl std::future::Future<Output = ()> + Send,
) -> anyhow::Result<()> {
    let listener = crate::server_transport::bind_listener(daemon_config.listen)?;
    crate::server_transport::serve_http_with_shutdown_on_listener(
        app,
        daemon_config,
        listener,
        shutdown,
    )
    .await
}

#[cfg(test)]
mod tests {
    #[cfg(not(feature = "tls"))]
    mod tls_disabled {
        use std::path::PathBuf;
        use std::sync::Arc;

        use axum::Router;

        use crate::config::{
            DaemonConfig, DaemonTransport, ProcessEnvironment, PtyMode, YieldTimeConfig,
        };

        fn tls_transport_config() -> Arc<DaemonConfig> {
            Arc::new(DaemonConfig {
                target: "builder-a".to_string(),
                listen: "127.0.0.1:9443".parse().unwrap(),
                default_workdir: PathBuf::from("."),
                windows_posix_root: None,
                transport: DaemonTransport::Tls,
                http_auth: None,
                sandbox: None,
                enable_transfer_compression: true,
                transfer_limits: remote_exec_proto::transfer::TransferLimits::default(),
                max_open_sessions: remote_exec_host::config::DEFAULT_MAX_OPEN_SESSIONS,
                allow_login_shell: true,
                pty: PtyMode::Auto,
                default_shell: None,
                yield_time: YieldTimeConfig::default(),
                port_forward_limits: crate::config::HostPortForwardLimits::default(),
                experimental_apply_patch_target_encoding_autodetect: false,
                process_environment: ProcessEnvironment::capture_current(),
                tls: None,
            })
        }

        #[tokio::test]
        async fn serve_tls_is_rejected_when_feature_disabled() {
            let err = super::super::serve_tls(Router::new(), tls_transport_config())
                .await
                .unwrap_err();
            assert!(
                err.to_string()
                    .contains(super::super::FEATURE_REQUIRED_MESSAGE),
                "unexpected error: {err}",
            );
        }

        #[tokio::test]
        async fn serve_tls_with_shutdown_is_rejected_when_feature_disabled() {
            let err = super::super::serve_tls_with_shutdown(
                Router::new(),
                tls_transport_config(),
                async {},
            )
            .await
            .unwrap_err();
            assert!(
                err.to_string()
                    .contains(super::super::FEATURE_REQUIRED_MESSAGE),
                "unexpected error: {err}",
            );
        }
    }
}
