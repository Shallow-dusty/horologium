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
use std::io::{IsTerminal, Read};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use horologium_core::source::Source;

#[derive(Args)]
pub struct StatusArgs {
    /// Input status source.
    #[arg(long, value_enum, default_value_t = Source::Claude)]
    source: Source,
    /// Render segments with Powerline arrow separators and background colors.
    /// Requires a Powerline-patched / Nerd Font for the  (U+E0B0) glyph.
    #[arg(long)]
    powerline: bool,
    /// Split output into two rows: identity (model/dir/branch) on top,
    /// usage (context %/cost/rate limits) below. Works alongside --powerline.
    #[arg(long)]
    multiline: bool,
    /// Emit OSC 8 hyperlink escapes so the directory and branch segments
    /// are clickable (file://... for cwd, git origin web URL for branch).
    /// Off by default because old terminals render the escape bytes literally.
    #[arg(long)]
    hyperlinks: bool,
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

#[derive(Deserialize)]
struct CodexLine {
    #[serde(rename = "type")]
    kind: Option<String>,
    payload: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct CodexTurnContext {
    cwd: Option<String>,
    model: Option<String>,
}

#[derive(Deserialize)]
struct CodexTokenInfo {
    #[serde(default)]
    last_token_usage: Option<CodexUsage>,
    #[serde(default)]
    total_token_usage: Option<CodexUsage>,
    model_context_window: Option<u64>,
}

#[derive(Clone, Deserialize)]
struct CodexUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    cached_input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    total_tokens: u64,
}

#[derive(Deserialize)]
struct CodexRateLimits {
    primary: Option<CodexRateWindow>,
    secondary: Option<CodexRateWindow>,
}

#[derive(Deserialize)]
struct CodexRateWindow {
    used_percent: Option<f64>,
    resets_at: Option<i64>,
}

/// One renderable unit of the statusline. Carries both a plain-mode string
/// (pre-colored via `owo-colors` for rate segments, raw otherwise) and a
/// background color index for powerline rendering. The two modes share the
/// same segment list but pick different representations at render time.
#[derive(Clone)]
struct Segment {
    /// Plain text without any ANSI. Used as the body in powerline mode.
    text: String,
    /// Plain-mode representation (may include ANSI fg color for rate segs).
    plain: String,
    /// Xterm 256-color index used as the segment background in powerline mode.
    pl_bg: u8,
    /// Xterm 256-color index for the segment foreground in powerline mode.
    pl_fg: u8,
    /// Row index for multiline mode. 0 = identity row (model/dir/branch),
    /// 1 = usage row (ctx%/cost/rate). Ignored when --multiline is off.
    row: u8,
    /// Optional URL for OSC 8 hyperlink wrapping. Ignored unless
    /// --hyperlinks is passed.
    link: Option<String>,
}

use crate::config::{self, Config, SegmentName, ThresholdConfig};

const ARROW: char = '\u{e0b0}';

pub fn run(args: StatusArgs) -> Result<()> {
    if std::io::stdin().is_terminal() {
        eprintln!("This command is called by Claude Code automatically via the statusLine config.");
        eprintln!("It reads session JSON from stdin and is not meant to be run directly.");
        eprintln!();
        eprintln!("To enable, add to ~/.claude/settings.json:");
        eprintln!(r#"  "statusLine": {{ "type": "command", "command": "horologium status" }}"#);
        eprintln!();
        eprintln!("Try `horologium daily` for interactive usage analytics.");
        std::process::exit(0);
    }

    let mut cfg = load_config_for_status();
    if args.powerline {
        cfg.render.powerline = true;
    }
    if args.multiline {
        cfg.render.multiline = true;
    }
    if args.hyperlinks {
        cfg.render.hyperlinks = true;
    }

    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .context("read stdin")?;
    let data = parse_input(&buf, args.source)?;

    let segments = build_segments(&data, &cfg);

    let opts = RenderOpts {
        powerline: cfg.render.powerline,
        hyperlinks: cfg.render.hyperlinks,
    };
    let output = if cfg.render.multiline {
        render_multiline(&segments, &opts)
    } else {
        render_row(&segments, &opts)
    };
    println!("{}", output);
    Ok(())
}

fn parse_input(buf: &str, source: Source) -> Result<Input> {
    match source {
        Source::Claude => serde_json::from_str(buf).context("parse Claude status JSON"),
        Source::Codex => parse_codex_input(buf),
    }
}

fn parse_codex_input(buf: &str) -> Result<Input> {
    let mut data = Input::default();
    for line in codex_json_lines(buf) {
        let raw: CodexLine = serde_json::from_str(line).context("parse Codex status JSON")?;
        match raw.kind.as_deref() {
            Some("turn_context") => {
                let Some(payload) = raw.payload else {
                    continue;
                };
                let ctx: CodexTurnContext =
                    serde_json::from_value(payload).context("parse Codex turn_context")?;
                if let Some(model) = ctx.model {
                    data.model.display_name = Some(model);
                }
                if let Some(cwd) = ctx.cwd {
                    data.workspace.current_dir = Some(cwd);
                }
            }
            Some("event_msg") => {
                let Some(payload) = raw.payload else {
                    continue;
                };
                if payload.get("type").and_then(|t| t.as_str()) != Some("token_count") {
                    continue;
                }
                if let Some(info) = payload.get("info").filter(|v| !v.is_null()).cloned() {
                    apply_codex_token_info(&mut data, info)?;
                }
                if let Some(rate_limits) =
                    payload.get("rate_limits").filter(|v| !v.is_null()).cloned()
                {
                    apply_codex_rate_limits(&mut data, rate_limits)?;
                }
            }
            _ => {}
        }
    }
    Ok(data)
}

fn codex_json_lines(buf: &str) -> Vec<&str> {
    let trimmed = buf.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    if trimmed.lines().count() == 1 {
        vec![trimmed]
    } else {
        trimmed
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect()
    }
}

fn apply_codex_token_info(data: &mut Input, info: serde_json::Value) -> Result<()> {
    let info: CodexTokenInfo = serde_json::from_value(info).context("parse Codex token_count")?;
    if let (Some(last), Some(window)) = (info.last_token_usage.as_ref(), info.model_context_window)
    {
        if window > 0 {
            let tokens = if last.total_tokens > 0 {
                last.total_tokens
            } else {
                last.input_tokens + last.output_tokens
            };
            data.context_window.used_percentage = Some(tokens as f64 * 100.0 / window as f64);
        }
    }

    if let Some(total) = info.total_token_usage.as_ref() {
        if let Some(model) = data.model.display_name.as_deref() {
            data.cost.total_cost_usd = codex_usage_cost(model, total);
        }
    }
    Ok(())
}

fn codex_usage_cost(model: &str, usage: &CodexUsage) -> Option<f64> {
    let row = horologium_core::pricing::lookup(model)?;
    let m = 1_000_000.0;
    let uncached_input = usage.input_tokens.saturating_sub(usage.cached_input_tokens);
    Some(
        (uncached_input as f64 / m) * row.input_per_mtok
            + (usage.cached_input_tokens as f64 / m) * row.cache_read_per_mtok
            + (usage.output_tokens as f64 / m) * row.output_per_mtok,
    )
}

fn apply_codex_rate_limits(data: &mut Input, rate_limits: serde_json::Value) -> Result<()> {
    let rate_limits: CodexRateLimits =
        serde_json::from_value(rate_limits).context("parse Codex rate_limits")?;
    let five_hour = rate_limits.primary.map(|w| Window {
        used_percentage: w.used_percent,
        resets_at: w.resets_at,
    });
    let seven_day = rate_limits.secondary.map(|w| Window {
        used_percentage: w.used_percent,
        resets_at: w.resets_at,
    });
    if five_hour.is_some() || seven_day.is_some() {
        data.rate_limits = Some(RateLimits {
            five_hour,
            seven_day,
        });
    }
    Ok(())
}

fn load_config_for_status() -> Config {
    match config::load_default_path() {
        Ok(cfg) => {
            for issue in config::validate(&cfg) {
                eprintln!("warning: horologium config issue: {}", issue);
            }
            cfg
        }
        Err(err) => {
            eprintln!(
                "warning: failed to load horologium config: {:#}; using defaults",
                err
            );
            Config::default()
        }
    }
}

struct RenderOpts {
    powerline: bool,
    hyperlinks: bool,
}

fn render_row(segs: &[Segment], opts: &RenderOpts) -> String {
    if opts.powerline {
        render_powerline(segs, opts.hyperlinks)
    } else {
        render_plain(segs, opts.hyperlinks)
    }
}

/// Split segments by `row`, render each group per RenderOpts, then join with
/// newlines. Empty groups are dropped so a missing row doesn't leave a blank
/// line.
fn render_multiline(segs: &[Segment], opts: &RenderOpts) -> String {
    let rows = max_row(segs) + 1;
    (0..rows)
        .map(|r| {
            segs.iter()
                .filter(|s| s.row == r)
                .cloned()
                .collect::<Vec<_>>()
        })
        .filter(|group| !group.is_empty())
        .map(|group| render_row(&group, opts))
        .collect::<Vec<_>>()
        .join("\n")
}

fn max_row(segs: &[Segment]) -> u8 {
    segs.iter().map(|s| s.row).max().unwrap_or(0)
}

fn build_segments(data: &Input, cfg: &Config) -> Vec<Segment> {
    let hyperlinks = cfg.render.hyperlinks;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let has_rate_gate = data
        .rate_limits
        .as_ref()
        .is_some_and(|rl| rl.five_hour.is_some());

    let mut segs: Vec<Segment> = Vec::new();

    for seg_cfg in &cfg.segments {
        let row = seg_cfg.resolved_row();
        let bg = seg_cfg.resolved_bg();
        let fg = seg_cfg.resolved_fg();

        match seg_cfg.name {
            SegmentName::Model => {
                if let Some(name) = data.model.display_name.as_deref() {
                    segs.push(Segment::fixed(name.to_string(), bg, fg, row));
                }
            }
            SegmentName::Dir => {
                if let Some(dir) = data.workspace.current_dir.as_deref() {
                    let dir_link = if hyperlinks {
                        Some(format!("file://{}", encode_path_for_url(dir)))
                    } else {
                        None
                    };
                    segs.push(
                        Segment::fixed(basename(dir).to_string(), bg, fg, row).with_link(dir_link),
                    );
                }
            }
            SegmentName::Branch => {
                if let Some(dir) = data.workspace.current_dir.as_deref() {
                    if let Some(branch) = crate::git::current_branch(Path::new(dir)) {
                        let branch_link = if hyperlinks {
                            crate::git::origin_web_url(Path::new(dir))
                        } else {
                            None
                        };
                        segs.push(Segment::fixed(branch, bg, fg, row).with_link(branch_link));
                    }
                }
            }
            SegmentName::Context => {
                let pct = data.context_window.used_percentage.unwrap_or(0.0);
                segs.push(Segment::fixed(format!("{}%", pct as i64), bg, fg, row));
            }
            SegmentName::Cost => {
                let cost = data.cost.total_cost_usd.unwrap_or(0.0);
                segs.push(Segment::fixed(format!("${:.2}", cost), bg, fg, row));
            }
            SegmentName::Rate5h => {
                if has_rate_gate {
                    let rl = data.rate_limits.as_ref().unwrap();
                    segs.push(build_rate_segment(
                        "5h",
                        rl.five_hour.as_ref(),
                        now,
                        row,
                        &cfg.thresholds,
                    ));
                }
            }
            SegmentName::Rate7d => {
                if has_rate_gate {
                    let rl = data.rate_limits.as_ref().unwrap();
                    segs.push(build_rate_segment(
                        "7d",
                        rl.seven_day.as_ref(),
                        now,
                        row,
                        &cfg.thresholds,
                    ));
                }
            }
        }
    }

    segs
}

impl Segment {
    /// Segment with fixed coloring: no plain-mode color, fixed powerline pair.
    fn fixed(text: String, pl_bg: u8, pl_fg: u8, row: u8) -> Self {
        Self {
            plain: text.clone(),
            text,
            pl_bg,
            pl_fg,
            row,
            link: None,
        }
    }

    fn threshold(text: String, pct: i64, row: u8, t: &ThresholdConfig) -> Self {
        let plain = colorize_plain(pct, &text, t);
        let (pl_fg, pl_bg) = powerline_rate_colors(pct, t);
        Self {
            text,
            plain,
            pl_bg,
            pl_fg,
            row,
            link: None,
        }
    }

    /// Builder-style attachment of an OSC 8 URL.
    fn with_link(mut self, url: Option<String>) -> Self {
        self.link = url;
        self
    }
}

fn build_rate_segment(
    label: &str,
    w: Option<&Window>,
    now: i64,
    row: u8,
    thresholds: &ThresholdConfig,
) -> Segment {
    let (pct, resets_at) = match w {
        Some(w) => (w.used_percentage.unwrap_or(0.0), w.resets_at),
        None => (0.0, None),
    };
    // Bash `printf '%.0f'` uses banker's rounding on glibc (IEEE 754 round-to-even).
    let pct_i = pct.round_ties_even() as i64;
    let mut body = format!("{}:{}%", label, pct_i);
    if let Some(reset_at) = resets_at {
        body.push_str(&format!("⏳{}", fmt_countdown(reset_at - now)));
    }
    Segment::threshold(body, pct_i, row, thresholds)
}

fn colorize_plain(pct: i64, s: &str, t: &ThresholdConfig) -> String {
    if pct >= t.red_above {
        s.red().to_string()
    } else if pct >= t.green_below {
        s.yellow().to_string()
    } else {
        s.green().to_string()
    }
}

fn powerline_rate_colors(pct: i64, t: &ThresholdConfig) -> (u8, u8) {
    if pct >= t.red_above {
        (15, 52) // white on dark red
    } else if pct >= t.green_below {
        (16, 214) // black on orange
    } else {
        (15, 22) // white on dark green
    }
}

fn render_plain(segs: &[Segment], hyperlinks: bool) -> String {
    segs.iter()
        .map(|s| wrap_link(&s.plain, s.link.as_deref(), hyperlinks))
        .collect::<Vec<_>>()
        .join("  ")
}

fn render_powerline(segs: &[Segment], hyperlinks: bool) -> String {
    let mut out = String::new();
    for (i, s) in segs.iter().enumerate() {
        if i > 0 {
            let prev_bg = segs[i - 1].pl_bg;
            // Transition arrow: fg = previous segment's bg, bg = current bg.
            out.push_str(&format!("\x1b[38;5;{};48;5;{}m{}", prev_bg, s.pl_bg, ARROW));
        }
        // Body (optionally wrapped in OSC 8) inherits the segment's bg so the
        // hyperlink underline appears inside the colored block.
        let body = format!("\x1b[38;5;{};48;5;{}m {} ", s.pl_fg, s.pl_bg, s.text);
        out.push_str(&wrap_link(&body, s.link.as_deref(), hyperlinks));
    }
    if let Some(last) = segs.last() {
        // Trailing arrow back to terminal default: reset bg, fg = last bg.
        out.push_str(&format!("\x1b[0;38;5;{}m{}\x1b[0m", last.pl_bg, ARROW));
    }
    out
}

/// Percent-encode a filesystem path for safe embedding in a URL (RFC 3986).
/// Preserves `/`, `:`, `@` and the unreserved set (alnum + `-._~`); encodes
/// every other byte as `%XX`. Handles UTF-8 naturally (each non-ASCII byte
/// becomes its own `%XX`). Used by the OSC 8 `file://` link on the cwd
/// segment — paths with spaces, `#`, or non-ASCII characters would otherwise
/// produce a broken hyperlink target.
fn encode_path_for_url(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for &b in path.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/' | b':' | b'@')
        {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

/// Wrap `body` in an OSC 8 hyperlink envelope when enabled and url is set.
/// Uses `ESC \` (ST) as the terminator — the modern-standard form.
fn wrap_link(body: &str, url: Option<&str>, hyperlinks: bool) -> String {
    match (hyperlinks, url) {
        (true, Some(u)) => format!("\x1b]8;;{}\x1b\\{}\x1b]8;;\x1b\\", u, body),
        _ => body.to_string(),
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

    fn default_thresholds() -> ThresholdConfig {
        ThresholdConfig::default()
    }

    fn segment_cfg(name: SegmentName) -> crate::config::SegmentConfig {
        crate::config::SegmentConfig {
            name,
            bg: None,
            fg: None,
            row: None,
        }
    }

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
        let s = build_rate_segment("5h", Some(&w), 0, 1, &default_thresholds());
        assert!(
            s.text.contains("5h:90%"),
            "expected 90% (rounded), got: {}",
            s.text
        );
    }

    #[test]
    fn rate_pct_banker_rounding_matches_bash() {
        // bash `printf '%.0f'` uses round-to-even. Verify each boundary.
        for (input, expected) in [
            (70.5, "5h:70%"), // ties → 70 (even)
            (71.5, "5h:72%"), // ties → 72 (even)
            (89.5, "5h:90%"), // ties → 90 (even)
            (90.5, "5h:90%"), // ties → 90 (even) — CRITICAL: would be 91 under .round()
            (89.7, "5h:90%"), // not a tie — half-away still correct
            (89.4, "5h:89%"), // not a tie — half-away still correct
        ] {
            let w = Window {
                used_percentage: Some(input),
                resets_at: None,
            };
            let s = build_rate_segment("5h", Some(&w), 0, 1, &default_thresholds());
            assert!(
                s.text.contains(expected),
                "for {}, expected '{}' in text but got '{}'",
                input,
                expected,
                s.text
            );
        }
    }

    #[test]
    fn rate_missing_window_defaults_to_zero() {
        let s = build_rate_segment("7d", None, 0, 1, &default_thresholds());
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
        let s = build_rate_segment("5h", Some(&w), 0, 1, &default_thresholds());
        assert!(s.text.contains("5h:50%"));
        assert!(!s.text.contains("⏳"));
    }

    #[test]
    fn plain_color_thresholds() {
        // Thresholds: <70 green, 70-89 yellow, >=90 red. Just verify distinct
        // color codes are emitted in plain mode.
        let t = default_thresholds();
        let g = colorize_plain(50, "x", &t);
        let y = colorize_plain(75, "x", &t);
        let r = colorize_plain(95, "x", &t);
        assert_ne!(g, y);
        assert_ne!(y, r);
        assert_ne!(g, r);
    }

    #[test]
    fn powerline_rate_colors_follow_thresholds() {
        // Same thresholds as plain mode, different palette (bg-centric).
        let t = default_thresholds();
        assert_ne!(powerline_rate_colors(50, &t), powerline_rate_colors(75, &t));
        assert_ne!(powerline_rate_colors(75, &t), powerline_rate_colors(95, &t));
        assert_ne!(powerline_rate_colors(50, &t), powerline_rate_colors(95, &t));
    }

    #[test]
    fn build_segments_follows_config_order_and_hidden_segments() {
        let data = Input {
            model: Model {
                display_name: Some("Opus".into()),
            },
            context_window: ContextWindow {
                used_percentage: Some(42.0),
            },
            cost: Cost {
                total_cost_usd: Some(1.23),
            },
            ..Default::default()
        };
        let cfg = Config {
            segments: vec![
                segment_cfg(SegmentName::Cost),
                segment_cfg(SegmentName::Model),
            ],
            ..Default::default()
        };

        let segs = build_segments(&data, &cfg);
        let texts = segs.iter().map(|s| s.text.as_str()).collect::<Vec<_>>();
        assert_eq!(texts, vec!["$1.23", "Opus"]);
    }

    #[test]
    fn custom_thresholds_drive_rate_segment_colors() {
        let default = default_thresholds();
        let custom = ThresholdConfig {
            green_below: 50,
            red_above: 80,
        };

        assert_ne!(
            powerline_rate_colors(65, &default),
            powerline_rate_colors(65, &custom)
        );
    }

    #[test]
    fn parse_codex_input_folds_context_tokens_and_rates() {
        let input = r#"
{"timestamp":"2026-05-03T09:00:00Z","type":"turn_context","payload":{"cwd":"/work/project","model":"gpt-5.5"}}
{"timestamp":"2026-05-03T09:00:05Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10000,"cached_input_tokens":4000,"output_tokens":500,"total_tokens":10500},"last_token_usage":{"input_tokens":1000,"cached_input_tokens":250,"output_tokens":125,"total_tokens":1125},"model_context_window":2250},"rate_limits":{"primary":{"used_percent":7.0,"resets_at":1777530376},"secondary":{"used_percent":64.0,"resets_at":1777961891}}}}
"#;
        let parsed = parse_input(input, Source::Codex).unwrap();
        assert_eq!(parsed.model.display_name.as_deref(), Some("gpt-5.5"));
        assert_eq!(
            parsed.workspace.current_dir.as_deref(),
            Some("/work/project")
        );
        assert_eq!(parsed.context_window.used_percentage, Some(50.0));
        assert!(parsed.cost.total_cost_usd.unwrap() > 0.0);

        let rl = parsed.rate_limits.unwrap();
        assert_eq!(rl.five_hour.unwrap().used_percentage, Some(7.0));
        assert_eq!(rl.seven_day.unwrap().used_percentage, Some(64.0));
    }

    #[test]
    fn render_plain_joins_with_two_spaces() {
        let segs = vec![
            Segment::fixed("a".into(), 0, 0, 0),
            Segment::fixed("b".into(), 0, 0, 0),
            Segment::fixed("c".into(), 0, 0, 0),
        ];
        assert_eq!(render_plain(&segs, false), "a  b  c");
    }

    #[test]
    fn render_powerline_emits_arrows_and_body() {
        let segs = vec![
            Segment::fixed("A".into(), 24, 15, 0),
            Segment::fixed("B".into(), 31, 15, 0),
        ];
        let out = render_powerline(&segs, false);
        // Two bodies + transition arrow + trailing arrow.
        assert!(out.contains(" A "));
        assert!(out.contains(" B "));
        // 0 leading + 1 transition (between A & B) + 1 trailing = 2.
        let arrow_count = out.matches(ARROW).count();
        assert_eq!(
            arrow_count, 2,
            "expected 2 arrows, got {}: {:?}",
            arrow_count, out
        );
        // Ends with reset.
        assert!(out.ends_with("\x1b[0m"));
    }

    #[test]
    fn render_powerline_empty_segments_is_empty() {
        assert_eq!(render_powerline(&[], false), "");
    }

    #[test]
    fn render_multiline_splits_by_row() {
        let segs = vec![
            Segment::fixed("m".into(), 0, 0, 0),
            Segment::fixed("d".into(), 0, 0, 0),
            Segment::fixed("42%".into(), 0, 0, 1),
            Segment::fixed("$0.10".into(), 0, 0, 1),
        ];
        let opts = RenderOpts {
            powerline: false,
            hyperlinks: false,
        };
        let out = render_multiline(&segs, &opts);
        let lines: Vec<&str> = out.split('\n').collect();
        assert_eq!(lines.len(), 2, "expected 2 rows, got {:?}", lines);
        assert_eq!(lines[0], "m  d");
        assert_eq!(lines[1], "42%  $0.10");
    }

    #[test]
    fn render_multiline_drops_empty_rows() {
        // Only row 0 populated: output should be a single line, not "m\n".
        let segs = vec![Segment::fixed("m".into(), 0, 0, 0)];
        let opts = RenderOpts {
            powerline: false,
            hyperlinks: false,
        };
        let out = render_multiline(&segs, &opts);
        assert_eq!(out, "m");
        assert!(!out.contains('\n'));
    }

    #[test]
    fn wrap_link_disabled_is_passthrough() {
        // --hyperlinks off: no OSC 8 envelope even when URL is set.
        assert_eq!(
            wrap_link("body", Some("https://example.com"), false),
            "body"
        );
    }

    #[test]
    fn wrap_link_without_url_is_passthrough() {
        // Segment has no URL: no envelope even when --hyperlinks is on.
        assert_eq!(wrap_link("body", None, true), "body");
    }

    #[test]
    fn encode_path_passes_unreserved_and_slash() {
        assert_eq!(encode_path_for_url("/home/alice/proj"), "/home/alice/proj");
        assert_eq!(encode_path_for_url("/a-b_c.d~e"), "/a-b_c.d~e");
    }

    #[test]
    fn encode_path_percent_encodes_special_chars() {
        assert_eq!(encode_path_for_url("/tmp/my project"), "/tmp/my%20project");
        assert_eq!(encode_path_for_url("/a#b"), "/a%23b");
        assert_eq!(encode_path_for_url("/q?x"), "/q%3Fx");
        assert_eq!(encode_path_for_url("/%raw"), "/%25raw");
    }

    #[test]
    fn encode_path_percent_encodes_utf8_bytes() {
        // 中文 -> each UTF-8 byte becomes %XX. "中" = E4 B8 AD
        assert_eq!(encode_path_for_url("/中"), "/%E4%B8%AD");
    }

    #[test]
    fn wrap_link_emits_osc8_envelope() {
        let out = wrap_link("body", Some("https://example.com"), true);
        assert!(out.starts_with("\x1b]8;;https://example.com\x1b\\"));
        assert!(out.ends_with("\x1b]8;;\x1b\\"));
        assert!(out.contains("body"));
    }

    #[test]
    fn render_plain_with_hyperlinks_wraps_segment_with_link() {
        let seg = Segment::fixed("01.Horologium".into(), 0, 0, 0).with_link(Some(
            "file:///home/shallow/08.Rust-Inscription/01.Horologium".into(),
        ));
        let out = render_plain(&[seg], true);
        assert!(out.contains("\x1b]8;;file:///home/shallow"));
        assert!(out.contains("01.Horologium"));
    }
}
