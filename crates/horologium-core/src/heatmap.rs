//! Heatmap value mapping: pure functions from aggregated cells to
//! color intensity levels, harness-agnostic and dependency-free.
//!
//! Rendering (ANSI / ASCII grids) lives in the CLI crate; this module only
//! decides *how intense a cell is* so the same mapping can be reused by
//! any renderer (terminal, SVG, future UI).

use crate::aggregate::{HeatCell, HeatmapGranularity, HeatmapMetric, HeatmapReport, Totals};
use chrono::{Datelike, Duration, NaiveDate, Weekday};

/// Number of intensity levels, mirroring GitHub's 5-level contribution
/// scale: 0 (no data) plus four increasing shades.
pub const LEVELS: usize = 5;

/// Value of one cell under the chosen metric.
pub fn cell_value(totals: &Totals, metric: HeatmapMetric) -> f64 {
    match metric {
        HeatmapMetric::Cost => totals.cost_usd,
        HeatmapMetric::Tokens => totals.all_tokens() as f64,
    }
}

/// Compute four ascending thresholds from the set of non-zero cell values
/// (nearest-rank quartiles, GitHub-style relative scale). An all-zero /
/// empty input yields all-zero thresholds, which maps every cell to level 1.
///
/// Threshold semantics: value <= t[0] → level 1, <= t[1] → level 2,
/// <= t[2] → level 3, else level 4. `t[3]` is always the maximum.
pub fn quantile_thresholds(values: &[f64]) -> [f64; 4] {
    let mut v: Vec<f64> = values.iter().copied().filter(|x| *x > 0.0).collect();
    if v.is_empty() {
        return [0.0; 4];
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    // nearest-rank: k-th quartile at ceil(n*k/4), 1-indexed.
    let q = |k: usize| {
        let idx = ((n * k) as f64 / 4.0).ceil() as usize;
        v[idx.saturating_sub(1).min(n - 1)]
    };
    [q(1), q(2), q(3), v[n - 1]]
}

/// Map a value to a 0-4 level using thresholds from
/// [`quantile_thresholds`]. Values <= 0 are level 0 (no activity).
pub fn level_for(value: f64, thresholds: &[f64; 4]) -> u8 {
    if value <= 0.0 {
        return 0;
    }
    if value <= thresholds[0] {
        1
    } else if value <= thresholds[1] {
        2
    } else if value <= thresholds[2] {
        3
    } else {
        4
    }
}

/// Short numeric summary of a cell's value under the metric (used in
/// month/week views where cells carry their date number instead).
pub fn metric_unit(metric: HeatmapMetric) -> &'static str {
    match metric {
        HeatmapMetric::Cost => "$",
        HeatmapMetric::Tokens => "",
    }
}

// ---- rendering (shared by the CLI and the pi helper) ----------------------
//
// ANSI is hand-assembled here (no owo-colors) so both consumers render
// identically; the pi-tui Text component preserves ANSI sequences.

/// Truecolor palette mirroring GitHub's contribution green ramp.
const LEVEL_COLORS: [(u8, u8, u8); 4] = [
    (155, 233, 168), // level 1
    (64, 196, 99),   // level 2
    (48, 161, 78),   // level 3
    (33, 110, 57),   // level 4
];

const PLAIN_CHARS: [&str; 5] = ["··", "░░", "▒▒", "▓▓", "██"];

fn ansi_bg(r: u8, g: u8, b: u8, text: &str) -> String {
    format!("\x1b[48;2;{r};{g};{b}m{text}\x1b[0m")
}

fn cell_str(level: u8, color: bool) -> String {
    if !color {
        return PLAIN_CHARS[level as usize].to_string();
    }
    if level == 0 {
        "  ".to_string()
    } else {
        let (r, g, b) = LEVEL_COLORS[(level - 1) as usize];
        ansi_bg(r, g, b, "  ")
    }
}

/// Render one grid cell for a date. `label` overrides the two characters
/// in color mode (month/week views show day numbers on a shaded
/// background); plain mode always renders the shaded block so shading
/// stays visible without colors.
fn render_cell(
    date: NaiveDate,
    hour: u8,
    report: &HeatmapReport,
    thresholds: &[f64; 4],
    metric: HeatmapMetric,
    color: bool,
    label: Option<&str>,
) -> String {
    let value = report
        .cells
        .get(&HeatCell::hour(date, hour))
        .map(|t| cell_value(t, metric))
        .unwrap_or(0.0);
    let level = level_for(value, thresholds);
    if level == 0 {
        // No activity: keep the label in color mode for month/week views,
        // blank otherwise.
        label.filter(|_| color).unwrap_or("  ").to_string()
    } else {
        match (color, label) {
            (true, Some(l)) => {
                let (r, g, b) = LEVEL_COLORS[(level - 1) as usize];
                ansi_bg(r, g, b, l)
            }
            _ => cell_str(level, color),
        }
    }
}

/// The visible window for a granularity anchored at `anchor` (inclusive).
pub fn window_for(anchor: NaiveDate, g: HeatmapGranularity) -> (NaiveDate, NaiveDate) {
    match g {
        HeatmapGranularity::Year => {
            let end = monday_of(anchor) + Duration::days(6);
            let start = monday_of(anchor) - Duration::days(52 * 7);
            (start, end)
        }
        HeatmapGranularity::Month => {
            let start = anchor.with_day(1).unwrap_or(anchor);
            let end = next_month_first(start) - Duration::days(1);
            (start, end)
        }
        HeatmapGranularity::Week => (monday_of(anchor), monday_of(anchor) + Duration::days(6)),
        HeatmapGranularity::Day => (anchor, anchor),
    }
}

fn next_month_first(d: NaiveDate) -> NaiveDate {
    let (y, m) = if d.month() == 12 {
        (d.year() + 1, 1)
    } else {
        (d.year(), d.month() + 1)
    };
    NaiveDate::from_ymd_opt(y, m, 1).unwrap()
}

fn monday_of(d: NaiveDate) -> NaiveDate {
    let offset = (d.weekday().num_days_from_monday()) as i64;
    d - Duration::days(offset)
}

/// Sum of all cells in the report (dedup-safe; hour cells collapse into
/// their day for day granularity).
pub fn totals_for(report: &HeatmapReport, g: HeatmapGranularity) -> Totals {
    let mut t = Totals::default();
    for (cell, totals) in &report.cells {
        let matches = match g {
            HeatmapGranularity::Day => true,
            _ => cell.hour == 0,
        };
        if matches {
            t.merge(totals);
        }
    }
    t
}

/// Format a token count compactly (1.23K / 4.56M / 7.89B).
pub fn fmt_tokens(n: f64) -> String {
    if n >= 1e9 {
        format!("{:.2}B", n / 1e9)
    } else if n >= 1e6 {
        format!("{:.2}M", n / 1e6)
    } else if n >= 1e3 {
        format!("{:.2}K", n / 1e3)
    } else {
        format!("{n:.0}")
    }
}

/// Full heatmap render: header, grid (granularity-dependent), legend.
/// Fixed layout (2-char cells + 1-char gap); interactive surfaces that
/// need width adaptation draw their own grid from raw cells instead.
pub fn render_heatmap(
    report: &HeatmapReport,
    g: HeatmapGranularity,
    metric: HeatmapMetric,
    anchor: NaiveDate,
    color: bool,
) -> String {
    let values: Vec<f64> = report
        .cells
        .values()
        .map(|t| cell_value(t, metric))
        .collect();
    let thresholds = quantile_thresholds(&values);
    let total = totals_for(report, g);
    let total_val = match metric {
        HeatmapMetric::Cost => format!("${:.2}", total.cost_usd),
        HeatmapMetric::Tokens => fmt_tokens(total.all_tokens() as f64),
    };
    let (since, until) = window_for(anchor, g);
    let mut out = String::new();
    out.push_str(&format!(
        "{} · {} · total {}\n",
        g.label(),
        since,
        total_val
    ));
    out.push_str(&format!(
        "metric: {} · window: {} ~ {}\n",
        match metric {
            HeatmapMetric::Cost => "cost (USD)",
            HeatmapMetric::Tokens => "tokens",
        },
        since,
        until
    ));
    match g {
        HeatmapGranularity::Year => {
            render_year(&mut out, report, &thresholds, metric, anchor, color)
        }
        HeatmapGranularity::Month => {
            render_month(&mut out, report, &thresholds, metric, anchor, color)
        }
        HeatmapGranularity::Week => {
            render_week(&mut out, report, &thresholds, metric, anchor, color)
        }
        HeatmapGranularity::Day => render_day(&mut out, report, &thresholds, metric, anchor, color),
    }
    out.push_str(&legend(color));
    out
}

fn render_year(
    out: &mut String,
    report: &HeatmapReport,
    thresholds: &[f64; 4],
    metric: HeatmapMetric,
    anchor: NaiveDate,
    color: bool,
) {
    let start = monday_of(anchor) - Duration::days(52 * 7);
    // Month labels on the same 3-char slot as the cells below; the leading
    // 4 spaces match the Mon/Wed/Fri row labels.
    out.push_str("    ");
    let mut prev_month = 0;
    for w in 0..53 {
        let monday = start + Duration::days(w * 7);
        if monday.month() != prev_month {
            let label = format!("{:>3}", month_abbr(monday.month()));
            out.push_str(&label);
            prev_month = monday.month();
        } else {
            out.push_str("   ");
        }
    }
    out.push('\n');
    let mut rows: Vec<String> = Vec::new();
    for dow in 0..7 {
        let mut line = String::new();
        line.push_str(match dow {
            0 => "Mon ",
            2 => "Wed ",
            4 => "Fri ",
            _ => "    ",
        });
        for w in 0..53 {
            let date = start + Duration::days(w * 7 + dow);
            line.push_str(&render_cell(
                date, 0, report, thresholds, metric, color, None,
            ));
            line.push(' ');
        }
        rows.push(line);
    }
    out.push_str(&rows.join("\n"));
    out.push('\n');
}

fn render_month(
    out: &mut String,
    report: &HeatmapReport,
    thresholds: &[f64; 4],
    metric: HeatmapMetric,
    anchor: NaiveDate,
    color: bool,
) {
    out.push_str("Mon Tue Wed Thu Fri Sat Sun\n");
    let first = anchor.with_day(1).unwrap_or(anchor);
    let offset = first.weekday().num_days_from_monday() as i64;
    let days_in_month = (next_month_first(first) - Duration::days(1)).day();
    let mut line = String::new();
    for _ in 0..offset {
        line.push_str("    ");
    }
    for day in 1..=days_in_month {
        let date = first.with_day(day).unwrap();
        let label = format!("{day:>2}");
        line.push_str(&render_cell(
            date,
            0,
            report,
            thresholds,
            metric,
            color,
            Some(&label),
        ));
        line.push(' ');
        if first.with_day(day).unwrap().weekday() == Weekday::Sun && day < days_in_month {
            out.push_str(line.trim_end());
            out.push('\n');
            line.clear();
        }
    }
    out.push_str(line.trim_end());
    out.push('\n');
}

fn render_week(
    out: &mut String,
    report: &HeatmapReport,
    thresholds: &[f64; 4],
    metric: HeatmapMetric,
    anchor: NaiveDate,
    color: bool,
) {
    out.push_str("Mon Tue Wed Thu Fri Sat Sun\n");
    let start = monday_of(anchor);
    let mut line = String::new();
    for dow in 0..7 {
        let date = start + Duration::days(dow);
        let label = format!("{:>2}", date.day());
        line.push_str(&render_cell(
            date,
            0,
            report,
            thresholds,
            metric,
            color,
            Some(&label),
        ));
        line.push(' ');
    }
    out.push_str(line.trim_end());
    out.push('\n');
}

fn render_day(
    out: &mut String,
    report: &HeatmapReport,
    thresholds: &[f64; 4],
    metric: HeatmapMetric,
    anchor: NaiveDate,
    color: bool,
) {
    let mut line = String::new();
    for h in 0..24 {
        line.push_str(&render_cell(
            anchor, h as u8, report, thresholds, metric, color, None,
        ));
        line.push(' ');
    }
    out.push_str(line.trim_end());
    out.push('\n');
    // Hour ticks aligned under the 2-char cells: each tick slot is 3 chars.
    let mut ticks = String::new();
    for h in 0..24 {
        if h % 6 == 0 || h == 23 {
            ticks.push_str(&format!("{h:>3}"));
        } else {
            ticks.push_str("   ");
        }
    }
    out.push_str(ticks.trim_end());
    out.push('\n');
}

fn legend(color: bool) -> String {
    let mut s = String::from("less ");
    for lvl in 0..5 {
        s.push_str(&cell_str(lvl, color));
        s.push(' ');
    }
    s.push_str("more\n");
    s
}

fn month_abbr(m: u32) -> &'static str {
    match m {
        1 => "Jan",
        2 => "Feb",
        3 => "Mar",
        4 => "Apr",
        5 => "May",
        6 => "Jun",
        7 => "Jul",
        8 => "Aug",
        9 => "Sep",
        10 => "Oct",
        11 => "Nov",
        12 => "Dec",
        _ => "???",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregate::{HeatCell, HeatmapReport};
    use std::collections::BTreeMap;

    fn t(input: u64, out: u64, cr: u64) -> Totals {
        Totals {
            input_tokens: input,
            output_tokens: out,
            cache_read_tokens: cr,
            ..Totals::default()
        }
    }

    fn day_cells(days: &[(i32, u32, u32, f64)]) -> BTreeMap<HeatCell, Totals> {
        let mut m = BTreeMap::new();
        for (y, mo, d, cost) in days {
            let t = Totals {
                cost_usd: *cost,
                records: 1,
                ..Totals::default()
            };
            m.insert(
                HeatCell::day(NaiveDate::from_ymd_opt(*y, *mo, *d).unwrap()),
                t,
            );
        }
        m
    }

    #[test]
    fn cell_value_maps_both_metrics() {
        let totals = t(100, 50, 30);
        assert_eq!(cell_value(&totals, HeatmapMetric::Tokens), 180.0);
        assert_eq!(cell_value(&totals, HeatmapMetric::Cost), 0.0);
        let mut costly = t(0, 0, 0);
        costly.cost_usd = 1.25;
        assert_eq!(cell_value(&costly, HeatmapMetric::Cost), 1.25);
    }

    #[test]
    fn all_tokens_includes_cache_writes() {
        let mut totals = t(10, 20, 30);
        totals.cache_creation_5m_tokens = 4;
        totals.cache_creation_1h_tokens = 5;
        assert_eq!(totals.all_tokens(), 69);
    }

    #[test]
    fn empty_values_map_to_zero_thresholds() {
        assert_eq!(quantile_thresholds(&[]), [0.0; 4]);
        assert_eq!(quantile_thresholds(&[0.0, -1.0]), [0.0; 4]);
    }

    #[test]
    fn quartiles_are_ascending_nearest_rank() {
        // 1..=8: q1 = ceil(8/4)=2nd → 2, q2 = ceil(16/4)=4th → 4,
        // q3 = ceil(24/4)=6th → 6, max = 8.
        let vals: Vec<f64> = (1..=8).map(|x| x as f64).collect();
        assert_eq!(quantile_thresholds(&vals), [2.0, 4.0, 6.0, 8.0]);
    }

    #[test]
    fn levels_follow_threshold_boundaries() {
        let th = [2.0, 4.0, 6.0, 8.0];
        assert_eq!(level_for(0.0, &th), 0);
        assert_eq!(level_for(-3.0, &th), 0);
        assert_eq!(level_for(1.0, &th), 1);
        assert_eq!(level_for(2.0, &th), 1);
        assert_eq!(level_for(3.0, &th), 2);
        assert_eq!(level_for(4.0, &th), 2);
        assert_eq!(level_for(5.0, &th), 3);
        assert_eq!(level_for(6.0, &th), 3);
        assert_eq!(level_for(6.1, &th), 4);
        assert_eq!(level_for(100.0, &th), 4);
    }

    #[test]
    fn zero_thresholds_only_match_zero_values() {
        // All-zero input cells are level 0; a positive value with zero
        // thresholds cannot occur (thresholds come from the same set).
        assert_eq!(level_for(0.0, &[0.0; 4]), 0);
        assert_eq!(level_for(0.5, &[0.0; 4]), 4);
    }

    #[test]
    fn window_for_year_is_53_weeks() {
        let (s, u) = window_for(
            NaiveDate::from_ymd_opt(2026, 8, 11).unwrap(),
            HeatmapGranularity::Year,
        );
        assert_eq!(s, NaiveDate::from_ymd_opt(2025, 8, 11).unwrap());
        assert_eq!(u, NaiveDate::from_ymd_opt(2026, 8, 16).unwrap());
        assert_eq!(s.weekday(), Weekday::Mon);
    }

    #[test]
    fn window_for_month_covers_calendar_month() {
        let (s, u) = window_for(
            NaiveDate::from_ymd_opt(2026, 8, 11).unwrap(),
            HeatmapGranularity::Month,
        );
        assert_eq!(s, NaiveDate::from_ymd_opt(2026, 8, 1).unwrap());
        assert_eq!(u, NaiveDate::from_ymd_opt(2026, 8, 31).unwrap());
        let (_, u) = window_for(
            NaiveDate::from_ymd_opt(2026, 12, 25).unwrap(),
            HeatmapGranularity::Month,
        );
        assert_eq!(u, NaiveDate::from_ymd_opt(2026, 12, 31).unwrap());
        let (s, _) = window_for(
            NaiveDate::from_ymd_opt(2026, 1, 2).unwrap(),
            HeatmapGranularity::Month,
        );
        assert_eq!(s, NaiveDate::from_ymd_opt(2026, 1, 1).unwrap());
    }

    #[test]
    fn render_year_is_53x7_grid() {
        let cells = day_cells(&[(2026, 8, 11, 1.0), (2026, 8, 1, 2.0), (2025, 8, 15, 0.5)]);
        let report = HeatmapReport {
            cells,
            ..HeatmapReport::default()
        };
        let s = render_heatmap(
            &report,
            HeatmapGranularity::Year,
            HeatmapMetric::Cost,
            NaiveDate::from_ymd_opt(2026, 8, 11).unwrap(),
            false,
        );
        let lines: Vec<&str> = s.lines().collect();
        // header(2) + month label row + 7 day rows + legend(2)
        assert!(lines.len() >= 10);
        assert!(lines[2].len() >= 53 * 2);
        assert!(s.contains("total $3.50"));
        assert!(s.contains("Mon"));
        assert!(s.contains("Fri"));
    }

    #[test]
    fn render_day_has_24_cells_and_ticks() {
        let mut cells = day_cells(&[]);
        let t = Totals {
            cost_usd: 9.0,
            ..Totals::default()
        };
        cells.insert(
            HeatCell::hour(NaiveDate::from_ymd_opt(2026, 8, 11).unwrap(), 15),
            t,
        );
        let report = HeatmapReport {
            cells,
            ..HeatmapReport::default()
        };
        let s = render_heatmap(
            &report,
            HeatmapGranularity::Day,
            HeatmapMetric::Cost,
            NaiveDate::from_ymd_opt(2026, 8, 11).unwrap(),
            false,
        );
        assert!(s.contains("total $9.00"));
        // tick row contains the hour labels
        assert!(s.contains("23"));
    }

    #[test]
    fn month_view_lays_out_by_weekday() {
        // 2026-08-01 is a Saturday → 5 leading blanks (20 spaces).
        // Plain mode: the active day renders as a shaded block.
        let cells = day_cells(&[(2026, 8, 1, 1.0)]);
        let report = HeatmapReport {
            cells,
            ..HeatmapReport::default()
        };
        let s = render_heatmap(
            &report,
            HeatmapGranularity::Month,
            HeatmapMetric::Cost,
            NaiveDate::from_ymd_opt(2026, 8, 11).unwrap(),
            false,
        );
        let body: Vec<&str> = s.lines().skip(3).collect();
        assert!(body[0].starts_with(&format!("{}░░", " ".repeat(20))));
    }

    #[test]
    fn month_view_colors_existing_cells() {
        // Mirrors real data: only 7/1..7/4 and 7/19 have activity.
        let cells = day_cells(&[
            (2026, 7, 1, 0.518),
            (2026, 7, 2, 6.65),
            (2026, 7, 3, 0.028),
            (2026, 7, 19, 0.0),
        ]);
        let report = HeatmapReport {
            cells,
            ..HeatmapReport::default()
        };
        let s = render_heatmap(
            &report,
            HeatmapGranularity::Month,
            HeatmapMetric::Cost,
            NaiveDate::from_ymd_opt(2026, 7, 31).unwrap(),
            false,
        );
        let grid = s.lines().skip(3).take(5).collect::<Vec<_>>().join("\n");
        // 7/1..7/3 (nonzero cost) are shaded; zero-cost days and legend are
        // excluded from `grid` so the check is unambiguous.
        assert!(
            grid.contains("░░")
                || grid.contains("▒▒")
                || grid.contains("▓▓")
                || grid.contains("██"),
            "expected shaded cells in month view, got:\n{s}"
        );
    }

    #[test]
    fn plain_mode_uses_ascii_only() {
        let cells = day_cells(&[(2026, 8, 11, 5.0)]);
        let report = HeatmapReport {
            cells,
            ..HeatmapReport::default()
        };
        let s = render_heatmap(
            &report,
            HeatmapGranularity::Week,
            HeatmapMetric::Cost,
            NaiveDate::from_ymd_opt(2026, 8, 11).unwrap(),
            false,
        );
        assert!(!s.contains('\u{1b}'));
        assert!(s.contains("██") || s.contains("▓▓") || s.contains("▒▒") || s.contains("░░"));
    }

    #[test]
    fn color_mode_uses_truecolor_ansi() {
        let cells = day_cells(&[(2026, 8, 11, 5.0)]);
        let report = HeatmapReport {
            cells,
            ..HeatmapReport::default()
        };
        let s = render_heatmap(
            &report,
            HeatmapGranularity::Week,
            HeatmapMetric::Cost,
            NaiveDate::from_ymd_opt(2026, 8, 11).unwrap(),
            true,
        );
        assert!(s.contains("\x1b[48;2;"));
    }
}
