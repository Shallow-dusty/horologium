//! Usage analytics over `~/.claude/projects/**/*.jsonl`.
//!
//! Phase 2 MVP: `horologium stat daily` reads every `assistant` record from
//! the local Claude Code session logs, deduplicates by `message.id`, buckets
//! the surviving records by calendar day (local timezone), multiplies the
//! token counts against a built-in Anthropic pricing table, and prints a
//! table or NDJSON rollup.
//!
//! Module layout:
//! - `walker`    — discover JSONL files under the projects root
//! - `record`    — parse a line into a normalized `Record`
//! - `pricing`   — embedded pricing table + cost lookup
//! - `aggregate` — rayon-driven per-file fold into `BTreeMap<day, Totals>`
//! - `format`    — render table or NDJSON

use anyhow::{anyhow, Result};
use chrono::NaiveDate;
use clap::{Args, Subcommand};
use std::path::PathBuf;

use crate::source::Source;

mod aggregate;
mod format;
pub(crate) mod pricing;
mod record;
mod walker;
pub mod windows;

/// Re-export for sibling modules (e.g. `crate::now`) that need to discover
/// JSONL files under a custom root without depending on the private
/// `walker` module directly.
pub fn walker_find_jsonl(root: &std::path::Path) -> Vec<PathBuf> {
    walker::find_jsonl(root)
}

#[derive(Args)]
pub struct StatArgs {
    #[command(subcommand)]
    command: StatCommand,
}

#[derive(Subcommand)]
enum StatCommand {
    /// Aggregate usage by calendar day (local timezone).
    Daily(DailyArgs),
    /// Aggregate usage by session (one JSONL file = one session).
    Session(SessionArgs),
    /// Aggregate usage by 5-hour blocks (aligned to rate limit windows).
    Blocks(BlocksArgs),
    /// Aggregate Codex rate-limit windows (5h or 7d) from session JSONL.
    Windows(WindowsArgs),
}

#[derive(Args)]
pub struct WindowsArgs {
    /// Tier: `5h` (primary) or `7d` (secondary). Optional positional;
    /// defaults to `7d`. Backed by `--tier` for backward compatibility.
    #[arg(value_enum, default_value_t = TierArg::Sevenday)]
    tier_pos: TierArg,
    /// [deprecated alias] same as the positional `tier`. If both are set,
    /// this wins.
    #[arg(long, value_enum, hide = true)]
    tier: Option<TierArg>,
    /// Input log source. Only `codex` produces data; `claude` returns empty.
    #[arg(long, alias = "src", value_enum, default_value_t = Source::Codex)]
    source: Source,
    /// Cost display mode:
    /// - `std`: API-equivalent (GPT-5.5 public rates) only
    /// - `agg`: std × multiplier, approximating OpenAI Pro internal billing
    /// - `both`: show both columns side by side
    #[arg(long, alias = "cost-mode", value_enum, default_value_t = CostModeArg::Std)]
    show: CostModeArg,
    /// Multiplier applied to `std` cost to derive `agg` cost. Calibrate
    /// against your ChatGPT statusline value at a known used_percent
    /// (default 1.5x matches typical Pro Lite observation).
    #[arg(long, alias = "cost-multiplier", default_value_t = 1.5)]
    mult: f64,
    /// Emit one JSON object per window (pipe-friendly) instead of a table.
    #[arg(long)]
    json: bool,
    /// Override the logs root (default depends on --source).
    #[arg(long)]
    root: Option<PathBuf>,
    /// Hide windows whose max used_percent is below this threshold.
    /// Filters noise from short sessions that never accumulated usage.
    #[arg(long, alias = "min-used-percent", default_value_t = 0.0)]
    min_used: f64,
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
enum TierArg {
    /// 5-hour rolling window (`primary`).
    #[value(name = "5h")]
    Fivehour,
    /// 7-day rolling window (`secondary`).
    #[value(name = "7d")]
    Sevenday,
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
enum CostModeArg {
    /// API-equivalent (GPT-5.5 public rates) only.
    Std,
    /// std × multiplier (default 1.5).
    Agg,
    /// Both columns side by side.
    Both,
}

#[derive(Args)]
pub struct BlocksArgs {
    /// Input log source.
    #[arg(long, alias = "src", value_enum, default_value_t = Source::Codex)]
    source: Source,
    /// Inclusive lower bound on record date, YYYY-MM-DD (local tz).
    #[arg(long)]
    since: Option<String>,
    /// Inclusive upper bound on record date, YYYY-MM-DD (local tz).
    #[arg(long)]
    until: Option<String>,
    /// Case-sensitive substring matched against the record's `cwd`.
    #[arg(long)]
    project: Option<String>,
    /// Emit one JSON object per block (pipe-friendly) instead of a table.
    #[arg(long)]
    json: bool,
    /// Override the logs root (default depends on --source).
    #[arg(long)]
    root: Option<PathBuf>,
}

#[derive(Args)]
pub struct SessionArgs {
    /// Input log source.
    #[arg(long, alias = "src", value_enum, default_value_t = Source::Codex)]
    source: Source,
    /// Inclusive lower bound on session start date, YYYY-MM-DD (local tz).
    #[arg(long)]
    since: Option<String>,
    /// Inclusive upper bound on session start date, YYYY-MM-DD (local tz).
    #[arg(long)]
    until: Option<String>,
    /// Case-sensitive substring matched against the session's primary cwd.
    #[arg(long)]
    project: Option<String>,
    /// Emit one JSON object per session (pipe-friendly) instead of a table.
    #[arg(long)]
    json: bool,
    /// Override the logs root (default depends on --source).
    #[arg(long)]
    root: Option<PathBuf>,
    /// Sort by cost descending (default: chronological).
    #[arg(long)]
    sort_cost: bool,
}

#[derive(Args)]
pub struct DailyArgs {
    /// Input log source.
    #[arg(long, alias = "src", value_enum, default_value_t = Source::Codex)]
    source: Source,
    /// Inclusive lower bound on record date, YYYY-MM-DD (local tz).
    #[arg(long)]
    since: Option<String>,
    /// Inclusive upper bound on record date, YYYY-MM-DD (local tz).
    #[arg(long)]
    until: Option<String>,
    /// Case-sensitive substring matched against the record's `cwd`.
    /// Example: `--project Horologium` keeps records whose cwd contains
    /// "Horologium".
    #[arg(long)]
    project: Option<String>,
    /// Emit one JSON object per row (pipe-friendly) instead of a table.
    #[arg(long)]
    json: bool,
    /// Override the logs root (default depends on --source).
    #[arg(long)]
    root: Option<PathBuf>,
}

pub fn run(args: StatArgs) -> Result<()> {
    match args.command {
        StatCommand::Daily(d) => run_daily(d),
        StatCommand::Session(s) => run_session(s),
        StatCommand::Blocks(b) => run_blocks(b),
        StatCommand::Windows(w) => run_windows(w),
    }
}

pub fn run_windows(args: WindowsArgs) -> Result<()> {
    let root = resolve_root(args.root.clone(), args.source)?;
    let paths = walker::find_jsonl(&root);

    if !root.exists() {
        eprintln!(
            "warning: root `{}` does not exist — report will be empty",
            root.display(),
        );
    } else if paths.is_empty() {
        eprintln!(
            "hint: no .jsonl files found under `{}` — is `--root` correct?",
            root.display(),
        );
    }

    let tier_choice = args.tier.unwrap_or(args.tier_pos);
    let tier = match tier_choice {
        TierArg::Fivehour => windows::Tier::Primary,
        TierArg::Sevenday => windows::Tier::Secondary,
    };
    let cost_mode = match args.show {
        CostModeArg::Std => windows::CostMode::Std,
        CostModeArg::Agg => windows::CostMode::Aggressive,
        CostModeArg::Both => windows::CostMode::Both,
    };
    let multiplier = if args.mult.is_finite() && args.mult > 0.0 {
        args.mult
    } else {
        return Err(anyhow!("--mult must be a positive finite number"));
    };
    let mut report = windows::aggregate(&paths, args.source, tier, multiplier);
    if args.min_used > 0.0 {
        report
            .windows
            .retain(|w| w.max_used_percent >= args.min_used);
    }

    if args.json {
        let out = format::format_windows_ndjson(&report);
        print!("{}", out);
    } else {
        let out = format::format_windows_table(&report, cost_mode);
        print!("{}", out);
        // Stamp a short disclaimer so users don't read the cost column as
        // gospel — OpenAI Pro internal billing isn't fully public.
        match cost_mode {
            windows::CostMode::Std => {
                eprintln!(
                    "note: Cost is API-equivalent (GPT-5.5 public rates). OpenAI Pro internal billing \
                     is typically 30-50% higher; pass `--show agg` or `both` to add a calibrated estimate."
                );
            }
            windows::CostMode::Aggressive => {
                eprintln!(
                    "note: Cost = std × {:.2}x (OpenAI Pro internal billing estimate). \
                     Calibrate `--mult` against your ChatGPT statusline at a known used_percent.",
                    multiplier
                );
            }
            windows::CostMode::Both => {
                eprintln!(
                    "note: StdCost = API-equivalent (GPT-5.5). AggCost = std × {:.2}x \
                     (calibrate via `--mult`). EstLimit uses Std cost; multiply by {:.2}x \
                     for the aggressive estimate.",
                    multiplier, multiplier
                );
            }
        }
    }

    if !matches!(args.source, Source::Codex) {
        eprintln!(
            "note: `--source {}` does not carry rate-limit fields; only `codex` is supported",
            args.source,
        );
    }
    if report.malformed_lines > 0 {
        eprintln!("note: {} malformed line(s) skipped", report.malformed_lines);
    }
    Ok(())
}

pub fn run_daily(args: DailyArgs) -> Result<()> {
    let root = resolve_root(args.root.clone(), args.source)?;
    let filters = build_filters(&args)?;
    let paths = walker::find_jsonl(&root);

    // Surface obvious misconfiguration to stderr without blocking output.
    // Common pitfalls we want visible: pointing `--root` at a wrong path,
    // or running before Claude Code has written any session.
    if !root.exists() {
        eprintln!(
            "warning: root `{}` does not exist — report will be empty",
            root.display(),
        );
    } else if paths.is_empty() {
        eprintln!(
            "hint: no .jsonl files found under `{}` — is `--root` correct?",
            root.display(),
        );
    }

    let report = aggregate::aggregate_daily_source(&paths, &filters, args.source);
    let out = if args.json {
        format::format_ndjson(&report)
    } else {
        format::format_table(&report)
    };
    print!("{}", out);

    // Table mode already inlines these notes in stdout; JSON mode keeps
    // stdout a clean NDJSON stream, so diagnostics must go to stderr or
    // a `jq` pipeline would silently hide undercounted-cost warnings.
    if args.json {
        emit_diagnostics_to_stderr(&report);
    }
    Ok(())
}

pub fn run_session(args: SessionArgs) -> Result<()> {
    let root = resolve_root(args.root.clone(), args.source)?;
    let filters = build_filters_from_session_args(&args)?;
    let paths = walker::find_jsonl(&root);

    if !root.exists() {
        eprintln!(
            "warning: root `{}` does not exist — report will be empty",
            root.display(),
        );
    } else if paths.is_empty() {
        eprintln!(
            "hint: no .jsonl files found under `{}` — is `--root` correct?",
            root.display(),
        );
    }

    let mut report = aggregate::aggregate_sessions_source(&paths, &filters, args.source);
    if args.sort_cost {
        report
            .sessions
            .sort_by(|a, b| b.totals.cost_usd.total_cmp(&a.totals.cost_usd));
    }
    let out = if args.json {
        format::format_sessions_ndjson(&report)
    } else {
        format::format_sessions_table(&report)
    };
    print!("{}", out);

    if args.json {
        emit_session_diagnostics_to_stderr(&report);
    }
    Ok(())
}

fn emit_session_diagnostics_to_stderr(report: &aggregate::SessionReport) {
    if report.malformed_lines > 0 {
        eprintln!("note: {} malformed line(s) skipped", report.malformed_lines);
    }
    if !report.unknown_models.is_empty() {
        eprintln!("note: records with unpriced models (tokens counted, cost excluded):");
        for (model, count) in report.unknown_models.iter().take(5) {
            eprintln!("  {} × {}", model, count);
        }
        if report.unknown_models.len() > 5 {
            eprintln!("  … and {} more", report.unknown_models.len() - 5);
        }
    }
}

fn build_filters_from_session_args(args: &SessionArgs) -> Result<aggregate::Filters> {
    let parse_date = |s: &str| -> Result<NaiveDate> {
        NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .map_err(|e| anyhow!("bad date `{}` (expected YYYY-MM-DD): {}", s, e))
    };
    let project_substring = args.project.clone().filter(|s| !s.is_empty());
    Ok(aggregate::Filters {
        since: args.since.as_deref().map(parse_date).transpose()?,
        until: args.until.as_deref().map(parse_date).transpose()?,
        project_substring,
    })
}

pub fn run_blocks(args: BlocksArgs) -> Result<()> {
    let root = resolve_root(args.root.clone(), args.source)?;
    let filters = build_filters_from_blocks_args(&args)?;
    let paths = walker::find_jsonl(&root);

    if !root.exists() {
        eprintln!(
            "warning: root `{}` does not exist — report will be empty",
            root.display(),
        );
    } else if paths.is_empty() {
        eprintln!(
            "hint: no .jsonl files found under `{}` — is `--root` correct?",
            root.display(),
        );
    }

    let report = aggregate::aggregate_blocks_source(&paths, &filters, args.source);
    let out = if args.json {
        format::format_blocks_ndjson(&report)
    } else {
        format::format_blocks_table(&report)
    };
    print!("{}", out);

    if args.json {
        emit_block_diagnostics_to_stderr(&report);
    }
    Ok(())
}

fn emit_block_diagnostics_to_stderr(report: &aggregate::BlockReport) {
    if report.malformed_lines > 0 {
        eprintln!("note: {} malformed line(s) skipped", report.malformed_lines);
    }
    if report.divergent_duplicates > 0 {
        eprintln!(
            "note: {} duplicate message.id(s) carried divergent payloads — kept first-seen (log may be corrupted)",
            report.divergent_duplicates,
        );
    }
    if !report.unknown_models.is_empty() {
        eprintln!("note: records with unpriced models (tokens counted, cost excluded):");
        for (model, count) in report.unknown_models.iter().take(5) {
            eprintln!("  {} × {}", model, count);
        }
        if report.unknown_models.len() > 5 {
            eprintln!("  … and {} more", report.unknown_models.len() - 5);
        }
    }
}

fn build_filters_from_blocks_args(args: &BlocksArgs) -> Result<aggregate::Filters> {
    let parse_date = |s: &str| -> Result<NaiveDate> {
        NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .map_err(|e| anyhow!("bad date `{}` (expected YYYY-MM-DD): {}", s, e))
    };
    let project_substring = args.project.clone().filter(|s| !s.is_empty());
    Ok(aggregate::Filters {
        since: args.since.as_deref().map(parse_date).transpose()?,
        until: args.until.as_deref().map(parse_date).transpose()?,
        project_substring,
    })
}

fn emit_diagnostics_to_stderr(report: &aggregate::Report) {
    if report.malformed_lines > 0 {
        eprintln!("note: {} malformed line(s) skipped", report.malformed_lines);
    }
    if report.divergent_duplicates > 0 {
        eprintln!(
            "note: {} duplicate message.id(s) carried divergent payloads — kept first-seen (log may be corrupted)",
            report.divergent_duplicates,
        );
    }
    if !report.unknown_models.is_empty() {
        eprintln!("note: records with unpriced models (tokens counted, cost excluded):");
        for (model, count) in report.unknown_models.iter().take(5) {
            eprintln!("  {} × {}", model, count);
        }
        if report.unknown_models.len() > 5 {
            eprintln!("  … and {} more", report.unknown_models.len() - 5);
        }
    }
}

fn resolve_root(override_path: Option<PathBuf>, source: Source) -> Result<PathBuf> {
    if let Some(p) = override_path {
        return Ok(p);
    }
    source
        .default_root()
        .ok_or_else(|| anyhow!("$HOME not set; pass --root explicitly for {} logs", source))
}

fn build_filters(args: &DailyArgs) -> Result<aggregate::Filters> {
    let parse_date = |s: &str| -> Result<NaiveDate> {
        NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .map_err(|e| anyhow!("bad date `{}` (expected YYYY-MM-DD): {}", s, e))
    };
    // An empty `--project ""` would otherwise match every cwd (including
    // records with no cwd via `unwrap_or("")` inside the filter), which
    // contradicts the documented "no cwd never matches" semantics. Treat
    // empty as absent so users who accidentally pass `--project ''` get
    // the same result as omitting the flag.
    let project_substring = args.project.clone().filter(|s| !s.is_empty());
    Ok(aggregate::Filters {
        since: args.since.as_deref().map(parse_date).transpose()?,
        until: args.until.as_deref().map(parse_date).transpose()?,
        project_substring,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_args() -> DailyArgs {
        DailyArgs {
            source: Source::Claude,
            since: None,
            until: None,
            project: None,
            json: false,
            root: None,
        }
    }

    #[test]
    fn resolve_root_uses_override_when_set() {
        let p = resolve_root(Some(PathBuf::from("/custom/root")), Source::Codex).unwrap();
        assert_eq!(p, PathBuf::from("/custom/root"));
    }

    #[test]
    fn build_filters_parses_dates() {
        let args = DailyArgs {
            since: Some("2026-04-01".into()),
            until: Some("2026-04-23".into()),
            project: Some("Horologium".into()),
            ..empty_args()
        };
        let f = build_filters(&args).unwrap();
        assert_eq!(f.since, Some(NaiveDate::from_ymd_opt(2026, 4, 1).unwrap()));
        assert_eq!(f.until, Some(NaiveDate::from_ymd_opt(2026, 4, 23).unwrap()));
        assert_eq!(f.project_substring.as_deref(), Some("Horologium"));
    }

    #[test]
    fn build_filters_errors_on_bad_date() {
        let args = DailyArgs {
            since: Some("yesterday".into()),
            ..empty_args()
        };
        assert!(build_filters(&args).is_err());
    }

    #[test]
    fn build_filters_defaults_to_none() {
        let f = build_filters(&empty_args()).unwrap();
        assert!(f.since.is_none());
        assert!(f.until.is_none());
        assert!(f.project_substring.is_none());
    }

    #[test]
    fn build_filters_treats_empty_project_as_none() {
        let args = DailyArgs {
            project: Some(String::new()),
            ..empty_args()
        };
        let f = build_filters(&args).unwrap();
        assert!(
            f.project_substring.is_none(),
            "empty --project should normalize to None"
        );
    }
}
