//! Render an aggregated `Report` as either a human-readable table or
//! one JSON object per day (NDJSON).
//!
//! The table uses only ASCII padding — no `prettytable` / `tabled`
//! dependency — so the release binary stays minimal. Columns are sized
//! to the widest cell so rollups from small to production-scale corpora
//! all align without wrapping.

#![allow(dead_code)] // wired into mod.rs in the next commit

use super::aggregate::Report;

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
        out.push_str(&format!("note: {} malformed line(s) skipped\n", report.malformed_lines));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stat::aggregate::{Report, Totals};
    use chrono::NaiveDate;
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
}
