use clap::Parser;

mod bootstrap;
mod certs;
mod cli;

fn main() -> anyhow::Result<()> {
    let cli = cli::Cli::parse();
    match cli.command {
        cli::Commands::Certs(args) => certs::run(args),
    }
}
