use jiff::tz::TimeZone;
use jiff::{Timestamp, ZonedDateTime};
use unicode_segmentation::UnicodeSegmentation as _;
use unicode_width::UnicodeWidthStr as _;

const WEEK_DAYS: i64 = 7;

#[derive(Debug, Clone)]
pub(crate) struct Clock {
    pub(crate) now: i64,
    zone: TimeZone,
}

impl Clock {
    pub(crate) fn new(now: i64, zone: TimeZone) -> Self {
        Self { now, zone }
    }

    fn local(&self, ts: i64) -> Option<ZonedDateTime> {
        Timestamp::from_second(ts)
            .ok()
            .map(|timestamp| timestamp.to_zoned_datetime(self.zone.clone()))
    }

    pub(crate) fn seconds(&self, ts: i64) -> String {
        self.local(ts)
            .map_or_else(String::new, |at| at.strftime("%H:%M:%S").to_string())
    }

    pub(crate) fn when(&self, ts: i64) -> String {
        let (Some(at), Some(now)) = (self.local(ts), self.local(self.now)) else {
            return String::new();
        };
        let days = at
            .date()
            .until(now.date())
            .map_or(i64::MAX, |span| i64::from(span.get_days()).abs());
        let pattern = if at.date() == now.date() {
            "%H:%M"
        } else if days < WEEK_DAYS {
            "%a %H:%M"
        } else {
            "%b %-d %H:%M"
        };
        at.strftime(pattern).to_string()
    }

    pub(crate) fn until(&self, ts: i64) -> String {
        remaining(ts - self.now)
    }

    pub(crate) fn since(&self, ts: i64) -> String {
        ago(self.now - ts)
    }
}

pub(crate) fn remaining(secs: i64) -> String {
    let secs = secs.max(0);
    let (days, hours, minutes) = (secs / 86_400, secs % 86_400 / 3600, secs % 3600 / 60);
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes:02}m")
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        format!("{secs}s")
    }
}

pub(crate) fn ago(secs: i64) -> String {
    let secs = secs.max(0);
    if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else {
        let days = secs / 86_400;
        format!("{days} day{} ago", if days == 1 { "" } else { "s" })
    }
}

pub(crate) fn count(value: i64) -> String {
    let digits = value.unsigned_abs().to_string();
    let mut grouped = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    if value < 0 {
        format!("-{grouped}")
    } else {
        grouped
    }
}

pub(crate) fn noun(value: i64, one: &str, many: &str) -> String {
    format!("{} {}", count(value), if value == 1 { one } else { many })
}

#[expect(
    clippy::cast_precision_loss,
    reason = "token counts far below 2^52 display with one decimal"
)]
pub(crate) fn compact(value: i64) -> String {
    let scaled = |divisor: f64, suffix: &str| {
        let number = value as f64 / divisor;
        if number >= 100.0 {
            format!("{number:.0}{suffix}")
        } else {
            format!("{number:.1}{suffix}")
        }
    };
    if value < 1000 {
        value.to_string()
    } else if value < 1_000_000 {
        scaled(1e3, "k")
    } else {
        scaled(1e6, "M")
    }
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "a percentage between 0 and a few hundred fits in i64"
)]
pub(crate) fn percent(fraction: f64) -> i64 {
    (fraction * 100.0).round() as i64
}

pub(crate) fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

pub(crate) fn width(text: &str) -> usize {
    text.width()
}

pub(crate) fn truncate(text: &str, max: usize) -> String {
    if text.width() <= max {
        return text.to_owned();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for grapheme in text.graphemes(true) {
        let cells = grapheme.width();
        if used + cells + 1 > max {
            break;
        }
        out.push_str(grapheme);
        used += cells;
    }
    out.push('…');
    out
}

pub(crate) fn fit(text: &str, cells: usize) -> String {
    let mut out = truncate(text, cells);
    let pad = cells.saturating_sub(out.width());
    out.extend(std::iter::repeat_n(' ', pad));
    out
}

pub(crate) fn wrap(text: &str, cells: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.lines() {
        let mut line = String::new();
        for word in paragraph.split(' ') {
            let mut word = word.to_owned();
            while word.width() > cells && cells > 0 {
                let head: String = take_cells(&word, cells);
                if !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                }
                word = word.get(head.len()..).unwrap_or_default().to_owned();
                lines.push(head);
            }
            let extra = usize::from(!line.is_empty());
            if line.width() + extra + word.width() > cells && !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(&word);
        }
        lines.push(line);
    }
    lines
}

fn take_cells(text: &str, cells: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for grapheme in text.graphemes(true) {
        let step = grapheme.width();
        if used + step > cells {
            break;
        }
        out.push_str(grapheme);
        used += step;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clock() -> Clock {
        Clock::new(1_763_301_600, TimeZone::UTC)
    }

    #[test]
    fn times_follow_today_week_and_older_rules() {
        let clock = clock();
        assert_eq!(clock.when(1_763_301_600 + 5400), "15:30");
        assert_eq!(clock.when(1_763_301_600 + 2 * 86_400), "Tue 14:00");
        assert_eq!(clock.when(1_763_301_600 - 3 * 86_400), "Thu 14:00");
        assert_eq!(clock.when(1_763_301_600 + 4 * 86_400), "Thu 14:00");
        assert_eq!(clock.when(1_763_301_600 + 20 * 86_400), "Dec 6 14:00");
        assert_eq!(clock.seconds(1_763_301_601), "14:00:01");
    }

    #[test]
    fn times_use_the_given_zone() {
        let seoul = TimeZone::get("Asia/Seoul").unwrap();
        assert_eq!(
            Clock::new(1_763_301_600, seoul).when(1_763_301_600),
            "23:00"
        );
    }

    #[test]
    fn durations_are_short_and_human() {
        assert_eq!(remaining(4320), "1h 12m");
        assert_eq!(remaining(10_920), "3h 02m");
        assert_eq!(remaining(2460), "41m");
        assert_eq!(remaining(45), "45s");
        assert_eq!(remaining(2 * 86_400 + 3 * 3600), "2d 3h");
        assert_eq!(ago(12), "12s ago");
        assert_eq!(ago(2 * 86_400), "2 days ago");
        assert_eq!(ago(86_400), "1 day ago");
    }

    #[test]
    fn numbers_group_and_compact() {
        assert_eq!(count(1204), "1,204");
        assert_eq!(count(999), "999");
        assert_eq!(count(1_234_567), "1,234,567");
        assert_eq!(compact(812), "812");
        assert_eq!(compact(12_400), "12.4k");
        assert_eq!(compact(98_000), "98.0k");
        assert_eq!(compact(210_000), "210k");
        assert_eq!(compact(4_200_000), "4.2M");
        assert_eq!(percent(0.926), 93);
    }

    #[test]
    fn truncation_counts_display_cells() {
        assert_eq!(truncate("claude-opus", 20), "claude-opus");
        assert_eq!(truncate("claude-opus-5-5", 8), "claude-…");
        assert_eq!(truncate("한글이름입니다", 7), "한글이…");
        assert_eq!(width(&fit("한글", 6)), 6);
        assert_eq!(fit("abcdef", 4), "abc…");
    }

    #[test]
    fn wrapping_respects_width() {
        let lines = wrap("one two three four", 9);
        assert_eq!(lines, vec!["one two", "three", "four"]);
        assert_eq!(wrap("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
    }
}
