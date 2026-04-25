use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "xtask")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Placeholder for future build/release tasks.
    Noop,
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().cmd {
        Cmd::Noop => Ok(()),
    }
}
