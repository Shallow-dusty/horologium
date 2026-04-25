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
    /// Render the status line (called by Claude Code via statusLine config, not for direct use)
    Status(status::StatusArgs),
    /// Analyze usage from ~/.claude/projects JSONL logs
    ///
    /// Example: horologium stat daily --since 2026-04-20
    Stat(stat::StatArgs),
    /// Interactive TUI configurator (not yet implemented)
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
