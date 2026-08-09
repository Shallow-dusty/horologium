//! Horologium core: harness-agnostic usage analytics primitives.
//!
//! Everything here is independent of CLI argument parsing and status-line
//! rendering: the embedded pricing table, JSONL record parsing (Claude /
//! Codex schemas), aggregation + dedup, Codex rate-limit windows, and
//! table/NDJSON formatting. The `horologium` CLI crate is a thin shell on
//! top; external harness adapters (e.g. Horologium-Pi) may depend on this
//! crate directly instead of re-implementing or copying these modules.
//!
//! Module layout:
//! - `source`    — agent CLI selector (Claude / Codex) + default log roots
//! - `walker`    — discover JSONL files under a logs root
//! - `record`    — parse a JSONL line into a normalized [`record::Record`]
//! - `pricing`   — embedded LiteLLM snapshot + cost lookup
//! - `aggregate` — rayon-driven fold into day/session/block reports
//! - `windows`   — Codex rate-limit window aggregation (5h / 7d)
//! - `format`    — render reports as aligned tables or NDJSON

pub mod aggregate;
pub mod format;
pub mod pricing;
pub mod record;
pub mod source;
pub mod walker;
pub mod windows;
