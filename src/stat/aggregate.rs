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
use chrono::{DateTime, Local, NaiveDate, Timelike, Utc};
use rayon::prelude::*;
use std::collections::{BTreeMap, HashMap, HashSet};
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
    local_hour: u8,
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
        let local_dt = record.timestamp.with_timezone(&Local);
        let local_date = local_dt.date_naive();
        let local_hour = local_dt.hour() as u8;
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
            local_hour,
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

    fn finalize_daily(self) -> Report {
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

    fn finalize_blocks(self) -> BlockReport {
        let mut rows: BTreeMap<BlockKey, Totals> = BTreeMap::new();
        let mut unknown_models: BTreeMap<String, u64> = BTreeMap::new();
        for (_, s) in self.per_id {
            let block_idx = s.local_hour / 5;
            let key = BlockKey {
                date: s.date,
                block: block_idx,
            };
            rows.entry(key).or_default().merge(&s.totals);
            if s.price_state == PriceState::UnknownBillable {
                *unknown_models.entry(s.model_id).or_insert(0) += 1;
            }
        }
        BlockReport {
            rows,
            malformed_lines: self.malformed,
            unknown_models,
            divergent_duplicates: self.divergent_duplicates,
        }
    }
}

/// 5-hour block key: date + block index (0=00:00-04:59, 1=05:00-09:59, ...,
/// 4=20:00-23:59).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct BlockKey {
    pub date: NaiveDate,
    pub block: u8,
}

impl BlockKey {
    pub fn label(&self) -> &'static str {
        match self.block {
            0 => "00-05",
            1 => "05-10",
            2 => "10-15",
            3 => "15-20",
            4 => "20-00",
            _ => "??-??",
        }
    }
}

#[derive(Default, Debug)]
pub struct BlockReport {
    pub rows: BTreeMap<BlockKey, Totals>,
    pub malformed_lines: u64,
    pub unknown_models: BTreeMap<String, u64>,
    pub divergent_duplicates: u64,
}

/// One session's aggregated data.
#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub session_id: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub project: String,
    pub totals: Totals,
}

#[derive(Default, Debug)]
pub struct SessionReport {
    pub sessions: Vec<SessionSummary>,
    pub malformed_lines: u64,
    pub unknown_models: BTreeMap<String, u64>,
}

/// Aggregate one file into a SessionSummary.
///
/// Filtering semantics: all records are read unconditionally to establish
/// the true session boundaries (start, end, primary cwd). Filters are then
/// applied at the **session level** — the whole session is included or
/// excluded as a unit. This avoids truncated sessions from per-record
/// filtering (see Codex review 2026-04-25).
fn aggregate_one_session(
    path: &Path,
    filters: &Filters,
) -> (Option<SessionSummary>, u64, BTreeMap<String, u64>) {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return (None, 0, BTreeMap::new()),
    };
    let reader = BufReader::new(file);
    let mut totals = Totals::default();
    let mut malformed: u64 = 0;
    let mut unknown_models: BTreeMap<String, u64> = BTreeMap::new();
    let mut start: Option<DateTime<Utc>> = None;
    let mut end: Option<DateTime<Utc>> = None;
    let mut cwd_counts: HashMap<String, u64> = HashMap::new();
    let mut seen_ids: HashSet<String> = HashSet::new();

    for line_result in reader.lines() {
        let Ok(line) = line_result else {
            malformed += 1;
            continue;
        };
        if line.is_empty() {
            continue;
        }
        match parse_line(&line) {
            Ok(Some(record)) => {
                if !seen_ids.insert(record.message_id.clone()) {
                    continue;
                }

                if let Some(cwd) = record.cwd.as_deref() {
                    *cwd_counts.entry(cwd.to_string()).or_insert(0) += 1;
                }
                match start {
                    None => {
                        start = Some(record.timestamp);
                        end = Some(record.timestamp);
                    }
                    Some(s) => {
                        if record.timestamp < s {
                            start = Some(record.timestamp);
                        }
                        if record.timestamp > end.unwrap() {
                            end = Some(record.timestamp);
                        }
                    }
                }

                let cost = match lookup(&record.model) {
                    Some(row) => cost_for_record(&record, row),
                    None if is_silent_unknown(&record.model) => 0.0,
                    None => {
                        *unknown_models.entry(record.model.clone()).or_insert(0) += 1;
                        0.0
                    }
                };
                totals.input_tokens += record.input_tokens;
                totals.output_tokens += record.output_tokens;
                totals.cache_creation_5m_tokens += record.cache_creation_5m_tokens;
                totals.cache_creation_1h_tokens += record.cache_creation_1h_tokens;
                totals.cache_read_tokens += record.cache_read_tokens;
                totals.cost_usd += cost;
                totals.records += 1;
            }
            Ok(None) => {}
            Err(_) => malformed += 1,
        }
    }

    if totals.records == 0 {
        return (None, malformed, unknown_models);
    }

    let primary_cwd = cwd_counts
        .iter()
        .max_by_key(|(_, count)| *count)
        .map(|(cwd, _)| cwd.clone());

    let project = primary_cwd
        .as_deref()
        .map(|cwd| {
            std::path::Path::new(cwd)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(cwd)
                .to_string()
        })
        .unwrap_or_default();

    let start = start.unwrap();
    let start_date = start.with_timezone(&Local).date_naive();

    // Session-level filters: include/exclude the whole session as a unit.
    if let Some(since) = filters.since {
        if start_date < since {
            return (None, malformed, unknown_models);
        }
    }
    if let Some(until) = filters.until {
        if start_date > until {
            return (None, malformed, unknown_models);
        }
    }
    if let Some(needle) = filters.project_substring.as_deref() {
        if !primary_cwd
            .as_deref()
            .is_some_and(|cwd| cwd.contains(needle))
        {
            return (None, malformed, unknown_models);
        }
    }

    let session_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    (
        Some(SessionSummary {
            session_id,
            start,
            end: end.unwrap(),
            project,
            totals,
        }),
        malformed,
        unknown_models,
    )
}

/// Aggregate all JSONL files as individual sessions.
pub fn aggregate_sessions(paths: &[PathBuf], filters: &Filters) -> SessionReport {
    let results: Vec<_> = paths
        .par_iter()
        .map(|path| aggregate_one_session(path, filters))
        .collect();

    let mut sessions = Vec::new();
    let mut malformed_lines = 0u64;
    let mut unknown_models: BTreeMap<String, u64> = BTreeMap::new();

    for (summary, mal, unk) in results {
        if let Some(s) = summary {
            sessions.push(s);
        }
        malformed_lines += mal;
        for (model, count) in unk {
            *unknown_models.entry(model).or_insert(0) += count;
        }
    }

    sessions.sort_by_key(|s| s.start);

    SessionReport {
        sessions,
        malformed_lines,
        unknown_models,
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
        .finalize_daily()
}

/// Same dedup pipeline as `aggregate_daily`, but buckets into 5-hour blocks.
pub fn aggregate_blocks(paths: &[PathBuf], filters: &Filters) -> BlockReport {
    paths
        .par_iter()
        .fold(LocalAccumulator::default, |mut acc, path| {
            acc.consume_file(path, filters);
            acc
        })
        .reduce(LocalAccumulator::default, LocalAccumulator::merge)
        .finalize_blocks()
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

    // --- session aggregation tests ---

    #[test]
    fn session_aggregates_single_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_jsonl(
            tmp.path(),
            "aaaaaaaa-0000-0000-0000-000000000000.jsonl",
            &[
                &assistant(
                    "m1",
                    "claude-opus-4-7",
                    "2026-04-05T08:00:00Z",
                    "/proj/foo",
                    100,
                    50,
                ),
                &assistant(
                    "m2",
                    "claude-opus-4-7",
                    "2026-04-05T10:30:00Z",
                    "/proj/foo",
                    200,
                    100,
                ),
            ],
        );
        let r = aggregate_sessions(&[path], &Filters::default());
        assert_eq!(r.sessions.len(), 1);
        let s = &r.sessions[0];
        assert!(s.session_id.starts_with("aaaaaaaa"));
        assert_eq!(s.totals.records, 2);
        assert_eq!(s.totals.input_tokens, 300);
        assert_eq!(s.project, "foo");
        let duration = (s.end - s.start).num_seconds();
        assert_eq!(duration, 9000); // 2.5h
    }

    #[test]
    fn session_multiple_files_are_separate() {
        let tmp = tempfile::tempdir().unwrap();
        let a = write_jsonl(
            tmp.path(),
            "aaaa0000.jsonl",
            &[&assistant(
                "m1",
                "claude-opus-4-7",
                "2026-04-05T08:00:00Z",
                "/a",
                100,
                50,
            )],
        );
        let b = write_jsonl(
            tmp.path(),
            "bbbb0000.jsonl",
            &[&assistant(
                "m2",
                "claude-opus-4-7",
                "2026-04-05T09:00:00Z",
                "/b",
                200,
                100,
            )],
        );
        let r = aggregate_sessions(&[a, b], &Filters::default());
        assert_eq!(r.sessions.len(), 2);
    }

    #[test]
    fn session_empty_after_filter_is_excluded() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_jsonl(
            tmp.path(),
            "sess.jsonl",
            &[&assistant(
                "m1",
                "claude-opus-4-7",
                "2026-04-05T08:00:00Z",
                "/proj/a",
                100,
                50,
            )],
        );
        let filters = Filters {
            project_substring: Some("nonexistent".into()),
            ..Default::default()
        };
        let r = aggregate_sessions(&[path], &filters);
        assert_eq!(r.sessions.len(), 0);
    }

    #[test]
    fn session_picks_most_common_cwd_as_project() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_jsonl(
            tmp.path(),
            "sess.jsonl",
            &[
                &assistant(
                    "m1",
                    "claude-opus-4-7",
                    "2026-04-05T08:00:00Z",
                    "/proj/alpha",
                    10,
                    10,
                ),
                &assistant(
                    "m2",
                    "claude-opus-4-7",
                    "2026-04-05T08:01:00Z",
                    "/proj/beta",
                    10,
                    10,
                ),
                &assistant(
                    "m3",
                    "claude-opus-4-7",
                    "2026-04-05T08:02:00Z",
                    "/proj/beta",
                    10,
                    10,
                ),
            ],
        );
        let r = aggregate_sessions(&[path], &Filters::default());
        assert_eq!(r.sessions[0].project, "beta");
    }

    #[test]
    fn session_sorted_chronologically_by_default() {
        let tmp = tempfile::tempdir().unwrap();
        let a = write_jsonl(
            tmp.path(),
            "late.jsonl",
            &[&assistant(
                "m1",
                "claude-opus-4-7",
                "2026-04-05T20:00:00Z",
                "/p",
                100,
                50,
            )],
        );
        let b = write_jsonl(
            tmp.path(),
            "early.jsonl",
            &[&assistant(
                "m2",
                "claude-opus-4-7",
                "2026-04-05T08:00:00Z",
                "/p",
                100,
                50,
            )],
        );
        let r = aggregate_sessions(&[a, b], &Filters::default());
        assert!(r.sessions[0].start < r.sessions[1].start);
    }

    #[test]
    fn session_empty_paths_is_empty() {
        let r = aggregate_sessions(&[], &Filters::default());
        assert!(r.sessions.is_empty());
        assert_eq!(r.malformed_lines, 0);
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

    // --- blocks aggregation tests ---

    fn utc_to_block_key(utc_ts: &str) -> BlockKey {
        let dt = DateTime::parse_from_rfc3339(utc_ts)
            .unwrap()
            .with_timezone(&Utc);
        let local = dt.with_timezone(&Local);
        BlockKey {
            date: local.date_naive(),
            block: (local.hour() / 5) as u8,
        }
    }

    #[test]
    fn blocks_buckets_by_5h_window() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_jsonl(
            tmp.path(),
            "s.jsonl",
            &[
                &assistant(
                    "m1",
                    "claude-opus-4-7",
                    "2026-04-05T02:00:00Z",
                    "/p",
                    100,
                    50,
                ),
                &assistant(
                    "m2",
                    "claude-opus-4-7",
                    "2026-04-05T03:00:00Z",
                    "/p",
                    100,
                    50,
                ),
                &assistant(
                    "m3",
                    "claude-opus-4-7",
                    "2026-04-05T07:00:00Z",
                    "/p",
                    200,
                    100,
                ),
                &assistant(
                    "m4",
                    "claude-opus-4-7",
                    "2026-04-05T14:00:00Z",
                    "/p",
                    300,
                    150,
                ),
                &assistant(
                    "m5",
                    "claude-opus-4-7",
                    "2026-04-05T22:00:00Z",
                    "/p",
                    400,
                    200,
                ),
            ],
        );
        let r = aggregate_blocks(&[path], &Filters::default());

        let k1 = utc_to_block_key("2026-04-05T02:00:00Z");
        let k2 = utc_to_block_key("2026-04-05T03:00:00Z");
        if k1 == k2 {
            assert_eq!(r.rows[&k1].records, 2);
            assert_eq!(r.rows[&k1].input_tokens, 200);
        } else {
            assert_eq!(r.rows[&k1].records, 1);
            assert_eq!(r.rows[&k2].records, 1);
        }

        let k3 = utc_to_block_key("2026-04-05T07:00:00Z");
        assert_eq!(r.rows[&k3].records, 1);
        let k4 = utc_to_block_key("2026-04-05T14:00:00Z");
        assert_eq!(r.rows[&k4].records, 1);
        let k5 = utc_to_block_key("2026-04-05T22:00:00Z");
        assert_eq!(r.rows[&k5].records, 1);

        let total: u64 = r.rows.values().map(|t| t.records).sum();
        assert_eq!(total, 5);
    }

    #[test]
    fn blocks_label_mapping() {
        let d = NaiveDate::from_ymd_opt(2026, 4, 5).unwrap();
        assert_eq!((BlockKey { date: d, block: 0 }).label(), "00-05");
        assert_eq!((BlockKey { date: d, block: 1 }).label(), "05-10");
        assert_eq!((BlockKey { date: d, block: 2 }).label(), "10-15");
        assert_eq!((BlockKey { date: d, block: 3 }).label(), "15-20");
        assert_eq!((BlockKey { date: d, block: 4 }).label(), "20-00");
    }

    #[test]
    fn blocks_cross_file_dedup() {
        let tmp = tempfile::tempdir().unwrap();
        let a = write_jsonl(
            tmp.path(),
            "a.jsonl",
            &[&assistant(
                "dup",
                "claude-opus-4-7",
                "2026-04-05T08:00:00Z",
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
                "2026-04-05T08:00:00Z",
                "/p",
                100,
                50,
            )],
        );
        let r = aggregate_blocks(&[a, b], &Filters::default());
        let key = utc_to_block_key("2026-04-05T08:00:00Z");
        assert_eq!(r.rows[&key].records, 1);
    }

    #[test]
    fn blocks_empty_paths_is_empty() {
        let r = aggregate_blocks(&[], &Filters::default());
        assert!(r.rows.is_empty());
    }
}
