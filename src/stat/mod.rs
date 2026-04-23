//! Usage analytics over `~/.claude/projects/**/*.jsonl`.
//!
//! Phase 2 MVP: `horologium stat daily` reads every `assistant` record from
//! the local Claude Code session logs, deduplicates by `message.id`, buckets
//! the surviving records by calendar day (local timezone), multiplies the
//! token counts against a built-in Anthropic pricing table, and prints a
//! table or JSON rollup.
//!
//! Module layout:
//! - `walker`    — discover JSONL files under the projects root
//! - `record`    — parse a line into a normalized `Record`
//! - `pricing`   — embedded pricing table + cost lookup
//! - `aggregate` — rayon-driven per-file fold into `BTreeMap<day, Totals>`
//! - `format`    — render table or JSON

use anyhow::Result;
use clap::{Args, Subcommand};

mod aggregate;
mod format;
mod pricing;
mod record;
mod walker;

#[derive(Args)]
pub struct StatArgs {
    #[command(subcommand)]
    command: StatCommand,
}

#[derive(Subcommand)]
enum StatCommand {
    /// Aggregate usage by calendar day (local timezone).
    Daily(DailyArgs),
}

#[derive(Args)]
struct DailyArgs {
    /// Inclusive lower bound on record date, YYYY-MM-DD (local tz).
    #[arg(long)]
    since: Option<String>,
    /// Inclusive upper bound on record date, YYYY-MM-DD (local tz).
    #[arg(long)]
    until: Option<String>,
    /// Glob-like substring matched against the project's `cwd` field.
    /// Example: `--project Horologium` keeps records whose cwd contains
    /// "Horologium".
    #[arg(long)]
    project: Option<String>,
    /// Emit one JSON object per row (pipe-friendly) instead of a table.
    #[arg(long)]
    json: bool,
    /// Override the projects root (default: ~/.claude/projects).
    #[arg(long)]
    root: Option<std::path::PathBuf>,
}

pub fn run(args: StatArgs) -> Result<()> {
    match args.command {
        StatCommand::Daily(d) => daily(d),
    }
}

fn daily(_args: DailyArgs) -> Result<()> {
    // Wired up in a follow-up commit once walker/record/pricing/aggregate
    // land. Keeping this stub here lets `cargo build` stay green through
    // the incremental milestones.
    eprintln!("stat daily: not implemented yet (scaffold only)");
    Ok(())
}
