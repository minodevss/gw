use std::collections::HashMap;

use super::chart::{Zone, float};
use crate::store::Point;

const HOUR: i64 = 3600;
const DAY: i64 = 86_400;
pub(super) const OTHER: &str = "Other";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Range {
    Day,
    Week,
    Month,
}

pub(super) const RANGES: [Range; 3] = [Range::Day, Range::Week, Range::Month];

impl Range {
    pub(super) fn parse(text: Option<&str>) -> Self {
        RANGES
            .into_iter()
            .find(|range| Some(range.query()) == text)
            .unwrap_or(Self::Week)
    }

    pub(super) fn query(self) -> &'static str {
        match self {
            Self::Day => "24h",
            Self::Week => "7d",
            Self::Month => "30d",
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Day => "Last 24 hours",
            Self::Week => "Last 7 days",
            Self::Month => "Last 30 days",
        }
    }

    pub(super) fn secs(self) -> i64 {
        match self {
            Self::Day => DAY,
            Self::Week => 7 * DAY,
            Self::Month => 30 * DAY,
        }
    }

    pub(super) fn bucket(self) -> i64 {
        match self {
            Self::Day => HOUR,
            Self::Week => 6 * HOUR,
            Self::Month => DAY,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum By {
    Key,
    Model,
    Account,
    Outcome,
}

pub(super) const BYS: [By; 4] = [By::Key, By::Model, By::Account, By::Outcome];

impl By {
    pub(super) fn parse(text: Option<&str>) -> Self {
        BYS.into_iter()
            .find(|by| Some(by.query()) == text)
            .unwrap_or(Self::Key)
    }

    pub(super) fn query(self) -> &'static str {
        match self {
            Self::Key => "key",
            Self::Model => "model",
            Self::Account => "account",
            Self::Outcome => "outcome",
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Key => "Key",
            Self::Model => "Model",
            Self::Account => "Account",
            Self::Outcome => "Outcome",
        }
    }

    pub(super) fn group(self, point: &Point) -> String {
        match self {
            Self::Key => point.key.clone().unwrap_or_else(|| "(no key)".to_owned()),
            Self::Model => point.model.clone(),
            Self::Account => point
                .account
                .clone()
                .unwrap_or_else(|| "(not sent)".to_owned()),
            Self::Outcome => outcome(point).to_owned(),
        }
    }
}

pub(super) fn outcome(point: &Point) -> &'static str {
    if point.limit {
        "limit"
    } else if point.failed {
        "error"
    } else {
        "ok"
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct Totals {
    pub(super) requests: i64,
    pub(super) limits: i64,
    pub(super) errors: i64,
    pub(super) uncached: i64,
    pub(super) cache_read: i64,
    pub(super) cache_write: i64,
    pub(super) output: i64,
    durations: Vec<i64>,
}

impl Totals {
    pub(super) fn add(&mut self, point: &Point) {
        self.requests += 1;
        match outcome(point) {
            "limit" => self.limits += 1,
            "error" => self.errors += 1,
            _ => self.durations.push(point.duration_ms),
        }
        self.uncached += point.input;
        self.cache_read += point.cache_read;
        self.cache_write += point.cache_write;
        self.output += point.output;
    }

    pub(super) fn input(&self) -> i64 {
        self.uncached + self.cache_read + self.cache_write
    }

    pub(super) fn tokens(&self) -> i64 {
        self.input() + self.output
    }

    pub(super) fn cache_ratio(&self) -> Option<f64> {
        let input = self.input();
        (input > 0).then(|| float(self.cache_read) / float(input))
    }

    pub(super) fn percentile(&mut self, fraction: f64) -> Option<i64> {
        if self.durations.is_empty() {
            return None;
        }
        self.durations.sort_unstable();
        let count = self.durations.len();
        let rank = (fraction * float(i64::try_from(count).unwrap_or(i64::MAX))).ceil();
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "rank is between 1 and the number of durations"
        )]
        let index = (rank.max(1.0) as usize).min(count) - 1;
        self.durations.get(index).copied()
    }
}

pub(super) struct Buckets {
    pub(super) starts: Vec<i64>,
    pub(super) end: i64,
    pub(super) size: i64,
}

impl Buckets {
    pub(super) fn new(range: Range, now: i64, zone: &Zone) -> Self {
        let size = range.bucket();
        let count = range.secs() / size;
        if size >= DAY {
            let mut starts = vec![zone.day_start(now)];
            while i64::try_from(starts.len()).unwrap_or(i64::MAX) < count {
                let Some(first) = starts.first().copied() else {
                    break;
                };
                starts.insert(0, zone.day_start(first - HOUR));
            }
            let end = starts.last().map_or(now, |last| zone.next_day(*last));
            return Self { starts, end, size };
        }
        let offset = zone.offset_at(now);
        let end = (now + offset).div_euclid(size) * size - offset + size;
        Self {
            starts: (0..count)
                .map(|index| end - (count - index) * size)
                .collect(),
            end,
            size,
        }
    }

    pub(super) fn since(&self) -> i64 {
        self.starts.first().copied().unwrap_or_default()
    }

    pub(super) fn bucket_end(&self, index: usize) -> i64 {
        self.starts.get(index + 1).copied().unwrap_or(self.end)
    }

    pub(super) fn index(&self, ts: i64) -> Option<usize> {
        if ts < self.since() || ts >= self.end {
            return None;
        }
        self.starts
            .partition_point(|start| *start <= ts)
            .checked_sub(1)
    }
}

pub(super) fn top_groups(points: &[Point], by: By, limit: usize) -> Vec<String> {
    let mut tokens: HashMap<String, (i64, i64)> = HashMap::new();
    for point in points {
        let entry = tokens.entry(by.group(point)).or_default();
        entry.0 += point.input + point.cache_read + point.cache_write + point.output;
        entry.1 += 1;
    }
    let mut groups: Vec<(String, (i64, i64))> = tokens.into_iter().collect();
    match by {
        By::Outcome => {
            let order = |name: &str| match name {
                "ok" => 0,
                "limit" => 1,
                _ => 2,
            };
            groups.sort_by_key(|(name, _)| order(name));
        }
        By::Key | By::Model | By::Account => {
            groups.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        }
    }
    let mut names: Vec<String> = groups.into_iter().map(|(name, _)| name).collect();
    if names.len() > limit {
        names.truncate(limit.saturating_sub(1));
        names.push(OTHER.to_owned());
    }
    names
}

pub(super) fn slot_of(groups: &[String], name: &str) -> usize {
    groups
        .iter()
        .position(|group| group == name)
        .or_else(|| groups.iter().position(|group| group == OTHER))
        .unwrap_or(groups.len())
}

pub(super) struct Grid {
    pub(super) cells: Vec<Vec<Totals>>,
}

impl Grid {
    pub(super) fn new(points: &[Point], buckets: &Buckets, by: By, groups: &[String]) -> Self {
        let mut cells = vec![vec![Totals::default(); groups.len()]; buckets.starts.len()];
        for point in points {
            let Some(bucket) = buckets.index(point.ts) else {
                continue;
            };
            let slot = slot_of(groups, &by.group(point));
            if let Some(cell) = cells.get_mut(bucket).and_then(|row| row.get_mut(slot)) {
                cell.add(point);
            }
        }
        Self { cells }
    }

    pub(super) fn bucket_totals(&self) -> Vec<Totals> {
        self.cells
            .iter()
            .map(|row| {
                let mut total = Totals::default();
                for cell in row {
                    total.requests += cell.requests;
                    total.limits += cell.limits;
                    total.errors += cell.errors;
                    total.uncached += cell.uncached;
                    total.cache_read += cell.cache_read;
                    total.cache_write += cell.cache_write;
                    total.output += cell.output;
                    total.durations.extend(&cell.durations);
                }
                total
            })
            .collect()
    }
}

pub(super) fn by_group(points: &[Point], by: By) -> Vec<(String, Totals)> {
    let mut groups: HashMap<String, Totals> = HashMap::new();
    for point in points {
        groups.entry(by.group(point)).or_default().add(point);
    }
    let mut groups: Vec<(String, Totals)> = groups.into_iter().collect();
    groups.sort_by(|a, b| {
        b.1.tokens()
            .cmp(&a.1.tokens())
            .then_with(|| b.1.requests.cmp(&a.1.requests))
            .then_with(|| a.0.cmp(&b.0))
    });
    groups
}

pub(super) fn total(points: &[Point]) -> Totals {
    let mut total = Totals::default();
    for point in points {
        total.add(point);
    }
    total
}

pub(super) fn week_hours(points: &[Point], zone: &Zone) -> Vec<Vec<f64>> {
    let mut grid = vec![vec![0.0; 24]; 7];
    for point in points {
        let local = zone.local(point.ts);
        let weekday = (local.div_euclid(DAY) + 3).rem_euclid(7);
        let hour = local.rem_euclid(DAY) / HOUR;
        let cell = usize::try_from(weekday)
            .ok()
            .zip(usize::try_from(hour).ok())
            .and_then(|(day, hour)| grid.get_mut(day).and_then(|row| row.get_mut(hour)));
        if let Some(cell) = cell {
            *cell += 1.0;
        }
    }
    grid
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(ts: i64, key: &str, tokens: i64, status: (bool, bool)) -> Point {
        Point {
            ts,
            key: Some(key.to_owned()),
            model: "m".to_owned(),
            account: Some("a".to_owned()),
            limit: status.0,
            failed: status.1,
            input: tokens,
            cache_read: tokens,
            cache_write: 0,
            output: 1,
            duration_ms: ts,
        }
    }

    #[test]
    fn buckets_end_after_now_on_local_boundaries() {
        let zone = Zone::new(9 * HOUR, None);
        let buckets = Buckets::new(Range::Month, 10 * DAY + 3 * HOUR, &zone);
        assert_eq!(buckets.starts.len(), 30);
        assert_eq!(buckets.starts.last(), Some(&(10 * DAY + 15 * HOUR - DAY)));
        assert_eq!(buckets.index(10 * DAY + 3 * HOUR), Some(29));
        assert_eq!(buckets.index(buckets.since() - 1), None);
        assert_eq!(buckets.bucket_end(29), 10 * DAY + 15 * HOUR);
        let hours = Buckets::new(Range::Day, 10 * DAY + 3 * HOUR + 5, &zone);
        assert_eq!(hours.starts.len(), 24);
        assert_eq!(hours.index(10 * DAY + 3 * HOUR + 5), Some(23));
    }

    #[test]
    fn totals_split_outcomes_and_ignore_failures_for_latency() {
        let points = [
            point(100, "a", 10, (false, false)),
            point(300, "a", 10, (false, false)),
            point(200, "b", 0, (true, false)),
            point(900, "b", 5, (false, true)),
        ];
        let mut total = total(&points);
        assert_eq!((total.requests, total.limits, total.errors), (4, 1, 1));
        assert_eq!(total.input(), 50);
        assert_eq!(total.cache_ratio(), Some(0.5));
        assert_eq!(total.percentile(0.5), Some(100));
        assert_eq!(total.percentile(0.95), Some(300));
    }

    #[test]
    fn top_groups_fold_the_tail_into_other() {
        let points: Vec<Point> = (0..5)
            .map(|n| point(n, &format!("k{n}"), n * 10, (false, false)))
            .collect();
        let groups = top_groups(&points, By::Key, 3);
        assert_eq!(groups, ["k4", "k3", OTHER]);
        assert_eq!(slot_of(&groups, "k1"), 2);
        assert_eq!(slot_of(&groups, "k3"), 1);
    }

    #[test]
    fn week_hours_use_the_local_day() {
        let zone = Zone::new(9 * HOUR, None);
        let grid = week_hours(&[point(20 * HOUR, "a", 0, (false, false))], &zone);
        assert!((grid[4][5] - 1.0).abs() < 1e-9);
    }
}
