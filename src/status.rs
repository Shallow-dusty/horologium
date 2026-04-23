//! Statusline renderer. Reads Claude Code session JSON from stdin,
//! prints a single line (or multiple) to stdout.
//!
//! JSON schema reference: https://code.claude.com/docs/en/statusline
//!
//! Output parity goal: match `~/.claude/statusline.sh` (the bash predecessor)
//! branch-by-branch. Known intentional divergences are documented in README
//! (e.g. git branch rendering is Phase 1 TODO, not shipped yet).

use anyhow::{Context, Result};
use clap::Args;
use owo_colors::OwoColorize;
use serde::Deserialize;
use std::io::Read;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Args)]
pub struct StatusArgs {}

#[derive(Deserialize, Default)]
struct Input {
    #[serde(default)]
    model: Model,
    #[serde(default)]
    workspace: Workspace,
    #[serde(default)]
    context_window: ContextWindow,
    #[serde(default)]
    cost: Cost,
    rate_limits: Option<RateLimits>,
}

#[derive(Deserialize, Default)]
struct Model {
    display_name: Option<String>,
}

#[derive(Deserialize, Default)]
struct Workspace {
    current_dir: Option<String>,
}

#[derive(Deserialize, Default)]
struct ContextWindow {
    used_percentage: Option<f64>,
}

#[derive(Deserialize, Default)]
struct Cost {
    total_cost_usd: Option<f64>,
}

#[derive(Deserialize)]
struct RateLimits {
    five_hour: Option<Window>,
    seven_day: Option<Window>,
}

#[derive(Deserialize)]
struct Window {
    #[serde(alias = "utilization")]
    used_percentage: Option<f64>,
    resets_at: Option<i64>,
}

pub fn run(_args: StatusArgs) -> Result<()> {
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .context("read stdin")?;
    let data: Input = serde_json::from_str(&buf).context("parse stdin JSON")?;

    let mut segments: Vec<String> = Vec::new();

    if let Some(name) = data.model.display_name.as_deref() {
        segments.push(name.to_string());
    }
    if let Some(dir) = data.workspace.current_dir.as_deref() {
        segments.push(basename(dir).to_string());
        // Mirror bash `git branch --show-current 2>/dev/null`: emit branch
        // only when attached to a local branch; detached HEAD / non-git dir
        // yields nothing.
        if let Some(branch) = crate::git::current_branch(Path::new(dir)) {
            segments.push(branch);
        }
    }

    // Context % and cost are ALWAYS rendered; absent values default to 0
    // (bash parity: `jq ... // 0` + `printf '$%.2f'` with 0 fallback).
    // Note: bash `cut -d. -f1` truncates decimals, so `as i64` matches.
    let pct = data.context_window.used_percentage.unwrap_or(0.0);
    segments.push(format!("{}%", pct as i64));

    let cost = data.cost.total_cost_usd.unwrap_or(0.0);
    segments.push(format!("${:.2}", cost));

    // Rate limit block is gated on `five_hour` presence (bash: `[ -n "$RATE_5H" ]`).
    // When gate passes, both 5h and 7d segments emit, with 7d defaulting to 0%
    // (no countdown) if seven_day is absent.
    if let Some(rl) = data.rate_limits.as_ref() {
        if let Some(five_hour) = rl.five_hour.as_ref() {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            segments.push(format_window("5h", Some(five_hour), now));
            segments.push(format_window("7d", rl.seven_day.as_ref(), now));
        }
    }

    println!("{}", segments.join("  "));
    Ok(())
}

fn format_window(label: &str, w: Option<&Window>, now: i64) -> String {
    let (pct, resets_at) = match w {
        Some(w) => (w.used_percentage.unwrap_or(0.0), w.resets_at),
        None => (0.0, None),
    };
    // Bash `printf '%.0f'` rounds (banker's rounding in some libc, half-away-from-zero
    // in others). `.round()` in Rust is half-away-from-zero — close enough.
    let pct_i = pct.round() as i64;
    let mut body = format!("{}:{}%", label, pct_i);
    if let Some(reset_at) = resets_at {
        body.push_str(&format!("⏳{}", fmt_countdown(reset_at - now)));
    }
    colorize(pct_i, &body)
}

fn colorize(pct: i64, s: &str) -> String {
    if pct >= 90 {
        s.red().to_string()
    } else if pct >= 70 {
        s.yellow().to_string()
    } else {
        s.green().to_string()
    }
}

fn fmt_countdown(secs: i64) -> String {
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

fn basename(p: &str) -> &str {
    if p.is_empty() || p == "/" {
        return p;
    }
    let trimmed = p.trim_end_matches('/');
    Path::new(trimmed)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn countdown_formats() {
        assert_eq!(fmt_countdown(0), "reset");
        assert_eq!(fmt_countdown(-10), "reset");
        assert_eq!(fmt_countdown(-86400 * 365), "reset");
        assert_eq!(fmt_countdown(45 * 60), "45m");
        assert_eq!(fmt_countdown(2 * 3600 + 14 * 60), "2h14m");
        assert_eq!(fmt_countdown(3 * 86400 + 5 * 3600), "3d5h");
    }

    #[test]
    fn basename_handles_edge_cases() {
        assert_eq!(basename(""), "");
        assert_eq!(basename("/"), "/");
        assert_eq!(basename("/home/shallow"), "shallow");
        assert_eq!(basename("/home/shallow/"), "shallow");
        assert_eq!(basename("project"), "project");
        assert_eq!(basename("./project"), "project");
        assert_eq!(basename("/a/b/c/"), "c");
    }

    #[test]
    fn rate_pct_rounds_not_truncates() {
        let w = Window {
            used_percentage: Some(89.7),
            resets_at: None,
        };
        let s = format_window("5h", Some(&w), 0);
        assert!(s.contains("5h:90%"), "expected 90% (rounded), got: {}", s);
    }

    #[test]
    fn rate_missing_window_defaults_to_zero() {
        let s = format_window("7d", None, 0);
        assert!(s.contains("7d:0%"), "expected 7d:0%, got: {}", s);
        assert!(!s.contains("⏳"), "should have no countdown, got: {}", s);
    }

    #[test]
    fn rate_window_without_resets_at_omits_countdown() {
        let w = Window {
            used_percentage: Some(50.0),
            resets_at: None,
        };
        let s = format_window("5h", Some(&w), 0);
        assert!(s.contains("5h:50%"));
        assert!(!s.contains("⏳"));
    }

    #[test]
    fn color_thresholds() {
        // Thresholds: <70 green, 70-89 yellow, >=90 red.
        // Just verify distinct color codes are emitted (can't cleanly diff
        // without pulling in a strip-ANSI crate).
        let g = colorize(50, "x");
        let y = colorize(75, "x");
        let r = colorize(95, "x");
        assert_ne!(g, y);
        assert_ne!(y, r);
        assert_ne!(g, r);
    }
}
