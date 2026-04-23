//! Per-day rollup of deduplicated usage records.
//!
//! Each worker reads one JSONL file, parses each line, filters non-assistant
//! and duplicate records, and folds into a local `BTreeMap`. Rayon then
//! merges the local maps into a single per-day total.

#![allow(dead_code)] // TODO: remove once wired into mod.rs

use chrono::NaiveDate;

/// Accumulated token and cost totals for one bucket (e.g. a calendar day).
#[derive(Default, Clone)]
pub struct Totals {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_5m_tokens: u64,
    pub cache_creation_1h_tokens: u64,
    pub cache_read_tokens: u64,
    pub cost_usd: f64,
    pub records: u64,
}

/// Bucket key = calendar day in local timezone. `BTreeMap<BucketKey, Totals>`
/// gives a deterministic ordered output without a separate sort pass.
pub type BucketKey = NaiveDate;
