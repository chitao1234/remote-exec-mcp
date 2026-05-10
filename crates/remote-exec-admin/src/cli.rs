use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "remote-exec-admin")]
#[command(about = "Administrative tooling for remote-exec-mcp")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    Certs(CertsArgs),
}

#[derive(Args, Debug)]
pub struct CertsArgs {
    #[command(subcommand)]
    pub command: CertsCommand,
}

#[derive(Subcommand, Debug)]
pub enum CertsCommand {
    DevInit(DevInitArgs),
    InitCa(InitCaArgs),
    IssueBroker(IssueBrokerArgs),
    IssueDaemon(IssueDaemonArgs),
}

#[derive(Args, Debug, Clone)]
pub struct DevInitArgs {
    #[arg(long)]
    pub out_dir: PathBuf,

    #[arg(long = "target", required = true)]
    pub targets: Vec<String>,

    #[arg(long = "san", visible_alias = "daemon-san")]
    pub daemon_sans: Vec<String>,

    #[arg(long, default_value = "remote-exec-broker")]
    pub broker_common_name: String,

    #[arg(long, default_value_t = false)]
    pub force: bool,

    #[command(flatten)]
    pub reuse_ca: ReuseCaArgs,
}

#[derive(Args, Debug, Clone, Default)]
pub struct ReuseCaArgs {
    #[arg(long)]
    pub reuse_ca_cert_pem: Option<PathBuf>,

    #[arg(long)]
    pub reuse_ca_key_pem: Option<PathBuf>,

    #[arg(long)]
    pub reuse_ca_from_dir: Option<PathBuf>,
}

#[derive(Args, Debug, Clone)]
pub struct InitCaArgs {
    #[arg(long)]
    pub out_dir: PathBuf,

    #[arg(long, default_value = "remote-exec-ca")]
    pub ca_common_name: String,

    #[arg(long, default_value_t = false)]
    pub force: bool,
}

#[derive(Args, Debug, Clone)]
pub struct IssueBrokerArgs {
    #[arg(long)]
    pub out_dir: PathBuf,

    #[arg(long)]
    pub ca_cert_pem: PathBuf,

    #[arg(long)]
    pub ca_key_pem: PathBuf,

    #[arg(long, default_value = "remote-exec-broker")]
    pub broker_common_name: String,

    #[arg(long, default_value_t = false)]
    pub force: bool,
}

#[derive(Args, Debug, Clone)]
pub struct IssueDaemonArgs {
    #[arg(long)]
    pub out_dir: PathBuf,

    #[arg(long)]
    pub ca_cert_pem: PathBuf,

    #[arg(long)]
    pub ca_key_pem: PathBuf,

    #[arg(long)]
    pub target: String,

    #[arg(long = "san")]
    pub sans: Vec<String>,

    #[arg(long, default_value_t = false)]
    pub force: bool,
}
