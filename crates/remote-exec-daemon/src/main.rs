#[tokio::main]
async fn main() -> anyhow::Result<()> {
    remote_exec_daemon::logging::init_logging();

    let config_path = std::env::args().nth(1).expect("config path");
    let config = remote_exec_daemon::config::DaemonConfig::load(config_path).await?;
    remote_exec_daemon::run(config).await
}
