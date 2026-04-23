//! Embedded Anthropic pricing table with per-model lookup and cost math.
//!
//! Prices are USD per million input/output/cache tokens. The table is
//! compiled into the binary — updates ship with a new release. Sources
//! are cited per-row so future maintainers can verify against Anthropic's
//! published pricing.

#![allow(dead_code)] // TODO: remove once aggregate.rs consumes these

pub struct PricingRow {
    pub model_id: &'static str,
    pub input_per_mtok: f64,
    pub output_per_mtok: f64,
    pub cache_write_5m_per_mtok: f64,
    pub cache_write_1h_per_mtok: f64,
    pub cache_read_per_mtok: f64,
}

/// Table populated in a follow-up commit. Keep sorted by model family
/// (opus → sonnet → haiku, newest → oldest within each family).
pub const PRICING: &[PricingRow] = &[];

/// Look up pricing by exact `model_id`. Returns `None` for unknown models;
/// callers should still count tokens but skip cost computation in that case.
pub fn lookup(_model_id: &str) -> Option<&'static PricingRow> {
    None
}
