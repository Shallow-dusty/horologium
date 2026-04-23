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
    /// Case-sensitive substring matched against the record's `cwd`.
    /// Example: `--project Horologium` keeps records whose cwd contains
    /// "Horologium".
    #[arg(long)]
    project: Option<String>,
    /// Emit one JSON object per row (pipe-friendly) instead of a table.
    #[arg(long)]
    json: bool,
    /// Override the projects root (default: $HOME/.claude/projects).
    #[arg(long)]
    root: Option<PathBuf>,
}

pub fn run(args: StatArgs) -> Result<()> {
    match args.command {
        StatCommand::Daily(d) => daily(d),
    }
}

fn daily(args: DailyArgs) -> Result<()> {
    let root = resolve_root(args.root.clone())?;
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

    let report = aggregate::aggregate_daily(&paths, &filters);
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

fn emit_diagnostics_to_stderr(report: &aggregate::Report) {
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

fn resolve_root(override_path: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(p) = override_path {
        return Ok(p);
    }
    let home = std::env::var_os("HOME")
        .ok_or_else(|| anyhow!("$HOME not set; pass --root explicitly"))?;
    Ok(PathBuf::from(home).join(".claude/projects"))
}

fn build_filters(args: &DailyArgs) -> Result<aggregate::Filters> {
    let parse_date = |s: &str| -> Result<NaiveDate> {
        NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .map_err(|e| anyhow!("bad date `{}` (expected YYYY-MM-DD): {}", s, e))
    };
    Ok(aggregate::Filters {
        since: args.since.as_deref().map(parse_date).transpose()?,
        until: args.until.as_deref().map(parse_date).transpose()?,
        project_substring: args.project.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_args() -> DailyArgs {
        DailyArgs {
            since: None,
            until: None,
            project: None,
            json: false,
            root: None,
        }
    }

    #[test]
    fn resolve_root_uses_override_when_set() {
        let p = resolve_root(Some(PathBuf::from("/custom/root"))).unwrap();
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
}
