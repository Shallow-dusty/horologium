//! Parse one JSONL line into a normalized usage `Record`.
//!
//! Only `type == "assistant"` rows carry usage; everything else (user,
//! system, attachment, file-history-snapshot, permission-mode) is silently
//! dropped by returning `Ok(None)`. Malformed lines — invalid JSON, or an
//! assistant row missing one of the mandatory identification fields —
//! return `Err`, so callers can count and move on without aborting the
//! whole corpus scan.

#![allow(dead_code)] // wired into aggregate.rs in a later milestone

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use serde::Deserialize;

/// Single `assistant`-type record with the fields needed for costing.
#[derive(Debug, Clone)]
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

// Raw structs mirror the JSONL layout exactly. All fields optional so a
// half-written record (still being flushed to disk, say) deserializes
// without exploding — we validate the mandatory ones manually below.
#[derive(Deserialize)]
struct RawLine {
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<String>,
    cwd: Option<String>,
    message: Option<RawMessage>,
}

#[derive(Deserialize)]
struct RawMessage {
    id: Option<String>,
    model: Option<String>,
    usage: Option<RawUsage>,
}

#[derive(Deserialize)]
struct RawUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    cache_creation: Option<RawCacheCreation>,
}

#[derive(Deserialize)]
struct RawCacheCreation {
    #[serde(default)]
    ephemeral_5m_input_tokens: u64,
    #[serde(default)]
    ephemeral_1h_input_tokens: u64,
}

/// Parse one JSONL line.
///
/// - `Ok(Some(_))` — valid assistant record with usage
/// - `Ok(None)` — non-assistant row, or assistant lacking `message.usage`
/// - `Err(_)` — invalid JSON, or assistant missing timestamp / id / model
///   (corrupt line; caller should log and continue)
pub fn parse_line(line: &str) -> Result<Option<Record>> {
    let raw: RawLine = serde_json::from_str(line)?;
    if raw.kind.as_deref() != Some("assistant") {
        return Ok(None);
    }
    let Some(msg) = raw.message else {
        return Err(anyhow!("assistant record missing `message` field"));
    };
    // An assistant row without usage is rare but not malformed — e.g.
    // error/rejection responses. Skip rather than error.
    let Some(usage) = msg.usage else {
        return Ok(None);
    };
    let message_id = msg
        .id
        .ok_or_else(|| anyhow!("assistant missing `message.id`"))?;
    let model = msg
        .model
        .ok_or_else(|| anyhow!("assistant missing `message.model`"))?;
    let timestamp_str = raw
        .timestamp
        .ok_or_else(|| anyhow!("assistant missing `timestamp`"))?;
    let timestamp = DateTime::parse_from_rfc3339(&timestamp_str)
        .map_err(|e| anyhow!("bad RFC 3339 timestamp `{}`: {}", timestamp_str, e))?
        .with_timezone(&Utc);

    let (cc_5m, cc_1h) = match usage.cache_creation {
        Some(cc) => (cc.ephemeral_5m_input_tokens, cc.ephemeral_1h_input_tokens),
        None => (0, 0),
    };

    Ok(Some(Record {
        timestamp,
        message_id,
        model,
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        // `cache_creation_input_tokens` is the flat sum; `cache_creation.*`
        // is the breakdown. We keep the breakdown because pricing differs
        // between 5m and 1h ephemeral windows. The flat sum is informational
        // and not reused. If the breakdown is absent but the flat sum isn't,
        // the caller will charge at the 5m rate by convention (the more
        // common window). We represent that by using the flat sum as the
        // 5m count when no breakdown was present.
        cache_creation_5m_tokens: if usage.cache_creation_input_tokens > 0
            && cc_5m == 0
            && cc_1h == 0
        {
            usage.cache_creation_input_tokens
        } else {
            cc_5m
        },
        cache_creation_1h_tokens: cc_1h,
        cache_read_tokens: usage.cache_read_input_tokens,
        cwd: raw.cwd,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL_ASSISTANT_LINE: &str = r#"{"type":"assistant","timestamp":"2026-04-05T07:31:14.156Z","cwd":"/home/shallow/04.AI-Prism/00.Agent-CLI","message":{"id":"msg_01NVnA2gTGAftN77bpQtsS1U","model":"claude-opus-4-6","usage":{"input_tokens":2,"cache_creation_input_tokens":33627,"cache_read_input_tokens":0,"cache_creation":{"ephemeral_5m_input_tokens":33627,"ephemeral_1h_input_tokens":0},"output_tokens":57,"service_tier":"standard","inference_geo":"not_available"}}}"#;

    #[test]
    fn parses_real_assistant_line() {
        let r = parse_line(REAL_ASSISTANT_LINE).unwrap().unwrap();
        assert_eq!(r.message_id, "msg_01NVnA2gTGAftN77bpQtsS1U");
        assert_eq!(r.model, "claude-opus-4-6");
        assert_eq!(r.input_tokens, 2);
        assert_eq!(r.output_tokens, 57);
        assert_eq!(r.cache_creation_5m_tokens, 33627);
        assert_eq!(r.cache_creation_1h_tokens, 0);
        assert_eq!(r.cache_read_tokens, 0);
        assert_eq!(r.cwd.as_deref(), Some("/home/shallow/04.AI-Prism/00.Agent-CLI"));
        assert_eq!(r.timestamp.to_rfc3339(), "2026-04-05T07:31:14.156+00:00");
    }

    #[test]
    fn non_assistant_rows_are_skipped() {
        let cases = [
            r#"{"type":"user","content":"hi"}"#,
            r#"{"type":"system","subtype":"init"}"#,
            r#"{"type":"attachment","path":"x.png"}"#,
            r#"{"type":"permission-mode","mode":"bypassPermissions"}"#,
            r#"{"type":"file-history-snapshot","files":[]}"#,
        ];
        for c in cases {
            assert!(
                matches!(parse_line(c), Ok(None)),
                "expected None for: {}",
                c
            );
        }
    }

    #[test]
    fn assistant_without_usage_returns_none() {
        let line = r#"{"type":"assistant","timestamp":"2026-04-05T07:31:14.156Z","message":{"id":"msg_x","model":"claude-opus-4-7"}}"#;
        assert!(matches!(parse_line(line), Ok(None)));
    }

    #[test]
    fn invalid_json_returns_err() {
        assert!(parse_line("not json").is_err());
        assert!(parse_line(r#"{"type":"assistant""#).is_err());
    }

    #[test]
    fn assistant_missing_mandatory_fields_returns_err() {
        // Missing timestamp
        let line = r#"{"type":"assistant","message":{"id":"x","model":"y","usage":{"input_tokens":1,"output_tokens":1}}}"#;
        assert!(parse_line(line).is_err());
        // Missing model
        let line = r#"{"type":"assistant","timestamp":"2026-04-05T07:31:14Z","message":{"id":"x","usage":{"input_tokens":1,"output_tokens":1}}}"#;
        assert!(parse_line(line).is_err());
        // Missing id
        let line = r#"{"type":"assistant","timestamp":"2026-04-05T07:31:14Z","message":{"model":"y","usage":{"input_tokens":1,"output_tokens":1}}}"#;
        assert!(parse_line(line).is_err());
    }

    #[test]
    fn cache_flat_sum_used_as_5m_when_breakdown_absent() {
        // Rare shape: some older log lines have the flat cache_creation_input_tokens
        // but no `cache_creation` breakdown object. Charge the flat sum at 5m.
        let line = r#"{"type":"assistant","timestamp":"2026-04-05T07:31:14Z","message":{"id":"x","model":"claude-opus-4-6","usage":{"input_tokens":10,"cache_creation_input_tokens":500,"cache_read_input_tokens":0,"output_tokens":20}}}"#;
        let r = parse_line(line).unwrap().unwrap();
        assert_eq!(r.cache_creation_5m_tokens, 500);
        assert_eq!(r.cache_creation_1h_tokens, 0);
    }

    #[test]
    fn breakdown_takes_precedence_over_flat_sum() {
        // When both are present, breakdown is authoritative (this is the
        // normal shape of modern log lines).
        let line = r#"{"type":"assistant","timestamp":"2026-04-05T07:31:14Z","message":{"id":"x","model":"claude-opus-4-6","usage":{"input_tokens":0,"cache_creation_input_tokens":1000,"cache_read_input_tokens":0,"cache_creation":{"ephemeral_5m_input_tokens":700,"ephemeral_1h_input_tokens":300},"output_tokens":0}}}"#;
        let r = parse_line(line).unwrap().unwrap();
        assert_eq!(r.cache_creation_5m_tokens, 700);
        assert_eq!(r.cache_creation_1h_tokens, 300);
    }
}
