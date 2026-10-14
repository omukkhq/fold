mod output;

use clap::Parser;

#[derive(Parser)]
#[command(name = "fold", version)]
struct Cli {}

fn main() -> anyhow::Result<()> {
    Cli::parse();
    output::line(&output::version_line());
    Ok(())
}
