#[tokio::main]
async fn main() -> anyhow::Result<()> {
    remote_exec_daemon::logging::init_logging();

    let config_path = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: remote-exec-daemon <config-path>"))?;
    let config = remote_exec_daemon::config::DaemonConfig::load(config_path).await?;
    remote_exec_daemon::run_until(config, wait_for_shutdown_signal()).await
}

async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        match signal(SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = terminate.recv() => {}
                }
            }
            Err(err) => {
                tracing::warn!(
                    ?err,
                    "failed to install SIGTERM handler; falling back to ctrl-c"
                );
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }

    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
