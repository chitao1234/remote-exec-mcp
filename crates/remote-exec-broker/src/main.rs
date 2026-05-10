#[tokio::main]
async fn main() -> anyhow::Result<()> {
    remote_exec_broker::logging::init_logging();

    let config_path = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: remote-exec-broker <config-path>"))?;
    let config = remote_exec_broker::config::BrokerConfig::load(config_path).await?;
    remote_exec_broker::run(config).await
}
