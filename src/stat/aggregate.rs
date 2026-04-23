//! Per-day rollup of deduplicated usage records.
//!
//! Each rayon worker opens one JSONL file, parses each line, filters and
//! prices the surviving records, and stores one `PerIdSummary` per
//! `message.id` into a local map. The reduce phase unions the per-id maps
//! (first-writer wins on collision), and only then do we fold into the
//! final `BTreeMap<date, Totals>`.
//!
//! Why two-phase instead of per-file-bucketed: dedup across files matters
//! for correctness (Claude Code *shouldn't* write the same `message.id`
//! to two JSONL files, but if it ever did — or if a file is accidentally
//! duplicated by backup tooling — naive per-file aggregation would double
//! count). Since per-id dedup happens before bucketing, the unknown-model
//! warning counts stay consistent with the row counts.
//!
//! Divergent duplicates: byte-identical duplicates are the expected shape,
//! but a corrupted/merged corpus could produce two records sharing an id
//! with different payloads. rayon's reduce ordering isn't deterministic,
//! so which writer wins would silently depend on thread scheduling. We
//! instead compare on every collision and count divergences in
//! `Report::divergent_duplicates`; the first-seen summary still wins (so
//! aggregates remain stable within a run), and the counter surfaces the
//! anomaly via stderr.

use super::pricing::{cost_for_record, is_silent_unknown, lookup};
use super::record::{parse_line, Record};
use chrono::{Local, NaiveDate};
use rayon::prelude::*;
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

/// Accumulated token and cost totals for one bucket (e.g. a calendar day).
#[derive(Default, Clone, Debug, PartialEq)]
pub struct Totals {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_5m_tokens: u64,
    pub cache_creation_1h_tokens: u64,
    pub cache_read_tokens: u64,
    pub cost_usd: f64,
    pub records: u64,
}

impl Totals {
    fn merge(&mut self, other: &Totals) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_creation_5m_tokens += other.cache_creation_5m_tokens;
        self.cache_creation_1h_tokens += other.cache_creation_1h_tokens;
        self.cache_read_tokens += other.cache_read_tokens;
        self.cost_usd += other.cost_usd;
        self.records += other.records;
    }
}

/// Bucket key = calendar day in local timezone. `BTreeMap<BucketKey, Totals>`
/// gives a deterministic ordered output without a separate sort pass.
pub type BucketKey = NaiveDate;

#[derive(Default, Clone, Debug)]
pub struct Filters {
    pub since: Option<NaiveDate>,
    pub until: Option<NaiveDate>,
    /// Case-sensitive substring matched against each record's `cwd`.
    /// Records with no `cwd` never match; a `None` filter accepts all.
    pub project_substring: Option<String>,
}

#[derive(Default, Debug)]
pub struct Report {
    pub rows: BTreeMap<BucketKey, Totals>,
    pub malformed_lines: u64,
    /// Model-id → count of unique records using that model. Token counts
    /// are still included in `rows`; only cost contribution is zero.
    pub unknown_models: BTreeMap<String, u64>,
    /// Count of dedup collisions where a second occurrence of the same
    /// `message.id` carried a payload that disagreed with the first
    /// occurrence (different date / model / tokens). The first-seen copy
    /// is kept in the totals; this counter exposes the anomaly so the
    /// user can investigate the underlying log corruption.
    pub divergent_duplicates: u64,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum PriceState {
    /// Model matched in the embedded pricing snapshot.
    Priced,
    /// Model absent from the snapshot and not on the silent-unknown list —
    /// surfaced in `Report::unknown_models` so the user can investigate.
    UnknownBillable,
    /// Model absent from the snapshot but on the silent-unknown list
    /// (e.g. `<synthetic>` sentinels). Tokens counted, cost 0, no warning.
    SilentUnknown,
}

/// A single-record contribution keyed by `message.id`. Kept whole through
/// the reduce phase so dedup is authoritative before bucket aggregation
/// and warning counts line up with row counts.
#[derive(Clone, PartialEq, Debug)]
struct PerIdSummary {
    date: NaiveDate,
    totals: Totals, // records=1 when filled from a live Record
    model_id: String,
    price_state: PriceState,
}

#[derive(Default)]
struct LocalAccumulator {
    per_id: HashMap<String, PerIdSummary>,
    malformed: u64,
    divergent_duplicates: u64,
}

impl LocalAccumulator {
    fn consume_record(&mut self, record: Record, filters: &Filters) {
        let local_date = record.timestamp.with_timezone(&Local).date_naive();
        if let Some(since) = filters.since {
            if local_date < since {
                return;
            }
        }
        if let Some(until) = filters.until {
            if local_date > until {
                return;
            }
        }
        if let Some(needle) = filters.project_substring.as_deref() {
            if !record.cwd.as_deref().unwrap_or("").contains(needle) {
                return;
            }
        }

        let (cost, price_state) = match lookup(&record.model) {
            Some(row) => (cost_for_record(&record, row), PriceState::Priced),
            None if is_silent_unknown(&record.model) => (0.0, PriceState::SilentUnknown),
            None => (0.0, PriceState::UnknownBillable),
        };
        let totals = Totals {
            input_tokens: record.input_tokens,
            output_tokens: record.output_tokens,
            cache_creation_5m_tokens: record.cache_creation_5m_tokens,
            cache_creation_1h_tokens: record.cache_creation_1h_tokens,
            cache_read_tokens: record.cache_read_tokens,
            cost_usd: cost,
            records: 1,
        };
        let new_summary = PerIdSummary {
            date: local_date,
            totals,
            model_id: record.model,
            price_state,
        };

        use std::collections::hash_map::Entry;
        match self.per_id.entry(record.message_id.clone()) {
            Entry::Vacant(slot) => {
                slot.insert(new_summary);
            }
            Entry::Occupied(existing) => {
                if *existing.get() != new_summary {
                    self.divergent_duplicates += 1;
                }
                // Byte-identical duplicates are silently dropped.
                // Divergent duplicates: keep first-seen (stable within a
                // run), surface the anomaly via the counter.
            }
        }
    }

    fn consume_file(&mut self, path: &Path, filters: &Filters) {
        let file = match File::open(path) {
            Ok(f) => f,
            Err(_) => return,
        };
        let reader = BufReader::new(file);
        for line_result in reader.lines() {
            let Ok(line) = line_result else {
                self.malformed += 1;
                continue;
            };
            if line.is_empty() {
                continue;
            }
            match parse_line(&line) {
                Ok(Some(record)) => self.consume_record(record, filters),
                Ok(None) => {}
                Err(_) => self.malformed += 1,
            }
        }
    }

    fn merge(mut self, other: Self) -> Self {
        self.per_id.reserve(other.per_id.len());
        for (id, summary) in other.per_id {
            use std::collections::hash_map::Entry;
            match self.per_id.entry(id) {
                Entry::Vacant(slot) => {
                    slot.insert(summary);
                }
                Entry::Occupied(existing) => {
                    if *existing.get() != summary {
                        self.divergent_duplicates += 1;
                    }
                }
            }
        }
        self.malformed += other.malformed;
        self.divergent_duplicates += other.divergent_duplicates;
        self
    }

    fn finalize(self) -> Report {
        let mut rows: BTreeMap<NaiveDate, Totals> = BTreeMap::new();
        let mut unknown_models: BTreeMap<String, u64> = BTreeMap::new();
        for (_, s) in self.per_id {
            rows.entry(s.date).or_default().merge(&s.totals);
            if s.price_state == PriceState::UnknownBillable {
                *unknown_models.entry(s.model_id).or_insert(0) += 1;
            }
        }
        Report {
            rows,
            malformed_lines: self.malformed,
            unknown_models,
            divergent_duplicates: self.divergent_duplicates,
        }
    }
}

/// Process every path in parallel via rayon, then reduce + finalize into
/// a single `Report`. Caller is responsible for discovering paths
/// (see `walker::find_jsonl`) and for supplying filters consistently.
pub fn aggregate_daily(paths: &[PathBuf], filters: &Filters) -> Report {
    paths
        .par_iter()
        .fold(LocalAccumulator::default, |mut acc, path| {
            acc.consume_file(path, filters);
            acc
        })
        .reduce(LocalAccumulator::default, LocalAccumulator::merge)
        .finalize()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    fn write_jsonl(dir: &Path, name: &str, lines: &[&str]) -> PathBuf {
        let path = dir.join(name);
        let mut f = fs::File::create(&path).unwrap();
        for line in lines {
            writeln!(f, "{}", line).unwrap();
        }
        path
    }

    fn assistant(
        msg_id: &str,
        model: &str,
        ts: &str,
        cwd: &str,
        input: u64,
        output: u64,
    ) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{}","cwd":"{}","message":{{"id":"{}","model":"{}","usage":{{"input_tokens":{},"output_tokens":{},"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}}}}"#,
            ts, cwd, msg_id, model, input, output
        )
    }

    #[test]
    fn aggregates_single_file_by_day() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_jsonl(
            tmp.path(),
            "s.jsonl",
            &[
                &assistant(
                    "m1",
                    "claude-opus-4-7",
                    "2026-04-05T12:00:00Z",
                    "/a",
                    1000,
                    500,
                ),
                &assistant(
                    "m2",
                    "claude-opus-4-7",
                    "2026-04-05T13:00:00Z",
                    "/a",
                    2000,
                    1000,
                ),
                &assistant(
                    "m3",
                    "claude-opus-4-7",
                    "2026-04-06T12:00:00Z",
                    "/a",
                    500,
                    250,
                ),
            ],
        );
        let r = aggregate_daily(&[path], &Filters::default());
        assert_eq!(r.rows.len(), 2);
        let d5 = NaiveDate::from_ymd_opt(2026, 4, 5).unwrap();
        let d6 = NaiveDate::from_ymd_opt(2026, 4, 6).unwrap();
        assert_eq!(r.rows[&d5].records, 2);
        assert_eq!(r.rows[&d5].input_tokens, 3000);
        assert_eq!(r.rows[&d5].output_tokens, 1500);
        assert_eq!(r.rows[&d6].records, 1);
        assert_eq!(r.malformed_lines, 0);
    }

    #[test]
    fn dedups_across_files() {
        let tmp = tempfile::tempdir().unwrap();
        let a = write_jsonl(
            tmp.path(),
            "a.jsonl",
            &[&assistant(
                "shared",
                "claude-opus-4-7",
                "2026-04-05T12:00:00Z",
                "/p",
                100,
                50,
            )],
        );
        let b = write_jsonl(
            tmp.path(),
            "b.jsonl",
            &[
                &assistant(
                    "shared",
                    "claude-opus-4-7",
                    "2026-04-05T12:00:00Z",
                    "/p",
                    100,
                    50,
                ),
                &assistant(
                    "unique",
                    "claude-opus-4-7",
                    "2026-04-05T12:00:00Z",
                    "/p",
                    200,
                    100,
                ),
            ],
        );
        let r = aggregate_daily(&[a, b], &Filters::default());
        let d = NaiveDate::from_ymd_opt(2026, 4, 5).unwrap();
        assert_eq!(r.rows[&d].records, 2);
        assert_eq!(r.rows[&d].input_tokens, 300);
    }

    #[test]
    fn malformed_lines_do_not_abort_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_jsonl(
            tmp.path(),
            "s.jsonl",
            &[
                "not json",
                &assistant(
                    "m1",
                    "claude-opus-4-7",
                    "2026-04-05T12:00:00Z",
                    "/p",
                    1000,
                    500,
                ),
                r#"{"type":"user","content":"ok"}"#,
                &assistant(
                    "m2",
                    "claude-opus-4-7",
                    "2026-04-05T13:00:00Z",
                    "/p",
                    2000,
                    1000,
                ),
            ],
        );
        let r = aggregate_daily(&[path], &Filters::default());
        let d = NaiveDate::from_ymd_opt(2026, 4, 5).unwrap();
        assert_eq!(r.rows[&d].records, 2);
        assert_eq!(r.malformed_lines, 1);
    }

    #[test]
    fn unknown_model_tokens_counted_but_cost_zero() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_jsonl(
            tmp.path(),
            "s.jsonl",
            &[&assistant(
                "m1",
                "claude-mystery-99",
                "2026-04-05T12:00:00Z",
                "/p",
                1_000_000,
                1_000_000,
            )],
        );
        let r = aggregate_daily(&[path], &Filters::default());
        let d = NaiveDate::from_ymd_opt(2026, 4, 5).unwrap();
        assert_eq!(r.rows[&d].input_tokens, 1_000_000);
        assert_eq!(r.rows[&d].cost_usd, 0.0);
        assert_eq!(r.unknown_models.get("claude-mystery-99"), Some(&1));
    }

    #[test]
    fn unknown_model_count_dedups_along_with_records() {
        // If the same unknown-model id appears in two files, it should
        // still only count once — both in records and in unknown_models.
        let tmp = tempfile::tempdir().unwrap();
        let a = write_jsonl(
            tmp.path(),
            "a.jsonl",
            &[&assistant(
                "dup",
                "claude-mystery-99",
                "2026-04-05T12:00:00Z",
                "/p",
                100,
                100,
            )],
        );
        let b = write_jsonl(
            tmp.path(),
            "b.jsonl",
            &[&assistant(
                "dup",
                "claude-mystery-99",
                "2026-04-05T12:00:00Z",
                "/p",
                100,
                100,
            )],
        );
        let r = aggregate_daily(&[a, b], &Filters::default());
        assert_eq!(r.unknown_models.get("claude-mystery-99"), Some(&1));
    }

    #[test]
    fn synthetic_sentinel_is_silent() {
        // <synthetic> appears in real Claude Code logs on tool-use rows.
        // Tokens should be counted, cost 0, but it must NOT appear in
        // unknown_models (that's noise to the user).
        let tmp = tempfile::tempdir().unwrap();
        let path = write_jsonl(
            tmp.path(),
            "s.jsonl",
            &[
                &assistant("m1", "<synthetic>", "2026-04-05T12:00:00Z", "/p", 500, 100),
                &assistant(
                    "m2",
                    "claude-mystery-99",
                    "2026-04-05T12:00:00Z",
                    "/p",
                    300,
                    50,
                ),
            ],
        );
        let r = aggregate_daily(&[path], &Filters::default());
        let d = NaiveDate::from_ymd_opt(2026, 4, 5).unwrap();
        // Tokens from both records are counted.
        assert_eq!(r.rows[&d].input_tokens, 800);
        assert_eq!(r.rows[&d].records, 2);
        // <synthetic> is silent; mystery-99 warns.
        assert!(
            !r.unknown_models.contains_key("<synthetic>"),
            "got: {:?}",
            r.unknown_models
        );
        assert_eq!(r.unknown_models.get("claude-mystery-99"), Some(&1));
    }

    #[test]
    fn project_substring_filter() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_jsonl(
            tmp.path(),
            "s.jsonl",
            &[
                &assistant(
                    "m1",
                    "claude-opus-4-7",
                    "2026-04-05T12:00:00Z",
                    "/home/alice/proj-a",
                    100,
                    100,
                ),
                &assistant(
                    "m2",
                    "claude-opus-4-7",
                    "2026-04-05T12:00:00Z",
                    "/home/alice/proj-b",
                    200,
                    200,
                ),
            ],
        );
        let filters = Filters {
            project_substring: Some("proj-a".into()),
            ..Default::default()
        };
        let r = aggregate_daily(&[path], &filters);
        let d = NaiveDate::from_ymd_opt(2026, 4, 5).unwrap();
        assert_eq!(r.rows[&d].records, 1);
        assert_eq!(r.rows[&d].input_tokens, 100);
    }

    #[test]
    fn since_until_filter() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_jsonl(
            tmp.path(),
            "s.jsonl",
            &[
                &assistant(
                    "m1",
                    "claude-opus-4-7",
                    "2026-04-03T12:00:00Z",
                    "/p",
                    100,
                    100,
                ),
                &assistant(
                    "m2",
                    "claude-opus-4-7",
                    "2026-04-05T12:00:00Z",
                    "/p",
                    100,
                    100,
                ),
                &assistant(
                    "m3",
                    "claude-opus-4-7",
                    "2026-04-07T12:00:00Z",
                    "/p",
                    100,
                    100,
                ),
            ],
        );
        let filters = Filters {
            since: Some(NaiveDate::from_ymd_opt(2026, 4, 4).unwrap()),
            until: Some(NaiveDate::from_ymd_opt(2026, 4, 6).unwrap()),
            ..Default::default()
        };
        let r = aggregate_daily(&[path], &filters);
        assert_eq!(r.rows.len(), 1);
        assert!(r
            .rows
            .contains_key(&NaiveDate::from_ymd_opt(2026, 4, 5).unwrap()));
    }

    #[test]
    fn cost_is_computed_when_model_is_known() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_jsonl(
            tmp.path(),
            "s.jsonl",
            &[&assistant(
                "m1",
                "claude-opus-4-7",
                "2026-04-05T12:00:00Z",
                "/p",
                1_000_000,
                1_000_000,
            )],
        );
        let r = aggregate_daily(&[path], &Filters::default());
        let d = NaiveDate::from_ymd_opt(2026, 4, 5).unwrap();
        assert!((r.rows[&d].cost_usd - 30.0).abs() < 1e-6);
    }

    #[test]
    fn empty_paths_is_empty_report() {
        let r = aggregate_daily(&[], &Filters::default());
        assert!(r.rows.is_empty());
        assert_eq!(r.malformed_lines, 0);
        assert!(r.unknown_models.is_empty());
        assert_eq!(r.divergent_duplicates, 0);
    }

    #[test]
    fn byte_identical_duplicates_do_not_flag_divergence() {
        // Two files with the exact same record — the common backup/rsync
        // case. Must stay at records=1 with zero divergence flags.
        let tmp = tempfile::tempdir().unwrap();
        let a = write_jsonl(
            tmp.path(),
            "a.jsonl",
            &[&assistant(
                "dup",
                "claude-opus-4-7",
                "2026-04-05T12:00:00Z",
                "/p",
                100,
                50,
            )],
        );
        let b = write_jsonl(
            tmp.path(),
            "b.jsonl",
            &[&assistant(
                "dup",
                "claude-opus-4-7",
                "2026-04-05T12:00:00Z",
                "/p",
                100,
                50,
            )],
        );
        let r = aggregate_daily(&[a, b], &Filters::default());
        let d = NaiveDate::from_ymd_opt(2026, 4, 5).unwrap();
        assert_eq!(r.rows[&d].records, 1);
        assert_eq!(r.divergent_duplicates, 0);
    }

    #[test]
    fn divergent_duplicates_are_counted_but_first_seen_wins_within_file() {
        // Two records in the same file share an id but disagree on
        // tokens — simulates a corrupted session log. The counter must
        // fire and the first record must win in totals.
        let tmp = tempfile::tempdir().unwrap();
        let path = write_jsonl(
            tmp.path(),
            "s.jsonl",
            &[
                &assistant(
                    "x",
                    "claude-opus-4-7",
                    "2026-04-05T12:00:00Z",
                    "/p",
                    100,
                    50,
                ),
                &assistant(
                    "x",
                    "claude-opus-4-7",
                    "2026-04-05T12:00:00Z",
                    "/p",
                    999,
                    999,
                ),
            ],
        );
        let r = aggregate_daily(&[path], &Filters::default());
        let d = NaiveDate::from_ymd_opt(2026, 4, 5).unwrap();
        assert_eq!(r.rows[&d].records, 1);
        assert_eq!(r.rows[&d].input_tokens, 100, "first-seen should win");
        assert_eq!(r.divergent_duplicates, 1);
    }

    #[test]
    fn divergent_duplicates_across_files_are_counted() {
        // Same id in two files but with different token counts. Even
        // though rayon reduce ordering is non-deterministic, the
        // divergence counter is deterministic because it fires on every
        // collision regardless of which side wins.
        let tmp = tempfile::tempdir().unwrap();
        let a = write_jsonl(
            tmp.path(),
            "a.jsonl",
            &[&assistant(
                "x",
                "claude-opus-4-7",
                "2026-04-05T12:00:00Z",
                "/p",
                100,
                50,
            )],
        );
        let b = write_jsonl(
            tmp.path(),
            "b.jsonl",
            &[&assistant(
                "x",
                "claude-opus-4-7",
                "2026-04-05T12:00:00Z",
                "/p",
                200,
                50,
            )],
        );
        let r = aggregate_daily(&[a, b], &Filters::default());
        let d = NaiveDate::from_ymd_opt(2026, 4, 5).unwrap();
        assert_eq!(r.rows[&d].records, 1);
        assert_eq!(r.divergent_duplicates, 1);
    }
}
