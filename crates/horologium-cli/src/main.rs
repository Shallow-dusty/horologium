use anyhow::Result;
use clap::{Parser, Subcommand};

mod config;
mod git;
mod heatmap;
mod now;
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

    /// Show 5h + 7d rate-limit windows at a glance (zero-input).
    ///
    /// Example: horologium now
    Now(now::NowArgs),

    /// Aggregate Codex rate-limit windows (5h or 7d).
    ///
    /// Example: horologium windows 7d --show both
    Windows(stat::WindowsArgs),

    /// Aggregate usage by calendar day (local timezone).
    ///
    /// Example: horologium daily --since 2026-05-03
    Daily(stat::DailyArgs),

    /// Aggregate usage by session (one JSONL file = one session).
    ///
    /// Example: horologium sessions --sort-cost
    #[command(alias = "session")]
    Sessions(stat::SessionArgs),

    /// Aggregate usage by 5-hour blocks (aligned to rate limit windows).
    ///
    /// Example: horologium blocks
    Blocks(stat::BlocksArgs),

    /// GitHub-contribution-style activity heatmap (year / month / week / day).
    ///
    /// Example: horologium heatmap --source claude --metric cost
    ///          horologium heatmap --granularity month --at 2026-07-31
    Heatmap(heatmap::HeatmapArgs),

    /// Manage Horologium and agent statusline configuration.
    ///
    /// Example: horologium configure init
    Configure(config::ConfigureArgs),
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Status(args) => status::run(args),
        Command::Now(args) => now::run(args),
        Command::Windows(args) => stat::run_windows(args),
        Command::Daily(args) => stat::run_daily(args),
        Command::Sessions(args) => stat::run_session(args),
        Command::Blocks(args) => stat::run_blocks(args),
        Command::Heatmap(args) => heatmap::run_heatmap(args),
        Command::Configure(args) => config::run(args),
    }
}
