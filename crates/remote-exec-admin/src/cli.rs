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
}

#[derive(Args, Debug, Clone)]
pub struct DevInitArgs {
    #[arg(long)]
    pub out_dir: PathBuf,

    #[arg(long = "target", required = true)]
    pub targets: Vec<String>,

    #[arg(long = "daemon-san")]
    pub daemon_sans: Vec<String>,

    #[arg(long, default_value = "remote-exec-broker")]
    pub broker_common_name: String,

    #[arg(long, default_value_t = false)]
    pub force: bool,
}
