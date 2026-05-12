use remote_exec_broker::{BrokerConfig, init_logging, run};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_logging();

    let config_path = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: remote-exec-broker <config-path>"))?;
    let config = BrokerConfig::load(config_path).await?;
    run(config).await
}
