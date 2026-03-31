#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("info").init();

    let config_path = std::env::args().nth(1).expect("config path");
    let config = remote_exec_daemon::config::DaemonConfig::load(config_path).await?;
    remote_exec_daemon::run(config).await
}
