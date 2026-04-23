//! Statusline renderer. Reads Claude Code session JSON from stdin,
//! prints a single line (or multiple) to stdout.
//!
//! JSON schema reference: https://code.claude.com/docs/en/statusline
//!
//! Output parity goal: match `~/.claude/statusline.sh` (the bash predecessor)
//! branch-by-branch.

use anyhow::{Context, Result};
use clap::Args;
use owo_colors::OwoColorize;
use serde::Deserialize;
use std::io::Read;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Args)]
pub struct StatusArgs {
    /// Render segments with Powerline arrow separators and background colors.
    /// Requires a Powerline-patched / Nerd Font for the  (U+E0B0) glyph.
    #[arg(long)]
    powerline: bool,
}

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

/// One renderable unit of the statusline. Carries both a plain-mode string
/// (pre-colored via `owo-colors` for rate segments, raw otherwise) and a
/// background color index for powerline rendering. The two modes share the
/// same segment list but pick different representations at render time.
struct Segment {
    /// Plain text without any ANSI. Used as the body in powerline mode.
    text: String,
    /// Plain-mode representation (may include ANSI fg color for rate segs).
    plain: String,
    /// Xterm 256-color index used as the segment background in powerline mode.
    pl_bg: u8,
    /// Xterm 256-color index for the segment foreground in powerline mode.
    pl_fg: u8,
}

// Fixed color pairs for powerline segments (xterm-256 indices).
// Tuned for legibility on both dark and light terminal themes.
const PL_MODEL_BG: u8 = 24; // deep blue
const PL_MODEL_FG: u8 = 15; // bright white
const PL_DIR_BG: u8 = 31; // steel blue
const PL_DIR_FG: u8 = 15;
const PL_BRANCH_BG: u8 = 22; // dark green
const PL_BRANCH_FG: u8 = 15;
const PL_CTX_BG: u8 = 237; // dark gray
const PL_CTX_FG: u8 = 15;
const PL_COST_BG: u8 = 90; // muted purple
const PL_COST_FG: u8 = 15;

const ARROW: char = '\u{e0b0}';

pub fn run(args: StatusArgs) -> Result<()> {
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .context("read stdin")?;
    let data: Input = serde_json::from_str(&buf).context("parse stdin JSON")?;

    let segments = build_segments(&data);

    let line = if args.powerline {
        render_powerline(&segments)
    } else {
        render_plain(&segments)
    };
    println!("{}", line);
    Ok(())
}

fn build_segments(data: &Input) -> Vec<Segment> {
    let mut segs: Vec<Segment> = Vec::new();

    if let Some(name) = data.model.display_name.as_deref() {
        segs.push(Segment::fixed(name.to_string(), PL_MODEL_BG, PL_MODEL_FG));
    }
    if let Some(dir) = data.workspace.current_dir.as_deref() {
        segs.push(Segment::fixed(
            basename(dir).to_string(),
            PL_DIR_BG,
            PL_DIR_FG,
        ));
        // Mirror bash `git branch --show-current 2>/dev/null`: emit branch
        // only when attached to a local branch; detached HEAD / non-git dir
        // yields nothing.
        if let Some(branch) = crate::git::current_branch(Path::new(dir)) {
            segs.push(Segment::fixed(branch, PL_BRANCH_BG, PL_BRANCH_FG));
        }
    }

    // Context % and cost are ALWAYS rendered; absent values default to 0
    // (bash parity: `jq ... // 0` + `printf '$%.2f'` with 0 fallback).
    // Note: bash `cut -d. -f1` truncates decimals, so `as i64` matches.
    let pct = data.context_window.used_percentage.unwrap_or(0.0);
    segs.push(Segment::fixed(
        format!("{}%", pct as i64),
        PL_CTX_BG,
        PL_CTX_FG,
    ));

    let cost = data.cost.total_cost_usd.unwrap_or(0.0);
    segs.push(Segment::fixed(
        format!("${:.2}", cost),
        PL_COST_BG,
        PL_COST_FG,
    ));

    // Rate limit block is gated on `five_hour` presence (bash: `[ -n "$RATE_5H" ]`).
    // When gate passes, both 5h and 7d segments emit, with 7d defaulting to 0%
    // (no countdown) if seven_day is absent.
    if let Some(rl) = data.rate_limits.as_ref() {
        if let Some(five_hour) = rl.five_hour.as_ref() {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            segs.push(build_rate_segment("5h", Some(five_hour), now));
            segs.push(build_rate_segment("7d", rl.seven_day.as_ref(), now));
        }
    }

    segs
}

impl Segment {
    /// Segment with fixed coloring: no plain-mode color, fixed powerline pair.
    fn fixed(text: String, pl_bg: u8, pl_fg: u8) -> Self {
        Self {
            plain: text.clone(),
            text,
            pl_bg,
            pl_fg,
        }
    }

    /// Segment colored by threshold (used for rate_limits 5h/7d).
    fn threshold(text: String, pct: i64) -> Self {
        let plain = colorize_plain(pct, &text);
        let (pl_fg, pl_bg) = powerline_rate_colors(pct);
        Self {
            text,
            plain,
            pl_bg,
            pl_fg,
        }
    }
}

fn build_rate_segment(label: &str, w: Option<&Window>, now: i64) -> Segment {
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
    Segment::threshold(body, pct_i)
}

fn colorize_plain(pct: i64, s: &str) -> String {
    if pct >= 90 {
        s.red().to_string()
    } else if pct >= 70 {
        s.yellow().to_string()
    } else {
        s.green().to_string()
    }
}

/// Threshold → (fg, bg) pair for powerline rate segments. Mirrors the
/// plain-mode color choice: green (<70), yellow/orange (70-89), red (>=90).
fn powerline_rate_colors(pct: i64) -> (u8, u8) {
    if pct >= 90 {
        (15, 52) // white on dark red
    } else if pct >= 70 {
        (16, 214) // black on orange
    } else {
        (15, 22) // white on dark green
    }
}

fn render_plain(segs: &[Segment]) -> String {
    segs.iter()
        .map(|s| s.plain.as_str())
        .collect::<Vec<_>>()
        .join("  ")
}

fn render_powerline(segs: &[Segment]) -> String {
    let mut out = String::new();
    for (i, s) in segs.iter().enumerate() {
        if i > 0 {
            let prev_bg = segs[i - 1].pl_bg;
            // Transition arrow: fg = previous segment's bg, bg = current bg.
            out.push_str(&format!(
                "\x1b[38;5;{};48;5;{}m{}",
                prev_bg, s.pl_bg, ARROW
            ));
        }
        out.push_str(&format!(
            "\x1b[38;5;{};48;5;{}m {} ",
            s.pl_fg, s.pl_bg, s.text
        ));
    }
    if let Some(last) = segs.last() {
        // Trailing arrow back to terminal default: reset bg, fg = last bg.
        out.push_str(&format!("\x1b[0;38;5;{}m{}\x1b[0m", last.pl_bg, ARROW));
    }
    out
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
        let s = build_rate_segment("5h", Some(&w), 0);
        assert!(
            s.text.contains("5h:90%"),
            "expected 90% (rounded), got: {}",
            s.text
        );
    }

    #[test]
    fn rate_missing_window_defaults_to_zero() {
        let s = build_rate_segment("7d", None, 0);
        assert!(s.text.contains("7d:0%"), "expected 7d:0%, got: {}", s.text);
        assert!(
            !s.text.contains("⏳"),
            "should have no countdown, got: {}",
            s.text
        );
    }

    #[test]
    fn rate_window_without_resets_at_omits_countdown() {
        let w = Window {
            used_percentage: Some(50.0),
            resets_at: None,
        };
        let s = build_rate_segment("5h", Some(&w), 0);
        assert!(s.text.contains("5h:50%"));
        assert!(!s.text.contains("⏳"));
    }

    #[test]
    fn plain_color_thresholds() {
        // Thresholds: <70 green, 70-89 yellow, >=90 red. Just verify distinct
        // color codes are emitted in plain mode.
        let g = colorize_plain(50, "x");
        let y = colorize_plain(75, "x");
        let r = colorize_plain(95, "x");
        assert_ne!(g, y);
        assert_ne!(y, r);
        assert_ne!(g, r);
    }

    #[test]
    fn powerline_rate_colors_follow_thresholds() {
        // Same thresholds as plain mode, different palette (bg-centric).
        assert_ne!(powerline_rate_colors(50), powerline_rate_colors(75));
        assert_ne!(powerline_rate_colors(75), powerline_rate_colors(95));
        assert_ne!(powerline_rate_colors(50), powerline_rate_colors(95));
    }

    #[test]
    fn render_plain_joins_with_two_spaces() {
        let segs = vec![
            Segment::fixed("a".into(), 0, 0),
            Segment::fixed("b".into(), 0, 0),
            Segment::fixed("c".into(), 0, 0),
        ];
        assert_eq!(render_plain(&segs), "a  b  c");
    }

    #[test]
    fn render_powerline_emits_arrows_and_body() {
        let segs = vec![
            Segment::fixed("A".into(), 24, 15),
            Segment::fixed("B".into(), 31, 15),
        ];
        let out = render_powerline(&segs);
        // Two bodies + transition arrow + trailing arrow.
        assert!(out.contains(" A "));
        assert!(out.contains(" B "));
        // Three arrow occurrences: one transition + one trailing (no leading).
        // Actually: 0 leading + 1 transition (between A & B) + 1 trailing = 2.
        let arrow_count = out.matches(ARROW).count();
        assert_eq!(arrow_count, 2, "expected 2 arrows, got {}: {:?}", arrow_count, out);
        // Ends with reset.
        assert!(out.ends_with("\x1b[0m"));
    }

    #[test]
    fn render_powerline_empty_segments_is_empty() {
        assert_eq!(render_powerline(&[]), "");
    }
}
