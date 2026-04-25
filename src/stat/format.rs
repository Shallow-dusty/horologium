//! Render an aggregated `Report` as either a human-readable table or
//! one JSON object per day (NDJSON).
//!
//! The table uses only ASCII padding — no `prettytable` / `tabled`
//! dependency — so the release binary stays minimal. Columns are sized
//! to the widest cell so rollups from small to production-scale corpora
//! all align without wrapping.

use super::aggregate::{BlockReport, Report, SessionReport};

/// Column order, left-to-right.
const HEADERS: &[&str] = &[
    "Day",
    "Records",
    "Input",
    "Cache-5m",
    "Cache-1h",
    "Cache-Read",
    "Output",
    "Cost",
];

fn fmt_thousands(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    // Walk right-to-left inserting ',' every 3 digits.
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, b) in bytes.iter().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
    out.chars().rev().collect()
}

fn fmt_cost(c: f64) -> String {
    format!("${:.2}", c)
}

/// Render the report as a monospace table. Includes a TOTAL footer row
/// and, when non-empty, a note summarizing malformed lines and the first
/// few unknown-model entries.
pub fn format_table(report: &Report) -> String {
    let mut rows: Vec<Vec<String>> = Vec::with_capacity(report.rows.len() + 2);

    for (date, t) in &report.rows {
        rows.push(vec![
            date.to_string(),
            fmt_thousands(t.records),
            fmt_thousands(t.input_tokens),
            fmt_thousands(t.cache_creation_5m_tokens),
            fmt_thousands(t.cache_creation_1h_tokens),
            fmt_thousands(t.cache_read_tokens),
            fmt_thousands(t.output_tokens),
            fmt_cost(t.cost_usd),
        ]);
    }

    let total = total_of(report);
    let total_row = vec![
        "TOTAL".to_string(),
        fmt_thousands(total.records),
        fmt_thousands(total.input_tokens),
        fmt_thousands(total.cache_creation_5m_tokens),
        fmt_thousands(total.cache_creation_1h_tokens),
        fmt_thousands(total.cache_read_tokens),
        fmt_thousands(total.output_tokens),
        fmt_cost(total.cost_usd),
    ];
    rows.push(total_row);

    // Column widths = max(header, all cells).
    let mut widths: Vec<usize> = HEADERS.iter().map(|h| h.len()).collect();
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.len());
        }
    }

    let mut out = String::new();
    // Header
    for (i, h) in HEADERS.iter().enumerate() {
        if i == 0 {
            out.push_str(&format!("{:<w$}", h, w = widths[i]));
        } else {
            out.push_str(&format!("  {:>w$}", h, w = widths[i]));
        }
    }
    out.push('\n');
    // Separator
    let total_width: usize = widths.iter().sum::<usize>() + 2 * (widths.len() - 1);
    out.push_str(&"-".repeat(total_width));
    out.push('\n');
    // Body
    // Separator before TOTAL
    let body_len = rows.len() - 1;
    for (row_idx, row) in rows.iter().enumerate() {
        if row_idx == body_len {
            // Print another separator before the TOTAL row.
            out.push_str(&"-".repeat(total_width));
            out.push('\n');
        }
        for (i, cell) in row.iter().enumerate() {
            if i == 0 {
                out.push_str(&format!("{:<w$}", cell, w = widths[i]));
            } else {
                out.push_str(&format!("  {:>w$}", cell, w = widths[i]));
            }
        }
        out.push('\n');
    }

    if report.malformed_lines > 0 {
        out.push('\n');
        out.push_str(&format!(
            "note: {} malformed line(s) skipped\n",
            report.malformed_lines
        ));
    }
    if report.divergent_duplicates > 0 {
        if report.malformed_lines == 0 {
            out.push('\n');
        }
        out.push_str(&format!(
            "note: {} duplicate message.id(s) carried divergent payloads — kept first-seen (log may be corrupted)\n",
            report.divergent_duplicates,
        ));
    }
    if !report.unknown_models.is_empty() {
        out.push('\n');
        out.push_str("note: records with unpriced models (tokens counted, cost excluded):\n");
        for (model, count) in report.unknown_models.iter().take(5) {
            out.push_str(&format!("  {} × {}\n", model, count));
        }
        if report.unknown_models.len() > 5 {
            out.push_str(&format!(
                "  … and {} more\n",
                report.unknown_models.len() - 5
            ));
        }
    }

    out
}

fn total_of(report: &Report) -> super::aggregate::Totals {
    let mut t = super::aggregate::Totals::default();
    for row in report.rows.values() {
        t.input_tokens += row.input_tokens;
        t.output_tokens += row.output_tokens;
        t.cache_creation_5m_tokens += row.cache_creation_5m_tokens;
        t.cache_creation_1h_tokens += row.cache_creation_1h_tokens;
        t.cache_read_tokens += row.cache_read_tokens;
        t.cost_usd += row.cost_usd;
        t.records += row.records;
    }
    t
}

/// Render the report as newline-delimited JSON. One object per row,
/// suitable for piping into `jq` or a log analyzer. Meta info (malformed
/// counts, unknown models) is intentionally omitted from stdout so
/// downstream consumers see a clean stream; print those to stderr
/// separately if desired.
pub fn format_ndjson(report: &Report) -> String {
    let mut out = String::new();
    for (date, t) in &report.rows {
        let obj = serde_json::json!({
            "date": date.to_string(),
            "records": t.records,
            "input_tokens": t.input_tokens,
            "output_tokens": t.output_tokens,
            "cache_creation_5m_tokens": t.cache_creation_5m_tokens,
            "cache_creation_1h_tokens": t.cache_creation_1h_tokens,
            "cache_read_tokens": t.cache_read_tokens,
            "cost_usd": t.cost_usd,
        });
        out.push_str(&serde_json::to_string(&obj).expect("JSON serialization of known shape"));
        out.push('\n');
    }
    out
}

const BLOCK_HEADERS: &[&str] = &["Day", "Window", "Records", "Input", "Output", "Cost"];

pub fn format_blocks_table(report: &BlockReport) -> String {
    let mut rows: Vec<Vec<String>> = Vec::with_capacity(report.rows.len() + 2);

    for (key, t) in &report.rows {
        rows.push(vec![
            key.date.to_string(),
            key.label().to_string(),
            fmt_thousands(t.records),
            fmt_thousands(t.input_tokens),
            fmt_thousands(t.output_tokens),
            fmt_cost(t.cost_usd),
        ]);
    }

    let mut total = super::aggregate::Totals::default();
    for t in report.rows.values() {
        total.input_tokens += t.input_tokens;
        total.output_tokens += t.output_tokens;
        total.cost_usd += t.cost_usd;
        total.records += t.records;
    }
    rows.push(vec![
        "TOTAL".to_string(),
        String::new(),
        fmt_thousands(total.records),
        fmt_thousands(total.input_tokens),
        fmt_thousands(total.output_tokens),
        fmt_cost(total.cost_usd),
    ]);

    let mut widths: Vec<usize> = BLOCK_HEADERS.iter().map(|h| h.len()).collect();
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.len());
        }
    }

    let mut out = String::new();
    for (i, h) in BLOCK_HEADERS.iter().enumerate() {
        if i == 0 {
            out.push_str(&format!("{:<w$}", h, w = widths[i]));
        } else {
            out.push_str(&format!("  {:>w$}", h, w = widths[i]));
        }
    }
    out.push('\n');
    let total_width: usize = widths.iter().sum::<usize>() + 2 * (widths.len() - 1);
    out.push_str(&"-".repeat(total_width));
    out.push('\n');

    let body_len = rows.len() - 1;
    for (row_idx, row) in rows.iter().enumerate() {
        if row_idx == body_len {
            out.push_str(&"-".repeat(total_width));
            out.push('\n');
        }
        for (i, cell) in row.iter().enumerate() {
            if i == 0 {
                out.push_str(&format!("{:<w$}", cell, w = widths[i]));
            } else {
                out.push_str(&format!("  {:>w$}", cell, w = widths[i]));
            }
        }
        out.push('\n');
    }

    if report.malformed_lines > 0 {
        out.push('\n');
        out.push_str(&format!(
            "note: {} malformed line(s) skipped\n",
            report.malformed_lines
        ));
    }
    if report.divergent_duplicates > 0 {
        if report.malformed_lines == 0 {
            out.push('\n');
        }
        out.push_str(&format!(
            "note: {} duplicate message.id(s) carried divergent payloads — kept first-seen (log may be corrupted)\n",
            report.divergent_duplicates,
        ));
    }
    if !report.unknown_models.is_empty() {
        out.push('\n');
        out.push_str("note: records with unpriced models (tokens counted, cost excluded):\n");
        for (model, count) in report.unknown_models.iter().take(5) {
            out.push_str(&format!("  {} × {}\n", model, count));
        }
        if report.unknown_models.len() > 5 {
            out.push_str(&format!(
                "  … and {} more\n",
                report.unknown_models.len() - 5
            ));
        }
    }

    out
}

pub fn format_blocks_ndjson(report: &BlockReport) -> String {
    let mut out = String::new();
    for (key, t) in &report.rows {
        let obj = serde_json::json!({
            "date": key.date.to_string(),
            "window": key.label(),
            "block": key.block,
            "records": t.records,
            "input_tokens": t.input_tokens,
            "output_tokens": t.output_tokens,
            "cache_creation_5m_tokens": t.cache_creation_5m_tokens,
            "cache_creation_1h_tokens": t.cache_creation_1h_tokens,
            "cache_read_tokens": t.cache_read_tokens,
            "cost_usd": t.cost_usd,
        });
        out.push_str(&serde_json::to_string(&obj).expect("JSON serialization of known shape"));
        out.push('\n');
    }
    out
}

const SESSION_HEADERS: &[&str] = &[
    "Session", "Start", "Duration", "Project", "Records", "Input", "Output", "Cost",
];

fn truncate_id(id: &str, max_chars: usize) -> String {
    id.chars().take(max_chars).collect()
}

fn fmt_duration_hm(secs: i64) -> String {
    if secs < 0 {
        return "0m".into();
    }
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    if h > 0 {
        format!("{}h{}m", h, m)
    } else {
        format!("{}m", m)
    }
}

fn fmt_local_time(ts: &chrono::DateTime<chrono::Utc>) -> String {
    use chrono::Local;
    ts.with_timezone(&Local).format("%m-%d %H:%M").to_string()
}

pub fn format_sessions_table(report: &SessionReport) -> String {
    let mut rows: Vec<Vec<String>> = Vec::with_capacity(report.sessions.len() + 2);

    for s in &report.sessions {
        let duration = (s.end - s.start).num_seconds();
        rows.push(vec![
            truncate_id(&s.session_id, 8),
            fmt_local_time(&s.start),
            fmt_duration_hm(duration),
            s.project.clone(),
            fmt_thousands(s.totals.records),
            fmt_thousands(s.totals.input_tokens),
            fmt_thousands(s.totals.output_tokens),
            fmt_cost(s.totals.cost_usd),
        ]);
    }

    // TOTAL row
    let mut total = super::aggregate::Totals::default();
    for s in &report.sessions {
        total.input_tokens += s.totals.input_tokens;
        total.output_tokens += s.totals.output_tokens;
        total.cost_usd += s.totals.cost_usd;
        total.records += s.totals.records;
    }
    let total_row = vec![
        "TOTAL".to_string(),
        format!("{} sessions", report.sessions.len()),
        String::new(),
        String::new(),
        fmt_thousands(total.records),
        fmt_thousands(total.input_tokens),
        fmt_thousands(total.output_tokens),
        fmt_cost(total.cost_usd),
    ];
    rows.push(total_row);

    let mut widths: Vec<usize> = SESSION_HEADERS.iter().map(|h| h.len()).collect();
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.len());
        }
    }

    let mut out = String::new();
    // Header
    for (i, h) in SESSION_HEADERS.iter().enumerate() {
        if i == 0 {
            out.push_str(&format!("{:<w$}", h, w = widths[i]));
        } else {
            out.push_str(&format!("  {:>w$}", h, w = widths[i]));
        }
    }
    out.push('\n');
    let total_width: usize = widths.iter().sum::<usize>() + 2 * (widths.len() - 1);
    out.push_str(&"-".repeat(total_width));
    out.push('\n');

    let body_len = rows.len() - 1;
    for (row_idx, row) in rows.iter().enumerate() {
        if row_idx == body_len {
            out.push_str(&"-".repeat(total_width));
            out.push('\n');
        }
        for (i, cell) in row.iter().enumerate() {
            if i == 0 {
                out.push_str(&format!("{:<w$}", cell, w = widths[i]));
            } else {
                out.push_str(&format!("  {:>w$}", cell, w = widths[i]));
            }
        }
        out.push('\n');
    }

    if report.malformed_lines > 0 {
        out.push('\n');
        out.push_str(&format!(
            "note: {} malformed line(s) skipped\n",
            report.malformed_lines
        ));
    }
    if !report.unknown_models.is_empty() {
        out.push('\n');
        out.push_str("note: records with unpriced models (tokens counted, cost excluded):\n");
        for (model, count) in report.unknown_models.iter().take(5) {
            out.push_str(&format!("  {} × {}\n", model, count));
        }
        if report.unknown_models.len() > 5 {
            out.push_str(&format!(
                "  … and {} more\n",
                report.unknown_models.len() - 5
            ));
        }
    }

    out
}

pub fn format_sessions_ndjson(report: &SessionReport) -> String {
    let mut out = String::new();
    for s in &report.sessions {
        let duration = (s.end - s.start).num_seconds();
        let obj = serde_json::json!({
            "session_id": s.session_id,
            "start": s.start.to_rfc3339(),
            "end": s.end.to_rfc3339(),
            "duration_seconds": duration,
            "project": s.project,
            "records": s.totals.records,
            "input_tokens": s.totals.input_tokens,
            "output_tokens": s.totals.output_tokens,
            "cache_creation_5m_tokens": s.totals.cache_creation_5m_tokens,
            "cache_creation_1h_tokens": s.totals.cache_creation_1h_tokens,
            "cache_read_tokens": s.totals.cache_read_tokens,
            "cost_usd": s.totals.cost_usd,
        });
        out.push_str(&serde_json::to_string(&obj).expect("JSON serialization of known shape"));
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stat::aggregate::{Report, SessionReport, SessionSummary, Totals};
    use chrono::{NaiveDate, TimeZone, Utc};
    use std::collections::BTreeMap;

    fn sample_report() -> Report {
        let mut rows = BTreeMap::new();
        rows.insert(
            NaiveDate::from_ymd_opt(2026, 4, 22).unwrap(),
            Totals {
                input_tokens: 12_345,
                output_tokens: 6_789,
                cache_creation_5m_tokens: 33_627,
                cache_creation_1h_tokens: 0,
                cache_read_tokens: 1_024,
                cost_usd: 1.23,
                records: 15,
            },
        );
        rows.insert(
            NaiveDate::from_ymd_opt(2026, 4, 23).unwrap(),
            Totals {
                input_tokens: 500_000,
                output_tokens: 200_000,
                cache_creation_5m_tokens: 0,
                cache_creation_1h_tokens: 100_000,
                cache_read_tokens: 0,
                cost_usd: 12.50,
                records: 3,
            },
        );
        Report {
            rows,
            malformed_lines: 0,
            unknown_models: BTreeMap::new(),
            divergent_duplicates: 0,
        }
    }

    #[test]
    fn table_header_and_body_align() {
        let s = format_table(&sample_report());
        let lines: Vec<&str> = s.lines().collect();
        // Header + 1 separator + 2 rows + 1 separator before TOTAL + TOTAL = 6
        assert_eq!(lines.len(), 6, "unexpected line count:\n{}", s);
        // Header contains each column label.
        assert!(lines[0].contains("Day"));
        assert!(lines[0].contains("Input"));
        assert!(lines[0].contains("Cost"));
        // TOTAL row sums input = 12,345 + 500,000 = 512,345
        assert!(lines[5].contains("TOTAL"));
        assert!(lines[5].contains("512,345"));
        // Cost TOTAL = 13.73
        assert!(lines[5].contains("$13.73"));
    }

    #[test]
    fn table_notes_malformed_and_unknown() {
        let mut r = sample_report();
        r.malformed_lines = 3;
        r.unknown_models.insert("claude-mystery-99".into(), 2);
        let s = format_table(&r);
        assert!(s.contains("3 malformed line(s)"));
        assert!(s.contains("claude-mystery-99 × 2"));
    }

    #[test]
    fn table_notes_divergent_duplicates() {
        let mut r = sample_report();
        r.divergent_duplicates = 2;
        let s = format_table(&r);
        assert!(s.contains("2 duplicate message.id(s) carried divergent payloads"));
    }

    #[test]
    fn table_thousands_separator() {
        let s = format_table(&sample_report());
        assert!(s.contains("12,345"), "expected thousands-sep in: {}", s);
        assert!(s.contains("500,000"));
    }

    #[test]
    fn ndjson_one_object_per_day_sorted() {
        let s = format_ndjson(&sample_report());
        let lines: Vec<&str> = s.lines().collect();
        assert_eq!(lines.len(), 2);
        // Dates should appear in BTreeMap (chronological) order.
        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(first["date"], "2026-04-22");
        assert_eq!(second["date"], "2026-04-23");
        assert_eq!(first["input_tokens"], 12_345);
        assert!((first["cost_usd"].as_f64().unwrap() - 1.23).abs() < 1e-9);
    }

    #[test]
    fn empty_report_still_prints_header_and_total() {
        let r = Report::default();
        let s = format_table(&r);
        assert!(s.contains("Day"));
        assert!(s.contains("TOTAL"));
        assert!(s.contains("$0.00"));
    }

    #[test]
    fn empty_report_ndjson_is_empty() {
        let r = Report::default();
        assert_eq!(format_ndjson(&r), "");
    }

    fn sample_session_report() -> SessionReport {
        SessionReport {
            sessions: vec![
                SessionSummary {
                    session_id: "abcd1234-0000-0000-0000-000000000000".into(),
                    start: Utc.with_ymd_and_hms(2026, 4, 23, 8, 0, 0).unwrap(),
                    end: Utc.with_ymd_and_hms(2026, 4, 23, 10, 30, 0).unwrap(),
                    project: "01.Horologium".into(),
                    totals: Totals {
                        input_tokens: 5_000,
                        output_tokens: 20_000,
                        cache_creation_5m_tokens: 0,
                        cache_creation_1h_tokens: 10_000,
                        cache_read_tokens: 100_000,
                        cost_usd: 12.34,
                        records: 50,
                    },
                },
                SessionSummary {
                    session_id: "ef567890-1111-1111-1111-111111111111".into(),
                    start: Utc.with_ymd_and_hms(2026, 4, 23, 14, 0, 0).unwrap(),
                    end: Utc.with_ymd_and_hms(2026, 4, 23, 14, 45, 0).unwrap(),
                    project: "00.Agent-CLI".into(),
                    totals: Totals {
                        input_tokens: 1_000,
                        output_tokens: 5_000,
                        cache_creation_5m_tokens: 0,
                        cache_creation_1h_tokens: 0,
                        cache_read_tokens: 50_000,
                        cost_usd: 3.21,
                        records: 10,
                    },
                },
            ],
            malformed_lines: 0,
            unknown_models: BTreeMap::new(),
        }
    }

    #[test]
    fn session_table_has_header_body_and_total() {
        let s = format_sessions_table(&sample_session_report());
        let lines: Vec<&str> = s.lines().collect();
        assert!(lines[0].contains("Session"));
        assert!(lines[0].contains("Duration"));
        assert!(lines[0].contains("Project"));
        assert!(lines[0].contains("Cost"));
        // 2 sessions + header + 2 separators + TOTAL = 6
        assert_eq!(lines.len(), 6, "got:\n{}", s);
        assert!(lines[5].contains("TOTAL"));
        assert!(lines[5].contains("2 sessions"));
        assert!(lines[5].contains("$15.55"));
    }

    #[test]
    fn session_table_shows_truncated_id() {
        let s = format_sessions_table(&sample_session_report());
        assert!(s.contains("abcd1234"), "should show first 8 chars of UUID");
        assert!(
            !s.contains("abcd1234-0000"),
            "should truncate after 8 chars"
        );
    }

    #[test]
    fn session_ndjson_emits_all_fields() {
        let s = format_sessions_ndjson(&sample_session_report());
        let lines: Vec<&str> = s.lines().collect();
        assert_eq!(lines.len(), 2);
        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert!(first["session_id"]
            .as_str()
            .unwrap()
            .starts_with("abcd1234"));
        assert_eq!(first["records"], 50);
        assert_eq!(first["input_tokens"], 5_000);
        assert!(first["duration_seconds"].as_i64().unwrap() > 0);
        assert!(first["start"].as_str().is_some());
        assert_eq!(first["project"], "01.Horologium");
    }

    #[test]
    fn session_empty_report() {
        let r = SessionReport::default();
        let s = format_sessions_table(&r);
        assert!(s.contains("TOTAL"));
        assert!(s.contains("0 sessions"));
        assert!(s.contains("$0.00"));
    }

    #[test]
    fn session_ndjson_empty_is_empty() {
        let r = SessionReport::default();
        assert_eq!(format_sessions_ndjson(&r), "");
    }

    use crate::stat::aggregate::{BlockKey, BlockReport};

    fn sample_block_report() -> BlockReport {
        let d = NaiveDate::from_ymd_opt(2026, 4, 23).unwrap();
        let mut rows = BTreeMap::new();
        rows.insert(
            BlockKey { date: d, block: 0 },
            Totals {
                input_tokens: 1_000,
                output_tokens: 500,
                cost_usd: 5.00,
                records: 10,
                ..Totals::default()
            },
        );
        rows.insert(
            BlockKey { date: d, block: 2 },
            Totals {
                input_tokens: 2_000,
                output_tokens: 1_000,
                cost_usd: 10.00,
                records: 20,
                ..Totals::default()
            },
        );
        BlockReport {
            rows,
            malformed_lines: 0,
            unknown_models: BTreeMap::new(),
            divergent_duplicates: 0,
        }
    }

    #[test]
    fn block_table_structure() {
        let s = format_blocks_table(&sample_block_report());
        let lines: Vec<&str> = s.lines().collect();
        assert!(lines[0].contains("Day"));
        assert!(lines[0].contains("Window"));
        assert!(lines[0].contains("Cost"));
        // header + sep + 2 blocks + sep + TOTAL = 6
        assert_eq!(lines.len(), 6, "got:\n{}", s);
        assert!(s.contains("00-05"));
        assert!(s.contains("10-15"));
        assert!(lines[5].contains("TOTAL"));
        assert!(lines[5].contains("$15.00"));
    }

    #[test]
    fn block_ndjson_emits_window_and_block() {
        let s = format_blocks_ndjson(&sample_block_report());
        let lines: Vec<&str> = s.lines().collect();
        assert_eq!(lines.len(), 2);
        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["window"], "00-05");
        assert_eq!(first["block"], 0);
        assert_eq!(first["records"], 10);
    }

    #[test]
    fn block_empty_report() {
        let r = BlockReport::default();
        let s = format_blocks_table(&r);
        assert!(s.contains("TOTAL"));
        assert!(s.contains("$0.00"));
    }

    #[test]
    fn fmt_duration_hm_formats() {
        assert_eq!(fmt_duration_hm(0), "0m");
        assert_eq!(fmt_duration_hm(-10), "0m");
        assert_eq!(fmt_duration_hm(45 * 60), "45m");
        assert_eq!(fmt_duration_hm(2 * 3600 + 30 * 60), "2h30m");
        assert_eq!(fmt_duration_hm(25 * 3600 + 5 * 60), "25h5m");
    }
}
