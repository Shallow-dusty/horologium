use anyhow::Result;
use clap::{Parser, Subcommand};

mod git;
mod stat;
mod status;

#[derive(Parser)]
#[command(
    name = "horologium",
    version,
    about = "Claude Code status line and usage analytics"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Render the status line (reads Claude Code JSON from stdin)
    Status(status::StatusArgs),
    /// Analyze usage from ~/.claude/projects JSONL logs
    Stat(stat::StatArgs),
    /// Interactive TUI configurator (TODO)
    Configure,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Status(args) => status::run(args),
        Command::Stat(args) => stat::run(args),
        Command::Configure => {
            eprintln!("configure: not implemented yet (phase 3)");
            Ok(())
        }
    }
}
