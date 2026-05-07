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
use clap::{Args, Subcommand, ValueEnum};
use std::path::PathBuf;

use crate::source::Source;
use record::ServiceTier;

mod aggregate;
mod format;
pub(crate) mod pricing;
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
    /// Aggregate usage by session (one JSONL file = one session).
    Session(SessionArgs),
    /// Aggregate usage by 5-hour blocks (aligned to rate limit windows).
    Blocks(BlocksArgs),
}

#[derive(Args)]
struct BlocksArgs {
    /// Input log source.
    #[arg(long, value_enum, default_value_t = Source::Claude)]
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
    /// Codex service-tier handling for logs without a tier field.
    #[arg(long, value_enum, default_value_t = CodexServiceTierArg::Auto)]
    codex_service_tier: CodexServiceTierArg,
    /// Multiplier applied to Codex fast-mode cost estimates.
    #[arg(long, default_value_t = 2.0)]
    codex_fast_multiplier: f64,
}

#[derive(Args)]
struct SessionArgs {
    /// Input log source.
    #[arg(long, value_enum, default_value_t = Source::Claude)]
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
    /// Codex service-tier handling for logs without a tier field.
    #[arg(long, value_enum, default_value_t = CodexServiceTierArg::Auto)]
    codex_service_tier: CodexServiceTierArg,
    /// Multiplier applied to Codex fast-mode cost estimates.
    #[arg(long, default_value_t = 2.0)]
    codex_fast_multiplier: f64,
}

#[derive(Args)]
struct DailyArgs {
    /// Input log source.
    #[arg(long, value_enum, default_value_t = Source::Claude)]
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
    /// Codex service-tier handling for logs without a tier field.
    #[arg(long, value_enum, default_value_t = CodexServiceTierArg::Auto)]
    codex_service_tier: CodexServiceTierArg,
    /// Multiplier applied to Codex fast-mode cost estimates.
    #[arg(long, default_value_t = 2.0)]
    codex_fast_multiplier: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum CodexServiceTierArg {
    /// Read service tier from logs when present; otherwise mark as unknown.
    Auto,
    /// Treat all Codex records as standard tier.
    Standard,
    /// Treat all Codex records as fast tier.
    Fast,
}

pub fn run(args: StatArgs) -> Result<()> {
    match args.command {
        StatCommand::Daily(d) => daily(d),
        StatCommand::Session(s) => session(s),
        StatCommand::Blocks(b) => blocks(b),
    }
}

fn daily(args: DailyArgs) -> Result<()> {
    let root = resolve_root(args.root.clone(), args.source)?;
    let filters = build_filters(&args)?;
    let pricing = build_pricing_options(
        args.source,
        args.codex_service_tier,
        args.codex_fast_multiplier,
    )?;
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

    let report =
        aggregate::aggregate_daily_source_with_pricing(&paths, &filters, args.source, &pricing);
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

fn session(args: SessionArgs) -> Result<()> {
    let root = resolve_root(args.root.clone(), args.source)?;
    let filters = build_filters_from_session_args(&args)?;
    let pricing = build_pricing_options(
        args.source,
        args.codex_service_tier,
        args.codex_fast_multiplier,
    )?;
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

    let mut report =
        aggregate::aggregate_sessions_source_with_pricing(&paths, &filters, args.source, &pricing);
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

fn blocks(args: BlocksArgs) -> Result<()> {
    let root = resolve_root(args.root.clone(), args.source)?;
    let filters = build_filters_from_blocks_args(&args)?;
    let pricing = build_pricing_options(
        args.source,
        args.codex_service_tier,
        args.codex_fast_multiplier,
    )?;
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

    let report =
        aggregate::aggregate_blocks_source_with_pricing(&paths, &filters, args.source, &pricing);
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

fn build_pricing_options(
    source: Source,
    tier: CodexServiceTierArg,
    fast_multiplier: f64,
) -> Result<aggregate::PricingOptions> {
    if !fast_multiplier.is_finite() || fast_multiplier <= 0.0 {
        return Err(anyhow!(
            "bad --codex-fast-multiplier `{}` (expected a positive number)",
            fast_multiplier
        ));
    }
    let codex_service_tier_override = if source == Source::Codex {
        match tier {
            CodexServiceTierArg::Auto => None,
            CodexServiceTierArg::Standard => Some(ServiceTier::Standard),
            CodexServiceTierArg::Fast => Some(ServiceTier::Fast),
        }
    } else {
        None
    };
    Ok(aggregate::PricingOptions {
        codex_service_tier_override,
        codex_fast_multiplier: fast_multiplier,
    })
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
            codex_service_tier: CodexServiceTierArg::Auto,
            codex_fast_multiplier: 2.0,
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
