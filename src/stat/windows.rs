//! `stat windows` — aggregate Codex rate-limit windows from session JSONL.
//!
//! OpenAI's server stamps every `token_count` event with the authoritative
//! `rate_limits` payload:
//!
//! ```json
//! "rate_limits": {
//!   "primary":   {"used_percent": 21.0, "window_minutes": 300,   "resets_at": 1778602873},
//!   "secondary": {"used_percent": 5.0,  "window_minutes": 10080, "resets_at": 1779166500},
//!   "plan_type": "prolite"
//! }
//! ```
//!
//! `resets_at` is the unique key of a rolling window — every event observed
//! while a window is active reports the same `resets_at`; when the window
//! rolls (or the server hands out an early reset), `resets_at` jumps to a
//! new value.
//!
//! We bucket events by `resets_at`, track per-session cumulative token
//! totals (from `info.total_token_usage`), price them with the standard
//! pricing table, and combine the highest observed `used_percent` with
//! the window's cumulative cost to back-derive a "100% limit ≈ $N" estimate.
//!
//! Only `--source codex` produces data; Claude sessions don't carry
//! comparable rate-limit fields.

use std::collections::BTreeMap;
use std::path::Path;

use chrono::{DateTime, TimeZone, Utc};
use clap::ValueEnum;
use serde::Deserialize;

use crate::source::Source;

/// Which rate-limit tier to report on. Directly used as a clap `ValueEnum`
/// so the CLI surface (`5h` / `7d`) stays in one place — no wrapper enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Tier {
    /// 5-hour rolling window (`primary`).
    #[value(name = "5h")]
    Primary,
    /// 7-day rolling window (`secondary`).
    #[value(name = "7d")]
    Secondary,
}

impl Tier {
    pub fn label(&self) -> &'static str {
        match self {
            Tier::Primary => "5h",
            Tier::Secondary => "7d",
        }
    }
}

/// One rolling window, identified by `resets_at`.
#[derive(Debug, Clone)]
pub struct Window {
    pub resets_at: i64,
    pub window_minutes: u32,
    pub first_observed: DateTime<Utc>,
    pub last_observed: DateTime<Utc>,
    pub max_used_percent: f64,
    /// Used percentage as of the chronologically last event in this window —
    /// what the user actually saw in their statusline at the end. Differs
    /// from `max_used_percent` because OpenAI may report slightly lower
    /// values as the rolling window slides forward and old usage drops off.
    pub last_used_percent: f64,
    pub plan_type: Option<String>,
    /// USD cost computed from per-session cumulative-total deltas inside
    /// the window. Uses GPT-5.5 API pricing as the baseline (Codex's
    /// default model). This systematically underestimates OpenAI Pro
    /// internal billing by ~30-50% (fast mode, reasoning surcharge etc).
    pub cost_usd_std: f64,
    /// Aggressive estimate: `cost_usd_std * multiplier`. Default multiplier
    /// is 1.5x based on observed bias against ChatGPT statusline values.
    pub cost_usd_aggressive: f64,
    pub session_count: usize,
    pub event_count: usize,
    /// Token deltas attributed to this window across all sessions.
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_output_tokens: u64,
}

impl Window {
    /// Back-derive the 100% limit USD-equivalent using the given cost mode.
    /// Returns None when no usage was observed (denominator would be zero).
    /// Prefers `last_used_percent` over `max_used_percent` because the user's
    /// statusline anchor reflects last-seen, not peak.
    pub fn estimated_limit_usd(&self, mode: CostMode) -> Option<f64> {
        let cost = match mode {
            CostMode::Std | CostMode::Both => self.cost_usd_std,
            CostMode::Aggressive => self.cost_usd_aggressive,
        };
        let pct = if self.last_used_percent > 0.0 {
            self.last_used_percent
        } else if self.max_used_percent > 0.0 {
            self.max_used_percent
        } else {
            return None;
        };
        Some(cost / pct * 100.0)
    }
}

/// Selects which cost column(s) the report exposes. Directly used as a
/// clap `ValueEnum` (`std` / `agg` / `both`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum CostMode {
    /// API-equivalent pricing only (GPT-5.5 public rates).
    #[value(name = "std")]
    Std,
    /// Std × multiplier; defaults to 1.5x to approximate OpenAI Pro internal billing.
    #[value(name = "agg")]
    Aggressive,
    /// Show both columns side by side.
    #[value(name = "both")]
    Both,
}

pub struct WindowsReport {
    pub tier: Tier,
    pub windows: Vec<Window>,
    pub malformed_lines: u64,
    pub cost_multiplier: f64,
}

#[derive(Deserialize)]
struct CodexLine {
    timestamp: Option<String>,
    #[serde(default)]
    payload: Option<serde_json::Value>,
}

#[derive(Deserialize, Default, Clone)]
struct TokenUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    cached_input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    reasoning_output_tokens: u64,
    #[serde(default)]
    total_tokens: u64,
}

#[derive(Deserialize)]
struct TokenInfo {
    #[serde(default)]
    total_token_usage: Option<TokenUsage>,
}

#[derive(Deserialize)]
struct WindowFrame {
    #[serde(default)]
    used_percent: Option<f64>,
    #[serde(default)]
    window_minutes: Option<u32>,
    #[serde(default)]
    resets_at: Option<i64>,
}

#[derive(Deserialize)]
struct RateLimitsFrame {
    #[serde(default)]
    primary: Option<WindowFrame>,
    #[serde(default)]
    secondary: Option<WindowFrame>,
    #[serde(default)]
    plan_type: Option<String>,
}

/// One parsed `token_count` event with the fields we care about.
struct Event {
    ts: DateTime<Utc>,
    session_id: String,
    cum: TokenUsage,
    primary: Option<WindowFrame>,
    secondary: Option<WindowFrame>,
    plan_type: Option<String>,
}

pub fn aggregate(
    paths: &[std::path::PathBuf],
    source: Source,
    tier: Tier,
    cost_multiplier: f64,
) -> WindowsReport {
    if !matches!(source, Source::Codex) {
        return WindowsReport {
            tier,
            windows: Vec::new(),
            malformed_lines: 0,
            cost_multiplier,
        };
    }

    let mut events: Vec<Event> = Vec::new();
    let mut malformed: u64 = 0;

    for path in paths {
        let session_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("<unknown>")
            .to_string();
        let Ok(contents) = std::fs::read_to_string(path) else {
            continue;
        };
        for line in contents.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match parse_event(trimmed, &session_id) {
                Ok(Some(e)) => events.push(e),
                Ok(None) => {}
                Err(_) => malformed += 1,
            }
        }
    }

    // Group events by (tier resets_at). Two sub-aggregations side by side:
    //   - per-window: max used_percent, observation timestamps, plan_type
    //   - per-(window, session): max cumulative token usage (we pay only
    //     once per session inside the window, regardless of how many
    //     token_count events that session emits)
    let mut windows: BTreeMap<i64, WindowAccum> = BTreeMap::new();

    // Also we need per-session "baseline at window start" so the delta
    // inside the window is the difference between the session's cumulative
    // at the earliest event in this window and the cumulative observed
    // outside this window just before it started.
    //
    // For simplicity we approximate baseline = (cumulative observed in the
    // immediately previous event of the same session). If the session
    // straddles multiple windows we still get a reasonable delta.

    // Sort events chronologically so per-session previous-cumulative makes
    // sense AND `last_used_percent` reflects the truly final observation.
    events.sort_by_key(|e| e.ts);

    // session_id -> last cumulative tokens we've already counted into some window
    let mut session_consumed: BTreeMap<String, TokenUsage> = BTreeMap::new();

    for e in &events {
        let frame = match tier {
            Tier::Primary => e.primary.as_ref(),
            Tier::Secondary => e.secondary.as_ref(),
        };
        let Some(frame) = frame else { continue };
        let Some(raw_reset) = frame.resets_at else {
            continue;
        };
        let window_minutes = frame.window_minutes.unwrap_or(0);
        let used_percent = frame.used_percent.unwrap_or(0.0);
        // Different sessions query the API at slightly different times,
        // so the server-reported `resets_at` jitters by a few seconds
        // around the true reset boundary. Round to the nearest minute so
        // the same logical window collapses into one row.
        let reset_at = (raw_reset / 60) * 60;

        let prev = session_consumed
            .get(&e.session_id)
            .cloned()
            .unwrap_or_default();

        let delta_input = e.cum.input_tokens.saturating_sub(prev.input_tokens);
        let delta_cached = e
            .cum
            .cached_input_tokens
            .saturating_sub(prev.cached_input_tokens);
        let delta_output = e.cum.output_tokens.saturating_sub(prev.output_tokens);
        let delta_reasoning = e
            .cum
            .reasoning_output_tokens
            .saturating_sub(prev.reasoning_output_tokens);

        // Skip events that contribute no new tokens — they still update
        // max_used_percent though.
        let acc = windows.entry(reset_at).or_insert_with(|| WindowAccum {
            window_minutes,
            first_observed: e.ts,
            last_observed: e.ts,
            max_used_percent: used_percent,
            last_used_percent: used_percent,
            plan_type: e.plan_type.clone(),
            input_tokens: 0,
            cached_input_tokens: 0,
            output_tokens: 0,
            reasoning_output_tokens: 0,
            session_ids: BTreeMap::new(),
            event_count: 0,
        });

        acc.last_observed = e.ts;
        // Events are sorted chronologically, so the last write wins.
        acc.last_used_percent = used_percent;
        if used_percent > acc.max_used_percent {
            acc.max_used_percent = used_percent;
        }
        if acc.plan_type.is_none() && e.plan_type.is_some() {
            acc.plan_type = e.plan_type.clone();
        }
        acc.input_tokens += delta_input;
        acc.cached_input_tokens += delta_cached;
        acc.output_tokens += delta_output;
        acc.reasoning_output_tokens += delta_reasoning;
        acc.event_count += 1;
        acc.session_ids.insert(e.session_id.clone(), ());

        // Update consumed baseline so subsequent events from the same
        // session only add their own incremental tokens.
        let entry = session_consumed.entry(e.session_id.clone()).or_default();
        if e.cum.input_tokens > entry.input_tokens {
            entry.input_tokens = e.cum.input_tokens;
        }
        if e.cum.cached_input_tokens > entry.cached_input_tokens {
            entry.cached_input_tokens = e.cum.cached_input_tokens;
        }
        if e.cum.output_tokens > entry.output_tokens {
            entry.output_tokens = e.cum.output_tokens;
        }
        if e.cum.reasoning_output_tokens > entry.reasoning_output_tokens {
            entry.reasoning_output_tokens = e.cum.reasoning_output_tokens;
        }
    }

    let pricing_row = crate::stat::pricing::lookup("gpt-5.5");
    let windows: Vec<Window> = windows
        .into_iter()
        .map(|(reset_at, acc)| {
            let cost_std = if let Some(row) = pricing_row {
                let non_cached = acc.input_tokens.saturating_sub(acc.cached_input_tokens);
                let m = 1_000_000.0;
                (non_cached as f64 / m) * row.input_per_mtok
                    + (acc.cached_input_tokens as f64 / m) * row.cache_read_per_mtok
                    + ((acc.output_tokens + acc.reasoning_output_tokens) as f64 / m)
                        * row.output_per_mtok
            } else {
                0.0
            };
            let cost_aggressive = cost_std * cost_multiplier;
            Window {
                resets_at: reset_at,
                window_minutes: acc.window_minutes,
                first_observed: acc.first_observed,
                last_observed: acc.last_observed,
                max_used_percent: acc.max_used_percent,
                last_used_percent: acc.last_used_percent,
                plan_type: acc.plan_type,
                cost_usd_std: cost_std,
                cost_usd_aggressive: cost_aggressive,
                session_count: acc.session_ids.len(),
                event_count: acc.event_count,
                input_tokens: acc.input_tokens,
                cached_input_tokens: acc.cached_input_tokens,
                output_tokens: acc.output_tokens,
                reasoning_output_tokens: acc.reasoning_output_tokens,
            }
        })
        .collect();

    WindowsReport {
        tier,
        windows,
        malformed_lines: malformed,
        cost_multiplier,
    }
}

struct WindowAccum {
    window_minutes: u32,
    first_observed: DateTime<Utc>,
    last_observed: DateTime<Utc>,
    max_used_percent: f64,
    last_used_percent: f64,
    plan_type: Option<String>,
    input_tokens: u64,
    cached_input_tokens: u64,
    output_tokens: u64,
    reasoning_output_tokens: u64,
    session_ids: BTreeMap<String, ()>,
    event_count: usize,
}

fn parse_event(line: &str, session_id: &str) -> anyhow::Result<Option<Event>> {
    let raw: CodexLine = serde_json::from_str(line)?;
    let Some(payload) = raw.payload else {
        return Ok(None);
    };
    // Only token_count events carry rate_limits. Newer format has type
    // directly on payload; older format wraps under "event_msg" first.
    let inner = if payload.get("type").and_then(|t| t.as_str()) == Some("token_count") {
        payload
    } else if payload.get("type").and_then(|t| t.as_str()) == Some("event_msg") {
        let inner = payload
            .get("payload")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        if inner.get("type").and_then(|t| t.as_str()) != Some("token_count") {
            return Ok(None);
        }
        inner
    } else {
        return Ok(None);
    };

    let Some(rate_limits_value) = inner.get("rate_limits").cloned() else {
        return Ok(None);
    };
    if rate_limits_value.is_null() {
        return Ok(None);
    }
    let rate_limits: RateLimitsFrame = serde_json::from_value(rate_limits_value)?;

    let info_value = inner
        .get("info")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let cum = if info_value.is_null() {
        TokenUsage::default()
    } else {
        let info: TokenInfo = serde_json::from_value(info_value)?;
        info.total_token_usage.unwrap_or_default()
    };
    // Bail if neither limits nor token usage signal — cheaper than
    // pushing junk events through the aggregator.
    if rate_limits.primary.is_none() && rate_limits.secondary.is_none() && cum.total_tokens == 0 {
        return Ok(None);
    }

    let ts_str = raw
        .timestamp
        .ok_or_else(|| anyhow::anyhow!("codex token_count missing `timestamp`"))?;
    let ts = DateTime::parse_from_rfc3339(&ts_str)
        .map_err(|e| anyhow::anyhow!("bad RFC 3339 timestamp `{}`: {}", ts_str, e))?
        .with_timezone(&Utc);

    Ok(Some(Event {
        ts,
        session_id: session_id.to_string(),
        cum,
        primary: rate_limits.primary,
        secondary: rate_limits.secondary,
        plan_type: rate_limits.plan_type,
    }))
}

/// Format a unix timestamp as `YYYY-MM-DD HH:MM UTC` for table output.
pub fn fmt_ts(ts: i64) -> String {
    Utc.timestamp_opt(ts, 0)
        .single()
        .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| format!("ts={}", ts))
}

#[allow(dead_code)]
pub fn paths_root(_: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_jsonl(path: &Path, lines: &[&str]) {
        let mut f = std::fs::File::create(path).unwrap();
        for l in lines {
            writeln!(f, "{}", l).unwrap();
        }
    }

    #[test]
    fn aggregate_returns_empty_for_claude_source() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("rollout-x.jsonl");
        write_jsonl(&p, &["{}"]);
        let report = aggregate(&[p], Source::Claude, Tier::Secondary, 1.5);
        assert!(report.windows.is_empty());
    }

    #[test]
    fn aggregate_groups_by_resets_at() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("rollout-test.jsonl");
        // Resets_at values are minute-aligned (already multiples of 60) so
        // the per-minute jitter normalization is a no-op here.
        let l1 = r#"{"timestamp":"2026-05-12T05:00:00Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1000,"cached_input_tokens":500,"output_tokens":100,"total_tokens":1100}},"rate_limits":{"primary":{"used_percent":5.0,"window_minutes":300,"resets_at":1020},"secondary":{"used_percent":1.0,"window_minutes":10080,"resets_at":2040}}}}"#;
        let l2 = r#"{"timestamp":"2026-05-12T06:00:00Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":3000,"cached_input_tokens":1500,"output_tokens":300,"total_tokens":3300}},"rate_limits":{"primary":{"used_percent":12.0,"window_minutes":300,"resets_at":1020},"secondary":{"used_percent":3.0,"window_minutes":10080,"resets_at":2040}}}}"#;
        let l3 = r#"{"timestamp":"2026-05-12T07:00:00Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":5000,"cached_input_tokens":2500,"output_tokens":500,"total_tokens":5500}},"rate_limits":{"primary":{"used_percent":2.0,"window_minutes":300,"resets_at":3000},"secondary":{"used_percent":5.0,"window_minutes":10080,"resets_at":4020}}}}"#;
        write_jsonl(&p, &[l1, l2, l3]);

        let report = aggregate(&[p.clone()], Source::Codex, Tier::Primary, 2.0);
        assert_eq!(report.windows.len(), 2);
        let w1 = report.windows.iter().find(|w| w.resets_at == 1020).unwrap();
        let w2 = report.windows.iter().find(|w| w.resets_at == 3000).unwrap();
        assert_eq!(w1.max_used_percent, 12.0);
        // Last observed in w1 is the second event at 06:00 — same 12% as max here.
        assert_eq!(w1.last_used_percent, 12.0);
        assert_eq!(w2.max_used_percent, 2.0);
        assert_eq!(w2.last_used_percent, 2.0);
        // Per-session baseline tracking: w1 should see deltas summing to
        // the cumulative at its last event (the prior baseline was 0):
        // input=3000, cached=1500, output=300.
        assert_eq!(w1.input_tokens, 3000);
        assert_eq!(w1.cached_input_tokens, 1500);
        assert_eq!(w1.output_tokens, 300);
        // w2 picks up the additional 2000/1000/200 over w1's baseline.
        assert_eq!(w2.input_tokens, 2000);
        assert_eq!(w2.cached_input_tokens, 1000);
        assert_eq!(w2.output_tokens, 200);
        // Cost is non-zero and limit estimate divides cleanly.
        assert!(w1.cost_usd_std > 0.0);
        // Aggressive cost respects the multiplier we passed in.
        assert!((w1.cost_usd_aggressive - w1.cost_usd_std * 2.0).abs() < 1e-9);
        let limit = w1.estimated_limit_usd(CostMode::Std).unwrap();
        assert!(limit > w1.cost_usd_std);
    }

    #[test]
    fn last_used_percent_can_differ_from_max() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("rollout-peak-then-drop.jsonl");
        // Same window (resets_at=2040), three events. Peak in the middle (74%),
        // last observation drops back to 71% — exactly the W2 pattern observed
        // in real Codex traffic when the rolling window slides forward.
        let l1 = r#"{"timestamp":"2026-05-12T04:00:00Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1000,"cached_input_tokens":500,"output_tokens":100,"total_tokens":1100}},"rate_limits":{"secondary":{"used_percent":40.0,"window_minutes":10080,"resets_at":2040}}}}"#;
        let l2 = r#"{"timestamp":"2026-05-12T04:30:00Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":2000,"cached_input_tokens":1000,"output_tokens":200,"total_tokens":2200}},"rate_limits":{"secondary":{"used_percent":74.0,"window_minutes":10080,"resets_at":2040}}}}"#;
        let l3 = r#"{"timestamp":"2026-05-12T05:00:00Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":2500,"cached_input_tokens":1200,"output_tokens":250,"total_tokens":2750}},"rate_limits":{"secondary":{"used_percent":71.0,"window_minutes":10080,"resets_at":2040}}}}"#;
        write_jsonl(&p, &[l1, l2, l3]);

        let report = aggregate(&[p], Source::Codex, Tier::Secondary, 1.5);
        let w = &report.windows[0];
        assert_eq!(w.max_used_percent, 74.0);
        assert_eq!(w.last_used_percent, 71.0);
        // estimated_limit prefers last_used_percent when present and >0.
        let est = w.estimated_limit_usd(CostMode::Std).unwrap();
        assert!((est - w.cost_usd_std / 71.0 * 100.0).abs() < 1e-9);
    }

    #[test]
    fn fmt_ts_renders_utc() {
        // 1778562000 = 2026-05-12 05:00:00 UTC.
        assert_eq!(fmt_ts(1778562000), "2026-05-12 05:00");
    }
}
