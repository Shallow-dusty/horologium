//! Per-day rollup of deduplicated usage records.
//!
//! Each rayon worker opens JSONL files and first collapses Claude Code's
//! intermediate/final snapshots by `message.id`: prefer a row carrying
//! `stop_reason`, otherwise take the largest `output_tokens` value (the
//! same policy used by CC Switch). Only the selected row is filtered,
//! priced, and stored as a `PerIdSummary`; the reduce phase then unions
//! per-id maps across files before bucketing into reports.
//!
//! Why two-phase instead of per-file-bucketed: dedup across files matters
//! for backup / rsync copies. Compatible snapshots are resolved to the
//! most complete row both within and across files, keeping unknown-model
//! counts aligned with report rows and avoiding the old systematic
//! first-snapshot undercount.
//!
//! Divergent duplicates now mean a genuine incompatible id collision:
//! model, input tokens, or cache-read tokens disagree. Expected streaming
//! changes (output, stop_reason, timestamp, late cache-creation fields) do
//! not increment the counter. Incompatible collisions preserve first-seen
//! and remain visible via stderr for investigation.

use crate::pricing::{cost_for_record, is_silent_unknown, lookup};
use crate::record::{ParserState, Record};
use crate::source::Source;
use chrono::{DateTime, Local, NaiveDate, Timelike, Utc};
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
    pub fn merge(&mut self, other: &Totals) {
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

/// Heatmap view granularity. `Year` / `Month` / `Week` render one cell per
/// calendar day; `Day` renders one cell per local hour.
#[derive(Clone, Copy, PartialEq, Eq, Debug, clap::ValueEnum)]
pub enum HeatmapGranularity {
    /// GitHub-style 53-week grid (one cell per day).
    Year,
    /// Calendar month grid (one cell per day).
    Month,
    /// Single row of seven day cells.
    Week,
    /// Single row of 24 hourly cells.
    Day,
}

impl HeatmapGranularity {
    pub fn label(self) -> &'static str {
        match self {
            HeatmapGranularity::Year => "year",
            HeatmapGranularity::Month => "month",
            HeatmapGranularity::Week => "week",
            HeatmapGranularity::Day => "day",
        }
    }
}

/// Which value drives cell color intensity.
#[derive(Clone, Copy, PartialEq, Eq, Debug, clap::ValueEnum)]
pub enum HeatmapMetric {
    /// Cost in USD (default).
    Cost,
    /// Total tokens (input + output + cache reads + cache writes).
    Tokens,
}

/// One heatmap cell. For day granularity `hour` is the local hour
/// (0-23); for coarser granularities it is always 0 and ignored.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub struct HeatCell {
    pub date: NaiveDate,
    pub hour: u8,
}

impl HeatCell {
    pub fn day(date: NaiveDate) -> Self {
        HeatCell { date, hour: 0 }
    }

    pub fn hour(date: NaiveDate, hour: u8) -> Self {
        HeatCell { date, hour }
    }
}

/// Aggregate heatmap cells. Same dedup pipeline as `Report`; rows are
/// keyed by [`HeatCell`] so the renderer can lay out any granularity.
#[derive(Default, Debug)]
pub struct HeatmapReport {
    pub cells: BTreeMap<HeatCell, Totals>,
    pub malformed_lines: u64,
    pub divergent_duplicates: u64,
}

impl Totals {
    /// All tokens moved through this bucket (cost-relevant weights).
    pub fn all_tokens(&self) -> u64 {
        self.input_tokens
            + self.output_tokens
            + self.cache_read_tokens
            + self.cache_creation_5m_tokens
            + self.cache_creation_1h_tokens
    }
}

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
    /// Count of genuine id collisions where model, input tokens, or
    /// cache-read tokens conflict. Expected intermediate/final streaming
    /// snapshots are merged to the most complete row and do not count.
    /// Incompatible collisions preserve first-seen for investigation.
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

/// True when two rows are snapshots of the same billable request rather
/// than a genuine id collision. Claude Code writes intermediate and final
/// assistant snapshots under one `message.id`; output / stop_reason /
/// timestamp differ, and cache-creation tokens may only materialize on
/// the final row. Model, input, and cache-read are stable from request
/// start, so they form the compatibility key.
fn same_usage_identity(a: &Record, b: &Record) -> bool {
    a.model == b.model
        && a.input_tokens == b.input_tokens
        && a.cache_read_tokens == b.cache_read_tokens
}

/// CC Switch-compatible snapshot preference: a final row carrying
/// `stop_reason` beats an intermediate row; when both have (or both lack)
/// it, the larger output count wins. Input/cache values are already equal
/// because the caller first checks [`same_usage_identity`].
fn candidate_is_better(existing: &Record, candidate: &Record) -> bool {
    match (
        existing.stop_reason.is_some(),
        candidate.stop_reason.is_some(),
    ) {
        (false, true) => true,
        (true, false) => false,
        _ => candidate.output_tokens > existing.output_tokens,
    }
}

#[derive(Default)]
struct ParsedFile {
    records: Vec<Record>,
    malformed: u64,
    divergent: u64,
}

/// Parse one JSONL file and collapse Claude Code's intermediate/final
/// snapshots before aggregation. Expected streaming snapshots are merged
/// silently; incompatible rows sharing an id keep first-seen and increment
/// `divergent` so genuine collisions remain visible.
fn read_selected_records(path: &Path, source: Source) -> ParsedFile {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(_) => return ParsedFile::default(),
    };
    let mut parser = ParserState::new(source);
    let mut per_id: HashMap<String, Record> = HashMap::new();
    let mut malformed = 0u64;
    let mut divergent = 0u64;

    for line_result in BufReader::new(file).lines() {
        let Ok(line) = line_result else {
            malformed += 1;
            continue;
        };
        if line.is_empty() {
            continue;
        }
        let record = match parser.parse_line(&line) {
            Ok(Some(record)) => record,
            Ok(None) => continue,
            Err(_) => {
                malformed += 1;
                continue;
            }
        };

        use std::collections::hash_map::Entry;
        match per_id.entry(record.message_id.clone()) {
            Entry::Vacant(slot) => {
                slot.insert(record);
            }
            Entry::Occupied(mut existing) => {
                if same_usage_identity(existing.get(), &record) {
                    if candidate_is_better(existing.get(), &record) {
                        existing.insert(record);
                    }
                } else {
                    divergent += 1;
                    // Genuine collision / inconsistent request metadata:
                    // preserve first-seen and surface the anomaly.
                }
            }
        }
    }

    ParsedFile {
        records: per_id.into_values().collect(),
        malformed,
        divergent,
    }
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
    stop_reason_present: bool,
}

impl PerIdSummary {
    fn same_usage_identity(&self, other: &Self) -> bool {
        self.model_id == other.model_id
            && self.totals.input_tokens == other.totals.input_tokens
            && self.totals.cache_read_tokens == other.totals.cache_read_tokens
    }

    fn candidate_is_better(&self, candidate: &Self) -> bool {
        match (self.stop_reason_present, candidate.stop_reason_present) {
            (false, true) => true,
            (true, false) => false,
            _ => candidate.totals.output_tokens > self.totals.output_tokens,
        }
    }
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
            stop_reason_present: record.stop_reason.is_some(),
        };

        use std::collections::hash_map::Entry;
        match self.per_id.entry(record.message_id.clone()) {
            Entry::Vacant(slot) => {
                slot.insert(new_summary);
            }
            Entry::Occupied(mut existing) => {
                if existing.get().same_usage_identity(&new_summary) {
                    if existing.get().candidate_is_better(&new_summary) {
                        existing.insert(new_summary);
                    }
                } else {
                    self.divergent_duplicates += 1;
                    // Incompatible metadata under one id: preserve
                    // first-seen and surface the anomaly.
                }
            }
        }
    }

    fn consume_file(&mut self, path: &Path, filters: &Filters, source: Source) {
        let parsed = read_selected_records(path, source);
        self.malformed += parsed.malformed;
        self.divergent_duplicates += parsed.divergent;
        for record in parsed.records {
            self.consume_record(record, filters);
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
                Entry::Occupied(mut existing) => {
                    if existing.get().same_usage_identity(&summary) {
                        if existing.get().candidate_is_better(&summary) {
                            existing.insert(summary);
                        }
                    } else {
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

    fn finalize_heatmap(self, granularity: HeatmapGranularity) -> HeatmapReport {
        let mut cells: BTreeMap<HeatCell, Totals> = BTreeMap::new();
        for (_, s) in self.per_id {
            let key = match granularity {
                HeatmapGranularity::Day => HeatCell::hour(s.date, s.local_hour),
                _ => HeatCell::day(s.date),
            };
            cells.entry(key).or_default().merge(&s.totals);
        }
        HeatmapReport {
            cells,
            malformed_lines: self.malformed,
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

/// Read access to the diagnostics fields shared by all three report
/// types (`Report` / `BlockReport` / `SessionReport`). Lets `format`
/// and the CLI dispatcher share one note-rendering path without
/// coupling to concrete report structs. `SessionReport` does not track
/// divergent duplicates (its dedup path predates `LocalAccumulator`),
/// so the default returns 0 — see AGENTS.md "后续项".
pub trait ReportDiagnostics {
    fn malformed_lines(&self) -> u64;
    fn unknown_models(&self) -> &BTreeMap<String, u64>;
    fn divergent_duplicates(&self) -> u64 {
        0
    }
}

impl ReportDiagnostics for Report {
    fn malformed_lines(&self) -> u64 {
        self.malformed_lines
    }
    fn unknown_models(&self) -> &BTreeMap<String, u64> {
        &self.unknown_models
    }
    fn divergent_duplicates(&self) -> u64 {
        self.divergent_duplicates
    }
}

impl ReportDiagnostics for BlockReport {
    fn malformed_lines(&self) -> u64 {
        self.malformed_lines
    }
    fn unknown_models(&self) -> &BTreeMap<String, u64> {
        &self.unknown_models
    }
    fn divergent_duplicates(&self) -> u64 {
        self.divergent_duplicates
    }
}

impl ReportDiagnostics for SessionReport {
    fn malformed_lines(&self) -> u64 {
        self.malformed_lines
    }
    fn unknown_models(&self) -> &BTreeMap<String, u64> {
        &self.unknown_models
    }
    fn divergent_duplicates(&self) -> u64 {
        self.divergent_duplicates
    }
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
    /// Count of true id collisions whose model or request-side token
    /// dimensions conflict. Expected intermediate/final streaming
    /// snapshots are resolved before aggregation and do not increment it.
    pub divergent_duplicates: u64,
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
    source: Source,
) -> (Option<SessionSummary>, u64, BTreeMap<String, u64>, u64) {
    let parsed = read_selected_records(path, source);
    let malformed = parsed.malformed;
    let divergent = parsed.divergent;
    let mut totals = Totals::default();
    let mut unknown_models: BTreeMap<String, u64> = BTreeMap::new();
    let mut start: Option<DateTime<Utc>> = None;
    let mut end: Option<DateTime<Utc>> = None;
    let mut cwd_counts: HashMap<String, u64> = HashMap::new();

    for record in parsed.records {
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

    if totals.records == 0 {
        return (None, malformed, unknown_models, divergent);
    }

    let primary_cwd = cwd_counts
        .iter()
        .max_by_key(|(_, count)| *count)
        .map(|(cwd, _)| cwd.clone());

    let project = primary_cwd
        .as_deref()
        .map(|cwd| {
            // Try POSIX file_name first; fall back to splitting on Windows
            // `\` so paths like `D:\Scoop\persist\foo` (recorded on a
            // Windows Claude Code host) collapse to `foo` when parsed on
            // Linux. Without this, `Path::file_name` returns the whole
            // string (no `/` present) and the full path leaks into the
            // Project column.
            if let Some(name) = std::path::Path::new(cwd)
                .file_name()
                .and_then(|s| s.to_str())
            {
                if name != cwd {
                    return name.to_string();
                }
            }
            cwd.rsplit(['\\', '/']).next().unwrap_or(cwd).to_string()
        })
        .unwrap_or_default();

    let start = start.unwrap();
    let start_date = start.with_timezone(&Local).date_naive();

    // Session-level filters: include/exclude the whole session as a unit.
    if let Some(since) = filters.since {
        if start_date < since {
            return (None, malformed, unknown_models, divergent);
        }
    }
    if let Some(until) = filters.until {
        if start_date > until {
            return (None, malformed, unknown_models, divergent);
        }
    }
    if let Some(needle) = filters.project_substring.as_deref() {
        if !primary_cwd
            .as_deref()
            .is_some_and(|cwd| cwd.contains(needle))
        {
            return (None, malformed, unknown_models, divergent);
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
        divergent,
    )
}

/// Aggregate all JSONL files as individual sessions.
#[cfg(test)]
pub fn aggregate_sessions(paths: &[PathBuf], filters: &Filters) -> SessionReport {
    aggregate_sessions_source(paths, filters, Source::Claude)
}

pub fn aggregate_sessions_source(
    paths: &[PathBuf],
    filters: &Filters,
    source: Source,
) -> SessionReport {
    let results: Vec<_> = paths
        .par_iter()
        .map(|path| aggregate_one_session(path, filters, source))
        .collect();

    let mut sessions = Vec::new();
    let mut malformed_lines = 0u64;
    let mut unknown_models: BTreeMap<String, u64> = BTreeMap::new();
    let mut divergent_duplicates = 0u64;

    for (summary, mal, unk, div) in results {
        if let Some(s) = summary {
            sessions.push(s);
        }
        malformed_lines += mal;
        divergent_duplicates += div;
        for (model, count) in unk {
            *unknown_models.entry(model).or_insert(0) += count;
        }
    }

    sessions.sort_by_key(|s| s.start);

    SessionReport {
        sessions,
        malformed_lines,
        unknown_models,
        divergent_duplicates,
    }
}

/// Process every path in parallel via rayon, then reduce + finalize into
/// a single `Report`. Caller is responsible for discovering paths
/// (see `walker::find_jsonl`) and for supplying filters consistently.
#[cfg(test)]
pub fn aggregate_daily(paths: &[PathBuf], filters: &Filters) -> Report {
    aggregate_daily_source(paths, filters, Source::Claude)
}

pub fn aggregate_daily_source(paths: &[PathBuf], filters: &Filters, source: Source) -> Report {
    paths
        .par_iter()
        .fold(LocalAccumulator::default, |mut acc, path| {
            acc.consume_file(path, filters, source);
            acc
        })
        .reduce(LocalAccumulator::default, LocalAccumulator::merge)
        .finalize_daily()
}

/// Same dedup pipeline as `aggregate_daily`, but buckets into heatmap cells
/// at the requested granularity.
pub fn aggregate_heatmap_source(
    paths: &[PathBuf],
    filters: &Filters,
    source: Source,
    granularity: HeatmapGranularity,
) -> HeatmapReport {
    paths
        .par_iter()
        .fold(LocalAccumulator::default, |mut acc, path| {
            acc.consume_file(path, filters, source);
            acc
        })
        .reduce(LocalAccumulator::default, LocalAccumulator::merge)
        .finalize_heatmap(granularity)
}

/// Test-only convenience wrapper over [`aggregate_heatmap_source`].
#[cfg(test)]
pub fn aggregate_heatmap(paths: &[PathBuf], filters: &Filters) -> HeatmapReport {
    aggregate_heatmap_source(paths, filters, Source::Claude, HeatmapGranularity::Year)
}

/// Same dedup pipeline as `aggregate_daily`, but buckets into 5-hour blocks.
#[cfg(test)]
pub fn aggregate_blocks(paths: &[PathBuf], filters: &Filters) -> BlockReport {
    aggregate_blocks_source(paths, filters, Source::Claude)
}

pub fn aggregate_blocks_source(
    paths: &[PathBuf],
    filters: &Filters,
    source: Source,
) -> BlockReport {
    paths
        .par_iter()
        .fold(LocalAccumulator::default, |mut acc, path| {
            acc.consume_file(path, filters, source);
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

    fn assistant_with_stop(
        msg_id: &str,
        model: &str,
        ts: &str,
        cwd: &str,
        input: u64,
        output: u64,
        stop_reason: &str,
    ) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{}","cwd":"{}","message":{{"id":"{}","model":"{}","usage":{{"input_tokens":{},"output_tokens":{},"cache_creation_input_tokens":0,"cache_read_input_tokens":0}},"stop_reason":"{}"}}}}"#,
            ts, cwd, msg_id, model, input, output, stop_reason
        )
    }

    fn codex_context(turn_id: &str, model: &str, cwd: &str) -> String {
        format!(
            r#"{{"timestamp":"2026-05-03T09:00:00Z","type":"turn_context","payload":{{"turn_id":"{}","cwd":"{}","model":"{}"}}}}"#,
            turn_id, cwd, model
        )
    }

    fn codex_token_count(ts: &str, input: u64, cached: u64, output: u64) -> String {
        format!(
            r#"{{"timestamp":"{}","type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":{},"cached_input_tokens":{},"output_tokens":{},"total_tokens":{}}},"model_context_window":258400}}}}}}"#,
            ts,
            input,
            cached,
            output,
            input + output
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
    fn streaming_snapshots_prefer_final_stop_reason_without_divergence() {
        let tmp = tempfile::tempdir().unwrap();
        let intermediate = assistant(
            "streamed",
            "claude-opus-4-8",
            "2026-04-05T12:00:00.100Z",
            "/p",
            16,
            1,
        );
        let final_row = assistant_with_stop(
            "streamed",
            "claude-opus-4-8",
            "2026-04-05T12:00:00.700Z",
            "/p",
            16,
            71,
            "end_turn",
        );
        let path = write_jsonl(tmp.path(), "stream.jsonl", &[&intermediate, &final_row]);

        let r = aggregate_daily(&[path], &Filters::default());
        let day = NaiveDate::from_ymd_opt(2026, 4, 5).unwrap();
        assert_eq!(r.rows[&day].records, 1);
        assert_eq!(r.rows[&day].output_tokens, 71);
        assert_eq!(r.divergent_duplicates, 0);
    }

    #[test]
    fn streaming_snapshots_without_stop_reason_take_max_output() {
        let tmp = tempfile::tempdir().unwrap();
        let early = assistant(
            "streamed",
            "claude-opus-4-8",
            "2026-04-05T12:00:00.100Z",
            "/p",
            16,
            1,
        );
        let later = assistant(
            "streamed",
            "claude-opus-4-8",
            "2026-04-05T12:00:00.700Z",
            "/p",
            16,
            71,
        );
        let path = write_jsonl(tmp.path(), "stream.jsonl", &[&early, &later]);

        let sessions = aggregate_sessions(&[path], &Filters::default());
        assert_eq!(sessions.sessions[0].totals.records, 1);
        assert_eq!(sessions.sessions[0].totals.output_tokens, 71);
        assert_eq!(sessions.divergent_duplicates, 0);
    }

    #[test]
    fn streaming_final_snapshot_can_materialize_cache_creation() {
        // Real Claude Code shape: the intermediate row has cache creation
        // zero; the final `tool_use` row fills the flat cache-creation
        // count. They are one request, not a divergent collision.
        let tmp = tempfile::tempdir().unwrap();
        let early = assistant(
            "streamed-cache",
            "claude-opus-4-6",
            "2026-04-05T12:00:00.100Z",
            "/p",
            3,
            1,
        );
        let final_row = r#"{"type":"assistant","timestamp":"2026-04-05T12:00:01.000Z","cwd":"/p","message":{"id":"streamed-cache","model":"claude-opus-4-6","usage":{"input_tokens":3,"output_tokens":148,"cache_creation_input_tokens":38131,"cache_read_input_tokens":71459,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":0}},"stop_reason":"tool_use"}}"#;
        // Match cache-read on the intermediate snapshot: request identity
        // is model + input + cache-read; cache creation may arrive later.
        let early = early.replace(
            r#""cache_read_input_tokens":0"#,
            r#""cache_read_input_tokens":71459"#,
        );
        let path = write_jsonl(tmp.path(), "stream.jsonl", &[&early, final_row]);

        let r = aggregate_daily(&[path], &Filters::default());
        let day = NaiveDate::from_ymd_opt(2026, 4, 5).unwrap();
        assert_eq!(r.rows[&day].records, 1);
        assert_eq!(r.rows[&day].output_tokens, 148);
        assert_eq!(r.rows[&day].cache_creation_5m_tokens, 38_131);
        assert_eq!(r.divergent_duplicates, 0);
    }

    #[test]
    fn codex_daily_aggregates_token_count_events() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = codex_context("turn_1", "gpt-5.5", "/work/project");
        let a = codex_token_count("2026-05-03T09:01:00Z", 1000, 250, 125);
        let b = codex_token_count("2026-05-03T09:02:00Z", 2000, 500, 250);
        let path = write_jsonl(tmp.path(), "codex.jsonl", &[&ctx, &a, &b]);

        let r = aggregate_daily_source(&[path], &Filters::default(), Source::Codex);
        let d = NaiveDate::from_ymd_opt(2026, 5, 3).unwrap();
        assert_eq!(r.rows[&d].records, 2);
        assert_eq!(r.rows[&d].input_tokens, 2250);
        assert_eq!(r.rows[&d].cache_read_tokens, 750);
        assert_eq!(r.rows[&d].output_tokens, 375);
        assert!(r.rows[&d].cost_usd > 0.0);
        assert_eq!(r.malformed_lines, 0);
    }

    #[test]
    fn codex_sessions_and_blocks_use_same_parser() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = codex_context("turn_1", "gpt-5.5", "/work/project");
        let a = codex_token_count("2026-05-03T09:01:00Z", 1000, 250, 125);
        let path = write_jsonl(tmp.path(), "rollout-demo.jsonl", &[&ctx, &a]);

        let sessions = aggregate_sessions_source(
            std::slice::from_ref(&path),
            &Filters::default(),
            Source::Codex,
        );
        assert_eq!(sessions.sessions.len(), 1);
        assert_eq!(sessions.sessions[0].project, "project");
        assert_eq!(sessions.sessions[0].totals.records, 1);

        let blocks = aggregate_blocks_source(&[path], &Filters::default(), Source::Codex);
        assert_eq!(blocks.rows.len(), 1);
        assert_eq!(blocks.rows.values().next().unwrap().records, 1);
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
    fn session_divergent_duplicates_are_counted() {
        // Two records in the same session file share an id but disagree on
        // tokens — simulates a corrupted session log. The counter must
        // fire, first-seen must win in totals (records=1, input=100).
        let tmp = tempfile::tempdir().unwrap();
        let path = write_jsonl(
            tmp.path(),
            "sess.jsonl",
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
        let r = aggregate_sessions(&[path], &Filters::default());
        assert_eq!(r.sessions.len(), 1);
        assert_eq!(r.sessions[0].totals.records, 1, "first-seen should win");
        assert_eq!(r.sessions[0].totals.input_tokens, 100);
        assert_eq!(r.divergent_duplicates, 1);
    }

    #[test]
    fn session_byte_identical_duplicates_do_not_flag_divergence() {
        // The same id appearing twice with identical payload is the
        // expected harmless duplicate — must NOT trip the divergent
        // counter.
        let tmp = tempfile::tempdir().unwrap();
        let path = write_jsonl(
            tmp.path(),
            "sess.jsonl",
            &[
                &assistant(
                    "dup",
                    "claude-opus-4-7",
                    "2026-04-05T12:00:00Z",
                    "/p",
                    100,
                    50,
                ),
                &assistant(
                    "dup",
                    "claude-opus-4-7",
                    "2026-04-05T12:00:00Z",
                    "/p",
                    100,
                    50,
                ),
            ],
        );
        let r = aggregate_sessions(&[path], &Filters::default());
        assert_eq!(r.sessions[0].totals.records, 1);
        assert_eq!(r.divergent_duplicates, 0);
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
