use anyhow::Result;
use clap::{Parser, Subcommand};

mod config;
mod git;
mod source;
mod stat;
mod status;

#[derive(Parser)]
#[command(
    name = "horologium",
    version,
    about = "Agent CLI status helpers and usage analytics"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Render a status line from Claude Code stdin or a Codex session JSONL.
    Status(status::StatusArgs),
    /// Analyze usage from agent JSONL logs.
    ///
    /// Example: horologium stat daily --since 2026-04-20
    Stat(stat::StatArgs),
    /// Manage Horologium and agent statusline configuration.
    ///
    /// Example: horologium configure init
    Configure(config::ConfigureArgs),
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Status(args) => status::run(args),
        Command::Stat(args) => stat::run(args),
        Command::Configure(args) => config::run(args),
    }
}
