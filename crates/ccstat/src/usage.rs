//! Day-bucketed usage store. Parsed records fold into per-(category, name,
//! project) daily buckets; queries re-slice by time-window + project on
//! demand without rescanning the raw logs.

use crate::model::{Category, ProjectFilter, SortKey, Window};
use cctk::jsonl::{Extracted, Line};
use cctk::pricing::ModelInfo;
use chrono::{DateTime, Duration, NaiveDate, Utc};
use std::collections::HashMap;

/// Number of trailing days shown in the inline detail trend sparkline.
pub const TREND_DAYS: usize = 30;
/// Upper bound on the all-time graph span (keeps `all` bounded).
pub const MAX_SPAN_DAYS: usize = 120;

/// One session-log line's contribution to the store: its timestamp (for day
/// bucketing) and the usage records it carries. Built from a parsed
/// [`cctk::jsonl::Line`] via [`LineData::from_line`]; `cwd` is retained so the
/// scanner can resolve a project label from any line in the file.
#[derive(Debug, Default)]
pub struct LineData {
    pub timestamp: Option<DateTime<Utc>>,
    pub cwd: Option<String>,
    pub items: Vec<Extracted>,
}

impl LineData {
    /// Project a parsed canonical line into the fields the store needs.
    #[must_use]
    pub fn from_line(line: &Line) -> Self {
        LineData {
            timestamp: line.timestamp_utc(),
            cwd: line.cwd.clone(),
            items: line.extracted(),
        }
    }
}

/// Per-day aggregate for one (category, name, project) entry.
#[derive(Debug, Default, Clone, Copy)]
struct DayStat {
    count: u64,
    input: u64,
    output: u64,
    cache_creation: u64,
    cache_read: u64,
    cost_usd: f64,
}

impl DayStat {
    fn add(&mut self, other: &DayStat) {
        self.count += other.count;
        self.input += other.input;
        self.output += other.output;
        self.cache_creation += other.cache_creation;
        self.cache_read += other.cache_read;
        self.cost_usd += other.cost_usd;
    }
}

#[derive(Hash, PartialEq, Eq, Clone)]
struct EntryKey {
    category: Category,
    name: String,
    project: String,
}

/// One ranked row for the UI.
#[derive(Debug, Clone)]
pub struct Row {
    pub name: String,
    pub count: u64,
    pub last_used: Option<NaiveDate>,
    pub first_used: Option<NaiveDate>,
    /// Per-day invocation count over the last `TREND_DAYS`, oldest→newest.
    pub trend: Vec<u64>,
    /// Per-day total tokens (in+out+cache) over the last `TREND_DAYS`.
    pub trend_tokens: Vec<u64>,
    /// Per-day cost (USD) over the last `TREND_DAYS`.
    pub trend_cost: Vec<f64>,
    pub input: u64,
    pub output: u64,
    pub cache_creation: u64,
    pub cache_read: u64,
    pub cost_usd: f64,
    pub by_project: Vec<(String, u64)>,
}

impl Row {
    /// The per-day series for `metric` as `f64`, oldest→newest.
    #[must_use]
    pub fn trend_series(&self, metric: GraphMetric) -> Vec<f64> {
        match metric {
            GraphMetric::Tokens => self.trend_tokens.iter().map(|&v| v as f64).collect(),
            GraphMetric::Cost => self.trend_cost.clone(),
            GraphMetric::Messages => self.trend.iter().map(|&v| v as f64).collect(),
        }
    }
}

/// Which metric the top trends graph plots. Cycled by a key in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphMetric {
    Tokens,
    Cost,
    Messages,
}

impl GraphMetric {
    /// Next metric in the toggle cycle.
    #[must_use]
    pub fn next(self) -> Self {
        match self {
            GraphMetric::Tokens => GraphMetric::Cost,
            GraphMetric::Cost => GraphMetric::Messages,
            GraphMetric::Messages => GraphMetric::Tokens,
        }
    }

    /// Short label for the graph title.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            GraphMetric::Tokens => "tokens",
            GraphMetric::Cost => "cost",
            GraphMetric::Messages => "messages",
        }
    }
}

/// Pre-aggregated usage store.
#[derive(Default)]
pub struct UsageDb {
    entries: HashMap<EntryKey, HashMap<NaiveDate, DayStat>>,
}

impl UsageDb {
    /// Fold one parsed line's records into the store under `project`. Lines
    /// without a timestamp bucket under `fallback_day` (the file's mtime date).
    pub fn absorb(&mut self, line: &LineData, project: &str, fallback_day: NaiveDate) {
        let day = line.timestamp.map_or(fallback_day, |t| t.date_naive());
        for item in &line.items {
            let (category, name, stat) = match item {
                Extracted::Model { name, usage } => {
                    let cost = ModelInfo::parse(name).pricing().cost_usd(usage);
                    (
                        Category::Model,
                        name.clone(),
                        DayStat {
                            count: 1,
                            input: usage.input_tokens,
                            output: usage.output_tokens,
                            cache_creation: usage.cache_creation_input_tokens,
                            cache_read: usage.cache_read_input_tokens,
                            cost_usd: cost,
                        },
                    )
                }
                Extracted::Skill { name } => (Category::Skill, name.clone(), unit()),
                Extracted::Agent { name } => (Category::Agent, name.clone(), unit()),
                Extracted::Command { name } => (Category::Command, name.clone(), unit()),
                Extracted::Mcp { server } => (Category::Mcp, server.clone(), unit()),
            };
            let key = EntryKey {
                category,
                name,
                project: project.to_string(),
            };
            self.entries
                .entry(key)
                .or_default()
                .entry(day)
                .or_default()
                .add(&stat);
        }
    }

    /// Merge another store into this one (used by the parallel scanner).
    pub fn merge(&mut self, other: UsageDb) {
        for (key, days) in other.entries {
            let target = self.entries.entry(key).or_default();
            for (day, stat) in days {
                target.entry(day).or_default().add(&stat);
            }
        }
    }

    /// Sorted, de-duplicated list of every project label seen.
    #[must_use]
    pub fn projects(&self) -> Vec<String> {
        let mut set: Vec<String> = self.entries.keys().map(|k| k.project.clone()).collect();
        set.sort_unstable();
        set.dedup();
        set
    }

    /// Ranked rows for one category under the given window/project/sort.
    #[must_use]
    pub fn rows(
        &self,
        category: Category,
        window: Window,
        project: &ProjectFilter,
        sort: SortKey,
        today: NaiveDate,
    ) -> Vec<Row> {
        let cutoff = window.days().map(|d| today - Duration::days(d - 1));
        #[allow(clippy::cast_possible_wrap)]
        let trend_start = today - Duration::days(TREND_DAYS as i64 - 1);
        let show_by_project = matches!(project, ProjectFilter::All);

        let mut acc: HashMap<String, RowAcc> = HashMap::new();
        for (key, days) in &self.entries {
            if key.category != category {
                continue;
            }
            if let ProjectFilter::Only(p) = project
                && &key.project != p
            {
                continue;
            }
            let row = acc.entry(key.name.clone()).or_default();
            for (day, stat) in days {
                // recency / first-seen: all-time.
                row.last_used = Some(row.last_used.map_or(*day, |d| d.max(*day)));
                row.first_used = Some(row.first_used.map_or(*day, |d| d.min(*day)));
                // trend: last TREND_DAYS days.
                if *day >= trend_start && *day <= today {
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let idx = (*day - trend_start).num_days() as usize;
                    if idx < TREND_DAYS {
                        row.trend[idx] += stat.count;
                        row.trend_tokens[idx] +=
                            stat.input + stat.output + stat.cache_creation + stat.cache_read;
                        row.trend_cost[idx] += stat.cost_usd;
                    }
                }
                // windowed aggregates.
                let in_window = cutoff.is_none_or(|c| *day >= c) && *day <= today;
                if in_window {
                    row.count += stat.count;
                    row.input += stat.input;
                    row.output += stat.output;
                    row.cache_creation += stat.cache_creation;
                    row.cache_read += stat.cache_read;
                    row.cost_usd += stat.cost_usd;
                    if show_by_project {
                        *row.by_project.entry(key.project.clone()).or_insert(0) += stat.count;
                    }
                }
            }
        }

        let mut rows: Vec<Row> = acc
            .into_iter()
            .map(|(name, a)| a.into_row(name))
            .filter(|r| r.count > 0 || r.last_used.is_some())
            .collect();
        sort_rows(&mut rows, sort);
        rows
    }

    /// Number of daily buckets a windowed graph spans: the window's day count
    /// (7 / 30), or for all-time the days from the earliest recorded day of
    /// `category` through `today`, capped at [`MAX_SPAN_DAYS`].
    #[must_use]
    pub fn span_days(
        &self,
        category: Category,
        project: &ProjectFilter,
        window: Window,
        today: NaiveDate,
    ) -> usize {
        if let Some(d) = window.days() {
            #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
            return (d as usize).max(1);
        }
        let earliest = self
            .entries
            .iter()
            .filter(|(k, _)| k.category == category && project_matches(project, &k.project))
            .flat_map(|(_, days)| days.keys())
            .min()
            .copied();
        match earliest {
            Some(e) => {
                #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
                let span = ((today - e).num_days().max(0) as usize) + 1;
                span.clamp(1, MAX_SPAN_DAYS)
            }
            None => TREND_DAYS,
        }
    }

    /// Per-day `metric` series for `(category, name)` over the last `span` days
    /// ending `today`, oldest→newest. Empty days read as zero.
    #[must_use]
    pub fn daily_series(
        &self,
        category: Category,
        name: &str,
        project: &ProjectFilter,
        metric: GraphMetric,
        span: usize,
        today: NaiveDate,
    ) -> Vec<f64> {
        let span = span.max(1);
        #[allow(clippy::cast_possible_wrap)]
        let start = today - Duration::days(span as i64 - 1);
        let mut series = vec![0.0_f64; span];
        for (key, days) in &self.entries {
            if key.category != category || key.name != name {
                continue;
            }
            if !project_matches(project, &key.project) {
                continue;
            }
            for (day, stat) in days {
                if *day < start || *day > today {
                    continue;
                }
                #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
                let idx = (*day - start).num_days() as usize;
                if idx < span {
                    series[idx] += metric_value(stat, metric);
                }
            }
        }
        series
    }

    #[cfg(test)]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Whether `project` filter admits the project label `p`.
fn project_matches(project: &ProjectFilter, p: &str) -> bool {
    match project {
        ProjectFilter::All => true,
        ProjectFilter::Only(only) => only == p,
    }
}

/// One day's value for the selected graph metric.
fn metric_value(stat: &DayStat, metric: GraphMetric) -> f64 {
    match metric {
        GraphMetric::Tokens => {
            (stat.input + stat.output + stat.cache_creation + stat.cache_read) as f64
        }
        GraphMetric::Cost => stat.cost_usd,
        GraphMetric::Messages => stat.count as f64,
    }
}

/// A one-invocation `DayStat` for the count-only categories.
fn unit() -> DayStat {
    DayStat {
        count: 1,
        ..Default::default()
    }
}

struct RowAcc {
    count: u64,
    last_used: Option<NaiveDate>,
    first_used: Option<NaiveDate>,
    trend: Vec<u64>,
    trend_tokens: Vec<u64>,
    trend_cost: Vec<f64>,
    input: u64,
    output: u64,
    cache_creation: u64,
    cache_read: u64,
    cost_usd: f64,
    by_project: HashMap<String, u64>,
}

impl Default for RowAcc {
    fn default() -> Self {
        Self {
            count: 0,
            last_used: None,
            first_used: None,
            trend: vec![0; TREND_DAYS],
            trend_tokens: vec![0; TREND_DAYS],
            trend_cost: vec![0.0; TREND_DAYS],
            input: 0,
            output: 0,
            cache_creation: 0,
            cache_read: 0,
            cost_usd: 0.0,
            by_project: HashMap::new(),
        }
    }
}

impl RowAcc {
    fn into_row(self, name: String) -> Row {
        let mut by_project: Vec<(String, u64)> = self.by_project.into_iter().collect();
        by_project.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        Row {
            name,
            count: self.count,
            last_used: self.last_used,
            first_used: self.first_used,
            trend: self.trend,
            trend_tokens: self.trend_tokens,
            trend_cost: self.trend_cost,
            input: self.input,
            output: self.output,
            cache_creation: self.cache_creation,
            cache_read: self.cache_read,
            cost_usd: self.cost_usd,
            by_project,
        }
    }
}

fn sort_rows(rows: &mut [Row], sort: SortKey) {
    match sort {
        SortKey::Count => {
            rows.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
        }
        SortKey::Name => rows.sort_by(|a, b| a.name.cmp(&b.name)),
        SortKey::Recency => rows.sort_by(|a, b| {
            b.last_used
                .cmp(&a.last_used)
                .then_with(|| a.name.cmp(&b.name))
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cctk::jsonl::Usage;
    use chrono::TimeZone;

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn line_at(y: i32, m: u32, d: u32, items: Vec<Extracted>) -> LineData {
        let ts = chrono::Utc.with_ymd_and_hms(y, m, d, 12, 0, 0).unwrap();
        LineData {
            timestamp: Some(ts),
            cwd: None,
            items,
        }
    }

    #[test]
    fn counts_within_window_only() {
        let mut db = UsageDb::default();
        let today = day(2026, 7, 16);
        db.absorb(
            &line_at(2026, 7, 15, vec![Extracted::Skill { name: "a".into() }]),
            "p",
            today,
        );
        db.absorb(
            &line_at(2026, 7, 1, vec![Extracted::Skill { name: "a".into() }]),
            "p",
            today,
        );
        db.absorb(
            &line_at(2026, 5, 1, vec![Extracted::Skill { name: "a".into() }]),
            "p",
            today,
        );

        let all = db.rows(
            Category::Skill,
            Window::All,
            &ProjectFilter::All,
            SortKey::Count,
            today,
        );
        assert_eq!(all[0].count, 3);
        let d7 = db.rows(
            Category::Skill,
            Window::Days7,
            &ProjectFilter::All,
            SortKey::Count,
            today,
        );
        assert_eq!(d7[0].count, 1); // only 7/15 within last 7 days
        let d30 = db.rows(
            Category::Skill,
            Window::Days30,
            &ProjectFilter::All,
            SortKey::Count,
            today,
        );
        assert_eq!(d30[0].count, 2); // 7/15 and 7/1
    }

    #[test]
    fn recency_is_all_time_regardless_of_window() {
        let mut db = UsageDb::default();
        let today = day(2026, 7, 16);
        db.absorb(
            &line_at(2026, 5, 1, vec![Extracted::Skill { name: "a".into() }]),
            "p",
            today,
        );
        let d7 = db.rows(
            Category::Skill,
            Window::Days7,
            &ProjectFilter::All,
            SortKey::Count,
            today,
        );
        // Zero count in the 7d window, but the row still surfaces with its last_used.
        assert_eq!(d7[0].last_used, Some(day(2026, 5, 1)));
        assert_eq!(d7[0].count, 0);
    }

    #[test]
    fn trend_buckets_last_30_days_oldest_to_newest() {
        let mut db = UsageDb::default();
        let today = day(2026, 7, 16);
        db.absorb(
            &line_at(2026, 7, 16, vec![Extracted::Skill { name: "a".into() }]),
            "p",
            today,
        );
        db.absorb(
            &line_at(2026, 7, 16, vec![Extracted::Skill { name: "a".into() }]),
            "p",
            today,
        );
        db.absorb(
            &line_at(2026, 7, 15, vec![Extracted::Skill { name: "a".into() }]),
            "p",
            today,
        );
        let rows = db.rows(
            Category::Skill,
            Window::All,
            &ProjectFilter::All,
            SortKey::Count,
            today,
        );
        let t = &rows[0].trend;
        assert_eq!(t.len(), TREND_DAYS);
        assert_eq!(t[TREND_DAYS - 1], 2); // today
        assert_eq!(t[TREND_DAYS - 2], 1); // yesterday
    }

    #[test]
    fn by_project_populated_only_when_filter_is_all() {
        let mut db = UsageDb::default();
        let today = day(2026, 7, 16);
        db.absorb(
            &line_at(2026, 7, 16, vec![Extracted::Skill { name: "a".into() }]),
            "alpha",
            today,
        );
        db.absorb(
            &line_at(2026, 7, 16, vec![Extracted::Skill { name: "a".into() }]),
            "alpha",
            today,
        );
        db.absorb(
            &line_at(2026, 7, 16, vec![Extracted::Skill { name: "a".into() }]),
            "beta",
            today,
        );

        let all = db.rows(
            Category::Skill,
            Window::All,
            &ProjectFilter::All,
            SortKey::Count,
            today,
        );
        assert_eq!(
            all[0].by_project,
            vec![("alpha".into(), 2), ("beta".into(), 1)]
        );

        let only = db.rows(
            Category::Skill,
            Window::All,
            &ProjectFilter::Only("beta".into()),
            SortKey::Count,
            today,
        );
        assert_eq!(only[0].count, 1);
        assert!(only[0].by_project.is_empty());
    }

    #[test]
    fn model_rows_accumulate_tokens_and_cost() {
        let mut db = UsageDb::default();
        let today = day(2026, 7, 16);
        let usage = Usage {
            input_tokens: 1_000_000,
            ..Default::default()
        };
        db.absorb(
            &line_at(
                2026,
                7,
                16,
                vec![Extracted::Model {
                    name: "claude-opus-4-8".into(),
                    usage,
                }],
            ),
            "p",
            today,
        );
        let rows = db.rows(
            Category::Model,
            Window::All,
            &ProjectFilter::All,
            SortKey::Count,
            today,
        );
        assert_eq!(rows[0].name, "claude-opus-4-8");
        assert_eq!(rows[0].count, 1);
        assert_eq!(rows[0].input, 1_000_000);
        assert!((rows[0].cost_usd - 5.0).abs() < 0.01); // opus input = $5/M
    }

    #[test]
    fn sort_orders_are_deterministic() {
        let mut db = UsageDb::default();
        let today = day(2026, 7, 16);
        db.absorb(
            &line_at(2026, 7, 16, vec![Extracted::Skill { name: "b".into() }]),
            "p",
            today,
        );
        db.absorb(
            &line_at(2026, 7, 16, vec![Extracted::Skill { name: "b".into() }]),
            "p",
            today,
        );
        db.absorb(
            &line_at(2026, 7, 10, vec![Extracted::Skill { name: "a".into() }]),
            "p",
            today,
        );

        let by_count = db.rows(
            Category::Skill,
            Window::All,
            &ProjectFilter::All,
            SortKey::Count,
            today,
        );
        assert_eq!(
            by_count.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            vec!["b", "a"]
        );

        let by_name = db.rows(
            Category::Skill,
            Window::All,
            &ProjectFilter::All,
            SortKey::Name,
            today,
        );
        assert_eq!(
            by_name.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            vec!["a", "b"]
        );

        let by_recency = db.rows(
            Category::Skill,
            Window::All,
            &ProjectFilter::All,
            SortKey::Recency,
            today,
        );
        assert_eq!(by_recency[0].name, "b"); // used today > used 7/10
    }

    #[test]
    fn merge_combines_two_stores() {
        let today = day(2026, 7, 16);
        let mut a = UsageDb::default();
        a.absorb(
            &line_at(2026, 7, 16, vec![Extracted::Skill { name: "x".into() }]),
            "p",
            today,
        );
        let mut b = UsageDb::default();
        b.absorb(
            &line_at(2026, 7, 16, vec![Extracted::Skill { name: "x".into() }]),
            "p",
            today,
        );
        a.merge(b);
        let rows = a.rows(
            Category::Skill,
            Window::All,
            &ProjectFilter::All,
            SortKey::Count,
            today,
        );
        assert_eq!(rows[0].count, 2);
    }

    #[test]
    fn fallback_day_used_when_line_has_no_timestamp() {
        let mut db = UsageDb::default();
        let today = day(2026, 7, 16);
        let line = LineData {
            timestamp: None,
            cwd: None,
            items: vec![Extracted::Skill { name: "a".into() }],
        };
        db.absorb(&line, "p", day(2026, 7, 16));
        let rows = db.rows(
            Category::Skill,
            Window::Days7,
            &ProjectFilter::All,
            SortKey::Count,
            today,
        );
        assert_eq!(rows[0].count, 1);
    }

    #[test]
    fn projects_are_sorted_and_unique() {
        let mut db = UsageDb::default();
        let today = day(2026, 7, 16);
        db.absorb(
            &line_at(2026, 7, 16, vec![Extracted::Skill { name: "a".into() }]),
            "beta",
            today,
        );
        db.absorb(
            &line_at(2026, 7, 16, vec![Extracted::Skill { name: "a".into() }]),
            "alpha",
            today,
        );
        db.absorb(
            &line_at(2026, 7, 16, vec![Extracted::Skill { name: "a".into() }]),
            "beta",
            today,
        );
        assert_eq!(db.projects(), vec!["alpha".to_string(), "beta".to_string()]);
    }

    #[test]
    fn trend_series_selects_metric_and_populates_daily_tokens_cost() {
        let mut db = UsageDb::default();
        let today = day(2026, 7, 16);
        let usage = Usage {
            input_tokens: 100,
            output_tokens: 50,
            cache_read_input_tokens: 10,
            ..Default::default()
        };
        db.absorb(
            &line_at(
                2026,
                7,
                16,
                vec![Extracted::Model {
                    name: "opus".into(),
                    usage,
                }],
            ),
            "p",
            today,
        );
        let rows = db.rows(
            Category::Model,
            Window::All,
            &ProjectFilter::All,
            SortKey::Count,
            today,
        );
        let r = &rows[0];
        // The last trend bucket (today) holds this event's metrics.
        assert!((r.trend_series(GraphMetric::Tokens).last().unwrap() - 160.0).abs() < f64::EPSILON);
        assert!((r.trend_series(GraphMetric::Messages).last().unwrap() - 1.0).abs() < f64::EPSILON);
        assert!(*r.trend_series(GraphMetric::Cost).last().unwrap() > 0.0);
    }

    #[test]
    fn graph_metric_cycles() {
        assert_eq!(GraphMetric::Tokens.next(), GraphMetric::Cost);
        assert_eq!(GraphMetric::Cost.next(), GraphMetric::Messages);
        assert_eq!(GraphMetric::Messages.next(), GraphMetric::Tokens);
    }

    #[test]
    fn span_days_follows_window() {
        let mut db = UsageDb::default();
        let today = day(2026, 7, 20);
        let unit = |name: &str| Extracted::Model {
            name: name.into(),
            usage: Usage {
                input_tokens: 1,
                ..Default::default()
            },
        };
        db.absorb(&line_at(2026, 6, 1, vec![unit("opus")]), "p", today);
        db.absorb(&line_at(2026, 7, 20, vec![unit("opus")]), "p", today);

        assert_eq!(
            db.span_days(Category::Model, &ProjectFilter::All, Window::Days7, today),
            7
        );
        assert_eq!(
            db.span_days(Category::Model, &ProjectFilter::All, Window::Days30, today),
            30
        );
        // 2026-06-01 .. 2026-07-20 inclusive = 50 days.
        assert_eq!(
            db.span_days(Category::Model, &ProjectFilter::All, Window::All, today),
            50
        );
    }

    #[test]
    fn daily_series_places_metric_in_the_right_bucket() {
        let mut db = UsageDb::default();
        let today = day(2026, 7, 20);
        let usage = Usage {
            input_tokens: 100,
            output_tokens: 50,
            ..Default::default()
        };
        db.absorb(
            &line_at(
                2026,
                7,
                20,
                vec![Extracted::Model {
                    name: "opus".into(),
                    usage,
                }],
            ),
            "p",
            today,
        );
        let s = db.daily_series(
            Category::Model,
            "opus",
            &ProjectFilter::All,
            GraphMetric::Tokens,
            7,
            today,
        );
        assert_eq!(s.len(), 7);
        // Today's event lands in the last (newest) bucket.
        assert!((s[6] - 150.0).abs() < f64::EPSILON);
        assert!(s[..6].iter().all(|&v| v.abs() < f64::EPSILON));
    }
}
