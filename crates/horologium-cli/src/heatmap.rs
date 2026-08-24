//! `horologium heatmap` — GitHub-contribution-style activity heatmap.
//!
//! Thin CLI shell: argument parsing + dispatch. Aggregation lives in
//! `aggregate::aggregate_heatmap_source`, value mapping and ANSI/ASCII
//! rendering in `horologium_core::heatmap` (shared with the pi helper).

use anyhow::{anyhow, Result};
use chrono::NaiveDate;
use clap::Args;

use horologium_core::aggregate::{
    aggregate_heatmap_source, HeatmapGranularity, HeatmapMetric, HeatmapReport,
};
use horologium_core::heatmap::{render_heatmap, window_for};
use horologium_core::walker;

use crate::stat::{build_filters, resolve_root as stat_resolve_root, CommonArgs};

#[derive(Args)]
pub struct HeatmapArgs {
    #[command(flatten)]
    pub common: CommonArgs,
    /// View granularity (year = 53-week grid, month = calendar grid,
    /// week = 7 day row, day = 24 hourly row).
    #[arg(long, value_enum, default_value_t = HeatmapGranularity::Year)]
    pub granularity: HeatmapGranularity,
    /// Metric driving color intensity.
    #[arg(long, value_enum, default_value_t = HeatmapMetric::Cost)]
    pub metric: HeatmapMetric,
    /// Anchor date (YYYY-MM-DD, local tz). The visible window depends on
    /// granularity: year → the 53 weeks ending with this date's week,
    /// month → its calendar month, week → its week, day → that day.
    /// Default: today.
    #[arg(long)]
    pub at: Option<String>,
    /// Disable ANSI colors; shade with ASCII block characters instead.
    #[arg(long)]
    pub plain: bool,
}

pub fn run_heatmap(args: HeatmapArgs) -> Result<()> {
    let root = stat_resolve_root(args.common.root.clone(), args.common.source)?;
    let anchor = match &args.at {
        Some(s) => NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .map_err(|_| anyhow!("invalid --at `{s}` (expected YYYY-MM-DD)"))?,
        None => chrono::Local::now().date_naive(),
    };
    let (since, until) = window_for(anchor, args.granularity);
    let mut filters = build_filters(&args.common)?;
    filters.since = Some(since);
    filters.until = Some(until);

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

    let report = aggregate_heatmap_source(&paths, &filters, args.common.source, args.granularity);

    if args.common.json {
        print!("{}", format_ndjson(&report, args.granularity));
    } else {
        print!(
            "{}",
            render_heatmap(&report, args.granularity, args.metric, anchor, !args.plain)
        );
    }
    if args.common.json {
        if report.malformed_lines > 0 {
            eprintln!("warning: {} malformed lines", report.malformed_lines);
        }
        if report.divergent_duplicates > 0 {
            eprintln!(
                "warning: {} divergent duplicate ids (cost undercount possible)",
                report.divergent_duplicates
            );
        }
    }
    Ok(())
}

fn format_ndjson(report: &HeatmapReport, g: HeatmapGranularity) -> String {
    let mut out = String::new();
    for (cell, totals) in &report.cells {
        let key = match g {
            HeatmapGranularity::Day => format!("{} {:02}", cell.date, cell.hour),
            _ => format!("{}", cell.date),
        };
        out.push_str(&format!(
            "{{\"key\":\"{key}\",\"cost_usd\":{:.6},\"tokens\":{},\"records\":{}}}\n",
            totals.cost_usd,
            totals.all_tokens(),
            totals.records,
        ));
    }
    out
}
