//! `horologium now` — zero-input snapshot of the current 5h and 7d
//! rate-limit windows: used %, time until reset, and a remaining USD
//! estimate. Backed by [`crate::stat::windows::aggregate`].

use std::path::PathBuf;

use anyhow::{anyhow, Result};
use chrono::{TimeZone, Utc};
use clap::Args;

use crate::source::Source;
use crate::stat::windows::{self, CostMode, Tier, Window};

#[derive(Args)]
pub struct NowArgs {
    /// Input log source. Only `codex` carries rate-limit fields.
    #[arg(long, alias = "src", value_enum, default_value_t = Source::Codex)]
    source: Source,

    /// Override the logs root (default depends on --source).
    #[arg(long)]
    root: Option<PathBuf>,

    /// Cost display mode used for the remaining-USD column.
    /// - `std`: API-equivalent (GPT-5.5 public rates)
    /// - `agg`: std × multiplier (default 1.5x) — closer to ChatGPT statusline
    /// - `both`: show both side by side
    #[arg(long, value_enum, default_value = "both")]
    show: CostMode,

    /// Multiplier applied to `std` cost when computing `agg`.
    #[arg(long, default_value_t = 1.5)]
    mult: f64,

    /// Emit one JSON object per tier (pipe-friendly) instead of a table.
    #[arg(long)]
    json: bool,
}

pub fn run(args: NowArgs) -> Result<()> {
    if !args.mult.is_finite() || args.mult <= 0.0 {
        return Err(anyhow!("--mult must be a positive finite number"));
    }
    let root = args
        .root
        .clone()
        .or_else(|| args.source.default_root())
        .ok_or_else(|| anyhow!("$HOME not set; pass --root explicitly"))?;
    let paths = crate::stat::walker_find_jsonl(&root);

    if !root.exists() {
        eprintln!(
            "warning: root `{}` does not exist — nothing to report",
            root.display(),
        );
    } else if paths.is_empty() {
        eprintln!(
            "hint: no .jsonl files found under `{}` — is `--root` correct?",
            root.display(),
        );
    }

    let report_5h = windows::aggregate(&paths, args.source, Tier::Primary, args.mult);
    let report_7d = windows::aggregate(&paths, args.source, Tier::Secondary, args.mult);
    let cur_5h = pick_current(&report_5h.windows);
    let cur_7d = pick_current(&report_7d.windows);
    let mode = args.show;

    if args.json {
        emit_json(&cur_5h, &cur_7d, mode, args.mult);
    } else {
        emit_table(&cur_5h, &cur_7d, mode);
        emit_disclaimer(mode, args.mult);
    }
    if !matches!(args.source, Source::Codex) {
        eprintln!(
            "note: `--source {}` does not carry rate-limit fields; only `codex` is supported",
            args.source,
        );
    }
    Ok(())
}

/// Choose the "current" window: the one whose `resets_at` is still in the
/// future *and* whose `last_observed` is the most recent. Falls back to
/// last_observed-newest if every reset is already in the past (e.g. stale
/// logs).
fn pick_current(windows: &[Window]) -> Option<Window> {
    let now = Utc::now().timestamp();
    let mut active: Vec<&Window> = windows.iter().filter(|w| w.resets_at > now).collect();
    if active.is_empty() {
        active = windows.iter().collect();
    }
    active.sort_by_key(|w| w.last_observed);
    active.last().map(|w| (*w).clone())
}

fn emit_table(cur_5h: &Option<Window>, cur_7d: &Option<Window>, mode: CostMode) {
    let headers: Vec<&'static str> = match mode {
        CostMode::Std | CostMode::Aggressive => {
            vec![
                "Tier",
                "Used%",
                "Resets-In",
                "Resets-At-UTC",
                "Rem.Cost",
                "Plan",
            ]
        }
        CostMode::Both => vec![
            "Tier",
            "Used%",
            "Resets-In",
            "Resets-At-UTC",
            "Rem.Std",
            "Rem.Agg",
            "Plan",
        ],
    };
    let body: Vec<Vec<String>> = [("5h", cur_5h), ("7d", cur_7d)]
        .iter()
        .map(|(label, w)| row_for(label, w.as_ref(), mode))
        .collect();
    print!(
        "{}",
        crate::stat::format::align_table(&headers, &body, None)
    );
}

fn row_for(label: &str, w: Option<&Window>, mode: CostMode) -> Vec<String> {
    let Some(w) = w else {
        return match mode {
            CostMode::Both => vec![
                label.into(),
                "—".into(),
                "—".into(),
                "—".into(),
                "—".into(),
                "—".into(),
                "—".into(),
            ],
            _ => vec![
                label.into(),
                "—".into(),
                "—".into(),
                "—".into(),
                "—".into(),
                "—".into(),
            ],
        };
    };
    let now = Utc::now().timestamp();
    let resets_in = fmt_duration(w.resets_at - now);
    let resets_at = Utc
        .timestamp_opt(w.resets_at, 0)
        .single()
        .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| format!("ts={}", w.resets_at));
    let used = format!("{:.1}%", w.last_used_percent);
    let plan = w.plan_type.clone().unwrap_or_default();
    match mode {
        CostMode::Std => {
            let rem = remaining(w, CostMode::Std);
            vec![label.into(), used, resets_in, resets_at, rem, plan]
        }
        CostMode::Aggressive => {
            let rem = remaining(w, CostMode::Aggressive);
            vec![label.into(), used, resets_in, resets_at, rem, plan]
        }
        CostMode::Both => {
            let rem_std = remaining(w, CostMode::Std);
            let rem_agg = remaining(w, CostMode::Aggressive);
            vec![
                label.into(),
                used,
                resets_in,
                resets_at,
                rem_std,
                rem_agg,
                plan,
            ]
        }
    }
}

fn remaining(w: &Window, mode: CostMode) -> String {
    match w.estimated_limit_usd(mode) {
        Some(limit) => {
            let cost = match mode {
                CostMode::Std | CostMode::Both => w.cost_usd_std,
                CostMode::Aggressive => w.cost_usd_aggressive,
            };
            let rem = (limit - cost).max(0.0);
            format!("${:.2}", rem)
        }
        None => "—".into(),
    }
}

fn fmt_duration(secs: i64) -> String {
    if secs <= 0 {
        return "reset".into();
    }
    let d = secs / 86400;
    let h = (secs % 86400) / 3600;
    let m = (secs % 3600) / 60;
    if d > 0 {
        format!("{}d{}h", d, h)
    } else if h > 0 {
        format!("{}h{}m", h, m)
    } else {
        format!("{}m", m)
    }
}

fn emit_disclaimer(mode: CostMode, mult: f64) {
    eprintln!(
        "{}",
        windows::cost_disclaimer(mode, mult, windows::DisclaimerScope::Remaining)
    );
}

fn emit_json(cur_5h: &Option<Window>, cur_7d: &Option<Window>, mode: CostMode, mult: f64) {
    for (label, w) in [("5h", cur_5h), ("7d", cur_7d)] {
        let obj = match w {
            Some(w) => {
                let resets_in_secs = (w.resets_at - Utc::now().timestamp()).max(0);
                serde_json::json!({
                    "tier": label,
                    "used_percent": w.last_used_percent,
                    "resets_at": w.resets_at,
                    "resets_at_utc": Utc.timestamp_opt(w.resets_at, 0).single().map(|d| d.to_rfc3339()),
                    "resets_in_seconds": resets_in_secs,
                    "cost_usd_std": w.cost_usd_std,
                    "cost_usd_aggressive": w.cost_usd_aggressive,
                    "estimated_limit_usd_std": w.estimated_limit_usd(CostMode::Std),
                    "estimated_limit_usd_aggressive": w.estimated_limit_usd(CostMode::Aggressive),
                    "plan_type": w.plan_type,
                    "show_mode": match mode {
                        CostMode::Std => "std",
                        CostMode::Aggressive => "agg",
                        CostMode::Both => "both",
                    },
                    "cost_multiplier": mult,
                })
            }
            None => serde_json::json!({
                "tier": label,
                "used_percent": null,
            }),
        };
        println!("{}", serde_json::to_string(&obj).unwrap());
    }
}
