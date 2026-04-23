//! Embedded Anthropic pricing table with per-model lookup and cost math.
//!
//! The pricing data is a slim snapshot of LiteLLM's `model_prices_and_
//! context_window.json`, filtered to `claude-*` keys and only the four
//! cost fields we need. Regenerate by running, from the repo root:
//!
//! ```sh
//! curl -sS -o /tmp/litellm.json \
//!   https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json
//! python3 scripts/gen-pricing.py /tmp/litellm.json data/litellm-anthropic-pricing.json
//! ```
//!
//! The snapshot is `include_str!`-embedded, parsed once into a HashMap,
//! then cached in an OnceLock — no runtime filesystem or network access.
//! Binary growth from the JSON is ≈4 KB.
//!
//! Unit conversion:
//! - LiteLLM stores costs as USD **per token** (e.g. 5e-06 = $5 / 1M)
//! - We store $/Mtok (5.0 in the example) so rendering & tests read
//!   naturally.
//!
//! Cache-write tiers (Anthropic public pricing):
//! - `ephemeral_5m` — the default from LiteLLM's `cache_creation_input_token_cost`
//! - `ephemeral_1h` — exactly 2× the 5m rate (not exposed by LiteLLM; hardcoded
//!   per the published schedule)

use crate::stat::record::Record;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::OnceLock;

const EMBEDDED_SNAPSHOT: &str = include_str!("../../data/litellm-anthropic-pricing.json");

pub struct PricingRow {
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    pub cache_write_5m_per_mtok: f64,
    pub cache_write_1h_per_mtok: f64,
    pub cache_read_per_mtok: f64,
}

#[derive(Deserialize)]
struct LiteLLMPrice {
    #[serde(default)]
    input_cost_per_token: f64,
    #[serde(default)]
    output_cost_per_token: f64,
    #[serde(default)]
    cache_creation_input_token_cost: f64,
    #[serde(default)]
    cache_read_input_token_cost: f64,
}

fn load_table() -> HashMap<String, PricingRow> {
    let raw: HashMap<String, LiteLLMPrice> = serde_json::from_str(EMBEDDED_SNAPSHOT)
        .expect("embedded pricing snapshot is malformed (build bug)");
    raw.into_iter()
        .map(|(k, p)| {
            let row = PricingRow {
                input_per_mtok: p.input_cost_per_token * 1_000_000.0,
                output_per_mtok: p.output_cost_per_token * 1_000_000.0,
                cache_write_5m_per_mtok: p.cache_creation_input_token_cost * 1_000_000.0,
                cache_write_1h_per_mtok: p.cache_creation_input_token_cost * 2_000_000.0,
                cache_read_per_mtok: p.cache_read_input_token_cost * 1_000_000.0,
            };
            (k, row)
        })
        .collect()
}

fn table() -> &'static HashMap<String, PricingRow> {
    static TABLE: OnceLock<HashMap<String, PricingRow>> = OnceLock::new();
    TABLE.get_or_init(load_table)
}

/// Look up pricing by `model_id`. Returns `None` for unknown models;
/// callers should still count tokens but skip cost computation in that case.
///
/// Input is normalized before lookup: upstream LiteLLM also ships
/// `anthropic/claude-*` and `openrouter/anthropic/claude-*` aliases that
/// point at the same prices, and Claude Code could in principle emit a
/// prefixed id. Strip these known prefixes so a prefixed model still
/// prices correctly instead of silently falling into `unknown_models`.
pub fn lookup(model_id: &str) -> Option<&'static PricingRow> {
    let normalized = normalize_model_id(model_id);
    table().get(normalized)
}

fn normalize_model_id(model_id: &str) -> &str {
    // Longest prefix first so the generic `anthropic/` branch doesn't
    // swallow the `openrouter/anthropic/` form.
    for prefix in ["openrouter/anthropic/", "anthropic/"] {
        if let Some(stripped) = model_id.strip_prefix(prefix) {
            return stripped;
        }
    }
    model_id
}

/// Model IDs that Claude Code emits as billable-shaped `assistant` rows
/// but that aren't real billable models. Entries here get counted into
/// token totals but suppressed from the "unknown model" warning — they
/// are not surprises worth nagging about.
const SILENT_UNKNOWN_MODELS: &[&str] = &[
    // Claude Code's sentinel for tool-use / synthetic assistant rows.
    "<synthetic>",
];

/// True when `model_id` is a known non-billable sentinel (no cost, no warning).
pub fn is_silent_unknown(model_id: &str) -> bool {
    SILENT_UNKNOWN_MODELS.contains(&model_id)
}

/// Compute USD cost for one record given its matched pricing row. All
/// five token classes are billed independently.
pub fn cost_for_record(r: &Record, row: &PricingRow) -> f64 {
    let m = 1_000_000.0;
    (r.input_tokens as f64 / m) * row.input_per_mtok
        + (r.output_tokens as f64 / m) * row.output_per_mtok
        + (r.cache_creation_5m_tokens as f64 / m) * row.cache_write_5m_per_mtok
        + (r.cache_creation_1h_tokens as f64 / m) * row.cache_write_1h_per_mtok
        + (r.cache_read_tokens as f64 / m) * row.cache_read_per_mtok
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn mk_record(model: &str, input: u64, output: u64, cc5m: u64, cc1h: u64, cr: u64) -> Record {
        Record {
            timestamp: Utc.with_ymd_and_hms(2026, 4, 5, 0, 0, 0).unwrap(),
            message_id: "x".into(),
            model: model.into(),
            input_tokens: input,
            output_tokens: output,
            cache_creation_5m_tokens: cc5m,
            cache_creation_1h_tokens: cc1h,
            cache_read_tokens: cr,
            cwd: None,
        }
    }

    #[test]
    fn snapshot_is_nonempty_and_parseable() {
        // If the snapshot got corrupted during regeneration this panics on
        // first lookup — catch it with a no-op access.
        assert!(!table().is_empty(), "embedded pricing snapshot is empty");
    }

    #[test]
    fn opus_4_7_pricing_matches_published_rates() {
        // Anthropic's published rates for Opus 4.7:
        //   $5/Mtok input, $25/Mtok output, $6.25/Mtok cache-write-5m,
        //   $12.50/Mtok cache-write-1h, $0.50/Mtok cache-read
        let row = lookup("claude-opus-4-7").expect("claude-opus-4-7 missing from snapshot");
        assert!((row.input_per_mtok - 5.0).abs() < 1e-6);
        assert!((row.output_per_mtok - 25.0).abs() < 1e-6);
        assert!((row.cache_write_5m_per_mtok - 6.25).abs() < 1e-6);
        assert!((row.cache_write_1h_per_mtok - 12.50).abs() < 1e-6);
        assert!((row.cache_read_per_mtok - 0.50).abs() < 1e-6);
    }

    #[test]
    fn cache_1h_is_exactly_twice_5m() {
        // Anthropic schedule: 1h ephemeral = 2× 5m ephemeral. Verify across
        // every row in the table rather than just opus.
        for (model_id, row) in table() {
            assert!(
                (row.cache_write_1h_per_mtok - 2.0 * row.cache_write_5m_per_mtok).abs() < 1e-6,
                "{} violates 2x rule: 5m={} 1h={}",
                model_id,
                row.cache_write_5m_per_mtok,
                row.cache_write_1h_per_mtok,
            );
        }
    }

    #[test]
    fn unknown_model_lookup_returns_none() {
        assert!(lookup("claude-opus-4-99-imaginary").is_none());
        assert!(lookup("gpt-4").is_none());
        assert!(lookup("").is_none());
    }

    #[test]
    fn prefixed_model_ids_are_normalized() {
        // LiteLLM lists aliases for each bare `claude-*` row; make sure
        // prefixed ids price identically instead of sliding into the
        // unknown_models warning bucket.
        let bare = lookup("claude-opus-4-7").expect("bare must price");
        let anth = lookup("anthropic/claude-opus-4-7").expect("anthropic/ prefix must price");
        let or = lookup("openrouter/anthropic/claude-opus-4-7")
            .expect("openrouter/anthropic/ prefix must price");
        assert_eq!(bare.input_per_mtok, anth.input_per_mtok);
        assert_eq!(bare.input_per_mtok, or.input_per_mtok);
        assert_eq!(bare.output_per_mtok, anth.output_per_mtok);
    }

    #[test]
    fn normalize_is_longest_prefix_first() {
        // Guards against the generic `anthropic/` branch stealing the
        // `openrouter/anthropic/` input and producing a still-prefixed
        // (and thus unrecognized) result.
        assert_eq!(
            normalize_model_id("openrouter/anthropic/claude-opus-4-7"),
            "claude-opus-4-7",
        );
        assert_eq!(
            normalize_model_id("anthropic/claude-opus-4-7"),
            "claude-opus-4-7",
        );
        assert_eq!(normalize_model_id("claude-opus-4-7"), "claude-opus-4-7");
    }

    #[test]
    fn silent_unknown_matches_synthetic_only() {
        assert!(is_silent_unknown("<synthetic>"));
        assert!(!is_silent_unknown("claude-opus-4-7"));
        assert!(!is_silent_unknown("claude-mystery-99"));
        assert!(!is_silent_unknown(""));
    }

    #[test]
    fn zero_tokens_is_zero_cost() {
        let r = mk_record("claude-opus-4-7", 0, 0, 0, 0, 0);
        let row = lookup("claude-opus-4-7").unwrap();
        assert_eq!(cost_for_record(&r, row), 0.0);
    }

    #[test]
    fn cost_math_input_plus_output_plus_caches() {
        // 1M input + 1M output + 1M cache-5m on opus-4-7:
        // = 5 + 25 + 6.25 = 36.25
        let r = mk_record("claude-opus-4-7", 1_000_000, 1_000_000, 1_000_000, 0, 0);
        let row = lookup("claude-opus-4-7").unwrap();
        let cost = cost_for_record(&r, row);
        assert!((cost - 36.25).abs() < 1e-6, "expected 36.25, got {}", cost);
    }

    #[test]
    fn cost_math_fractional_mtok() {
        // 500k input + 200k output + 100k cache-read on opus-4-7:
        // = 0.5*5 + 0.2*25 + 0.1*0.5 = 2.5 + 5.0 + 0.05 = 7.55
        let r = mk_record("claude-opus-4-7", 500_000, 200_000, 0, 0, 100_000);
        let row = lookup("claude-opus-4-7").unwrap();
        let cost = cost_for_record(&r, row);
        assert!((cost - 7.55).abs() < 1e-6, "expected 7.55, got {}", cost);
    }

    #[test]
    fn opus_4_1_priced_at_legacy_rate() {
        // Pre-4.5 Opus charged $15/$75 rather than the current $5/$25.
        // Regression guard: if we collapse the snapshot to a single row
        // per family, this test catches it.
        let row = lookup("claude-opus-4-1").expect("claude-opus-4-1 missing");
        assert!((row.input_per_mtok - 15.0).abs() < 1e-6);
        assert!((row.output_per_mtok - 75.0).abs() < 1e-6);
    }
}
