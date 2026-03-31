#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config_path = std::env::args().nth(1).expect("config path");
    let config = remote_exec_broker::config::BrokerConfig::load(config_path).await?;
    remote_exec_broker::run(config).await
}
