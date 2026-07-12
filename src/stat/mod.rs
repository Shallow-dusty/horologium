//! Usage analytics over agent CLI session logs (`~/.claude/projects`,
//! `~/.codex/sessions`, …).
//!
//! `horologium daily` / `sessions` / `blocks` read every `assistant` (or
//! Codex `token_count`) record from the local session logs, deduplicate
//! by `message.id` (Claude) or per-file (Codex), bucket by calendar day /
//! session / 5-hour block, multiply token counts against a built-in
//! pricing table, and print a table or NDJSON rollup.
//!
//! Module layout:
//! - `walker`    — discover JSONL files under the logs root
//! - `record`    — parse a line into a normalized `Record`
//! - `pricing`   — embedded pricing table + cost lookup
//! - `aggregate` — rayon-driven per-file fold into report structs
//! - `windows`   — Codex rate-limit window aggregation (5h / 7d)
//! - `format`    — render table or NDJSON
//!
//! CLI arg shape: `daily` / `sessions` / `blocks` all share [`CommonArgs`]
//! (flattened in), so `build_filters` / `resolve_root` take one input.
//! `windows` carries its own `source` / `root` / `json` because it does
//! not accept `--since` / `--until` / `--project`; flattening `CommonArgs`
//! there would silently accept flags the command ignores.

use anyhow::{anyhow, Result};
use chrono::NaiveDate;
use clap::Args;
use std::path::PathBuf;

use crate::source::Source;

mod aggregate;
pub(crate) mod format;
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

// ---- shared CLI args ---------------------------------------------------

/// Flags shared by `daily` / `sessions` / `blocks`. Flattened into each
/// subcommand's `Args` struct so the CLI surface stays in one place.
#[derive(Args)]
pub struct CommonArgs {
    /// Input log source.
    #[arg(long, alias = "src", value_enum, default_value_t = Source::Codex)]
    pub source: Source,
    /// Inclusive lower bound (YYYY-MM-DD, local tz). For `daily` / `blocks`
    /// this filters by record date; for `sessions` by session start date.
    #[arg(long)]
    pub since: Option<String>,
    /// Inclusive upper bound (YYYY-MM-DD, local tz).
    #[arg(long)]
    pub until: Option<String>,
    /// Case-sensitive substring matched against the record's `cwd` (for
    /// `sessions`, against the session's primary cwd).
    #[arg(long)]
    pub project: Option<String>,
    /// Emit one JSON object per row (pipe-friendly) instead of a table.
    #[arg(long)]
    pub json: bool,
    /// Override the logs root (default depends on `--source`).
    #[arg(long)]
    pub root: Option<PathBuf>,
}

// ---- per-subcommand arg structs ---------------------------------------

#[derive(Args)]
pub struct WindowsArgs {
    /// Tier: `5h` (primary) or `7d` (secondary). Optional positional;
    /// defaults to `7d`. Backed by `--tier` for backward compatibility.
    #[arg(value_enum, default_value = "7d")]
    tier_pos: windows::Tier,
    /// [deprecated alias] same as the positional `tier`. If both are set,
    /// this wins.
    #[arg(long, value_enum, hide = true)]
    tier: Option<windows::Tier>,
    /// Input log source. Only `codex` produces data; `claude` returns empty.
    #[arg(long, alias = "src", value_enum, default_value_t = Source::Codex)]
    source: Source,
    /// Cost display mode: `std` (API-equivalent) / `agg` (std × mult) / `both`.
    #[arg(long, alias = "cost-mode", value_enum, default_value = "std")]
    show: windows::CostMode,
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

#[derive(Args)]
pub struct BlocksArgs {
    #[command(flatten)]
    pub common: CommonArgs,
}

#[derive(Args)]
pub struct SessionArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// Sort by cost descending (default: chronological).
    #[arg(long)]
    pub sort_cost: bool,
}

#[derive(Args)]
pub struct DailyArgs {
    #[command(flatten)]
    pub common: CommonArgs,
}

// ---- dispatch ----------------------------------------------------------

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

    let tier = args.tier.unwrap_or(args.tier_pos);
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
        let out = format::format_windows_table(&report, args.show);
        print!("{}", out);
        eprintln!(
            "{}",
            windows::cost_disclaimer(args.show, multiplier, windows::DisclaimerScope::WindowCost,)
        );
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
    let root = resolve_root(args.common.root.clone(), args.common.source)?;
    let filters = build_filters(&args.common)?;
    let paths = walker::find_jsonl(&root);

    // Surface obvious misconfiguration to stderr without blocking output.
    // Common pitfalls we want visible: pointing `--root` at a wrong path,
    // or running before any agent CLI has written a session.
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

    let report = aggregate::aggregate_daily_source(&paths, &filters, args.common.source);
    let out = if args.common.json {
        format::format_ndjson(&report)
    } else {
        format::format_table(&report)
    };
    print!("{}", out);

    // Table mode already inlines these notes in stdout; JSON mode keeps
    // stdout a clean NDJSON stream, so diagnostics must go to stderr or
    // a `jq` pipeline would silently hide undercounted-cost warnings.
    if args.common.json {
        emit_diagnostics_to_stderr(&report);
    }
    Ok(())
}

pub fn run_session(args: SessionArgs) -> Result<()> {
    let root = resolve_root(args.common.root.clone(), args.common.source)?;
    let filters = build_filters(&args.common)?;
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

    let mut report = aggregate::aggregate_sessions_source(&paths, &filters, args.common.source);
    if args.sort_cost {
        report
            .sessions
            .sort_by(|a, b| b.totals.cost_usd.total_cmp(&a.totals.cost_usd));
    }
    let out = if args.common.json {
        format::format_sessions_ndjson(&report)
    } else {
        format::format_sessions_table(&report)
    };
    print!("{}", out);

    if args.common.json {
        emit_diagnostics_to_stderr(&report);
    }
    Ok(())
}

pub fn run_blocks(args: BlocksArgs) -> Result<()> {
    let root = resolve_root(args.common.root.clone(), args.common.source)?;
    let filters = build_filters(&args.common)?;
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

    let report = aggregate::aggregate_blocks_source(&paths, &filters, args.common.source);
    let out = if args.common.json {
        format::format_blocks_ndjson(&report)
    } else {
        format::format_blocks_table(&report)
    };
    print!("{}", out);

    if args.common.json {
        emit_diagnostics_to_stderr(&report);
    }
    Ok(())
}

// ---- shared helpers ----------------------------------------------------

fn resolve_root(override_path: Option<PathBuf>, source: Source) -> Result<PathBuf> {
    if let Some(p) = override_path {
        return Ok(p);
    }
    source
        .default_root()
        .ok_or_else(|| anyhow!("$HOME not set; pass --root explicitly for {} logs", source))
}

fn build_filters(common: &CommonArgs) -> Result<aggregate::Filters> {
    let parse_date = |s: &str| -> Result<NaiveDate> {
        NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .map_err(|e| anyhow!("bad date `{}` (expected YYYY-MM-DD): {}", s, e))
    };
    // An empty `--project ""` would otherwise match every cwd (including
    // records with no cwd via `unwrap_or("")` inside the filter), which
    // contradicts the documented "no cwd never matches" semantics. Treat
    // empty as absent so users who accidentally pass `--project ''` get
    // the same result as omitting the flag.
    let project_substring = common.project.clone().filter(|s| !s.is_empty());
    Ok(aggregate::Filters {
        since: common.since.as_deref().map(parse_date).transpose()?,
        until: common.until.as_deref().map(parse_date).transpose()?,
        project_substring,
    })
}

/// Print malformed / divergent-duplicate / unknown-model diagnostics to
/// stderr. Shared by `run_daily` / `run_session` / `run_blocks` in JSON
/// mode (where stdout must stay a clean NDJSON stream). Order matches
/// the in-table notes rendered by `format::format_diagnostics_notes`.
fn emit_diagnostics_to_stderr<R: aggregate::ReportDiagnostics>(report: &R) {
    if report.malformed_lines() > 0 {
        eprintln!(
            "note: {} malformed line(s) skipped",
            report.malformed_lines()
        );
    }
    if report.divergent_duplicates() > 0 {
        eprintln!(
            "note: {} duplicate message.id(s) carried incompatible request metadata — kept first-seen (possible id collision/log corruption)",
            report.divergent_duplicates(),
        );
    }
    let unknown = report.unknown_models();
    if !unknown.is_empty() {
        eprintln!("note: records with unpriced models (tokens counted, cost excluded):");
        for (model, count) in unknown.iter().take(5) {
            eprintln!("  {} × {}", model, count);
        }
        if unknown.len() > 5 {
            eprintln!("  … and {} more", unknown.len() - 5);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_common() -> CommonArgs {
        CommonArgs {
            source: Source::Claude,
            since: None,
            until: None,
            project: None,
            json: false,
            root: None,
        }
    }

    fn empty_args() -> DailyArgs {
        DailyArgs {
            common: empty_common(),
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
            common: CommonArgs {
                since: Some("2026-04-01".into()),
                until: Some("2026-04-23".into()),
                project: Some("Horologium".into()),
                ..empty_common()
            },
        };
        let f = build_filters(&args.common).unwrap();
        assert_eq!(f.since, Some(NaiveDate::from_ymd_opt(2026, 4, 1).unwrap()));
        assert_eq!(f.until, Some(NaiveDate::from_ymd_opt(2026, 4, 23).unwrap()));
        assert_eq!(f.project_substring.as_deref(), Some("Horologium"));
    }

    #[test]
    fn build_filters_errors_on_bad_date() {
        let args = DailyArgs {
            common: CommonArgs {
                since: Some("yesterday".into()),
                ..empty_common()
            },
        };
        assert!(build_filters(&args.common).is_err());
    }

    #[test]
    fn build_filters_defaults_to_none() {
        let f = build_filters(&empty_args().common).unwrap();
        assert!(f.since.is_none());
        assert!(f.until.is_none());
        assert!(f.project_substring.is_none());
    }

    #[test]
    fn build_filters_treats_empty_project_as_none() {
        let args = DailyArgs {
            common: CommonArgs {
                project: Some(String::new()),
                ..empty_common()
            },
        };
        let f = build_filters(&args.common).unwrap();
        assert!(
            f.project_substring.is_none(),
            "empty --project should normalize to None"
        );
    }
}
