#[tokio::main]
async fn main() -> anyhow::Result<()> {
    remote_exec_daemon::logging::init_logging();

    let config_path = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: remote-exec-daemon <config-path>"))?;
    let config = remote_exec_daemon::config::DaemonConfig::load(config_path).await?;
    remote_exec_daemon::run(config).await
}
