//! Parse one JSONL line into a normalized usage `Record`.

#![allow(dead_code)] // TODO: remove once aggregate.rs consumes these

use chrono::{DateTime, Utc};

/// Single `assistant`-type record with the fields needed for costing.
/// Non-assistant rows are filtered out before reaching this struct.
pub struct Record {
    pub timestamp: DateTime<Utc>,
    pub message_id: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_5m_tokens: u64,
    pub cache_creation_1h_tokens: u64,
    pub cache_read_tokens: u64,
    pub cwd: Option<String>,
}

/// Parse one JSONL line. Returns:
/// - `Ok(Some(Record))` for a well-formed `assistant` line with usage
/// - `Ok(None)` for any other record type (user, system, attachment, ...)
/// - `Err(_)` for malformed JSON we couldn't skip past
pub fn parse_line(_line: &str) -> anyhow::Result<Option<Record>> {
    // Implemented in follow-up commit.
    Ok(None)
}
