use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "hodl", version, about = "Light crypto wallet — TUI")]
struct Cli {}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let _cli = Cli::parse();
    hodl_tui::run()
}
