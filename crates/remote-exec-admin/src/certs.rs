use crate::cli::{CertsArgs, CertsCommand};

pub fn run(args: CertsArgs) -> anyhow::Result<()> {
    match args.command {
        CertsCommand::DevInit(_) => anyhow::bail!("cert generation is not implemented yet"),
    }
}
