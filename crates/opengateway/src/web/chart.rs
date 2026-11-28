use std::fmt::{self, Write as _};

use jiff::Timestamp;
use jiff::tz::TimeZone;

use crate::format;

pub(super) const WIDE: f64 = 960.0;
pub(super) const HALF: f64 = 520.0;
const LEFT: f64 = 44.0;
const RIGHT: f64 = 8.0;
const TOP: f64 = 14.0;
const BOTTOM: f64 = 22.0;
const BAR_MAX: f64 = 24.0;
const RADIUS: f64 = 3.0;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Svg(String);

impl fmt::Display for Svg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

pub(super) fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

#[expect(
    clippy::cast_precision_loss,
    reason = "timestamps and counts in charts are far below 2^52"
)]
pub(super) fn float(value: i64) -> f64 {
    value as f64
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "chart positions are small and already rounded"
)]
fn whole(value: f64) -> i64 {
    value.round() as i64
}

fn num(value: f64) -> String {
    let rounded = (value * 10.0).round() / 10.0;
    if rounded.fract() == 0.0 {
        format!("{rounded:.0}")
    } else {
        format!("{rounded:.1}")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Slot {
    Series(u8),
    Ramp(u8),
    Muted,
    Warn,
    Bad,
}

impl Slot {
    pub(super) fn ramp(index: usize) -> Self {
        Self::Ramp(u8::try_from(index % 6).unwrap_or(0) + 1)
    }

    pub(super) fn nth(index: usize) -> Self {
        u8::try_from(index)
            .ok()
            .filter(|index| *index < 8)
            .map_or(Self::Muted, |index| Self::Series(index + 1))
    }

    pub(super) fn class(self) -> String {
        match self {
            Self::Series(n) => format!("s{n}"),
            Self::Ramp(n) => format!("r{n}"),
            Self::Muted => "sm".to_owned(),
            Self::Warn => "st-warn".to_owned(),
            Self::Bad => "st-bad".to_owned(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Scale {
    Percent,
    Accounts,
    Count,
    Tokens,
    Millis,
}

impl Scale {
    pub(super) fn format(self, value: f64) -> String {
        match self {
            Self::Percent => format!("{}%", format::percent(value)),
            Self::Accounts => num(value),
            Self::Count => format::count(whole(value)),
            Self::Tokens => format::compact(whole(value)),
            Self::Millis => duration(value),
        }
    }

    fn ticks(self, max: f64) -> (f64, Vec<f64>) {
        match self {
            Self::Percent => (1.0, vec![0.0, 0.25, 0.5, 0.75, 1.0]),
            Self::Accounts => {
                let top = max.max(1.0).ceil();
                let step = if top <= 5.0 { 1.0 } else { (top / 4.0).ceil() };
                let mut ticks = Vec::new();
                let mut tick = 0.0;
                while tick <= top + 1e-9 {
                    ticks.push(tick);
                    tick += step;
                }
                (top, ticks)
            }
            Self::Count | Self::Tokens | Self::Millis => {
                let step = nice_step(max.max(1.0) / 4.0);
                let top = (max.max(1.0) / step).ceil() * step;
                let mut ticks = Vec::new();
                let mut tick = 0.0;
                while tick <= top + step / 2.0 {
                    ticks.push(tick);
                    tick += step;
                }
                (top, ticks)
            }
        }
    }
}

fn nice_step(raw: f64) -> f64 {
    let power = 10_f64.powf(raw.log10().floor());
    let fraction = raw / power;
    let nice = if fraction <= 1.0 {
        1.0
    } else if fraction <= 2.0 {
        2.0
    } else if fraction <= 5.0 {
        5.0
    } else {
        10.0
    };
    nice * power
}

pub(super) fn duration(millis: f64) -> String {
    if millis <= 0.0 {
        "0".to_owned()
    } else if millis < 1000.0 {
        format!("{}ms", whole(millis))
    } else if millis < 10_000.0 {
        format!("{:.1}s", millis / 1000.0)
    } else if millis < 60_000.0 {
        format!("{}s", whole(millis / 1000.0))
    } else {
        format!(
            "{}m {:02}s",
            whole(millis) / 60_000,
            whole(millis) / 1000 % 60
        )
    }
}

#[derive(Debug, Clone)]
pub(super) struct Zone {
    zone: TimeZone,
    name: Option<String>,
}

impl Zone {
    pub(super) fn new(offset: i64, name: Option<&str>) -> Self {
        if let Some(name) = name
            && let Ok(zone) = jiff::tz::db().get(name)
        {
            return Self {
                zone,
                name: Some(name.to_owned()),
            };
        }
        let zone = i32::try_from(offset)
            .ok()
            .and_then(|seconds| jiff::tz::Offset::from_seconds(seconds).ok())
            .map_or(TimeZone::UTC, TimeZone::fixed);
        Self { zone, name: None }
    }

    pub(super) fn offset_at(&self, ts: i64) -> i64 {
        Timestamp::from_second(ts).map_or(0, |at| i64::from(self.zone.to_offset(at).seconds()))
    }

    pub(super) fn local(&self, ts: i64) -> i64 {
        ts + self.offset_at(ts)
    }

    pub(super) fn label(&self, now: i64) -> String {
        let offset = self.offset_at(now);
        let fixed = if offset == 0 {
            "UTC".to_owned()
        } else {
            let sign = if offset < 0 { '-' } else { '+' };
            let minutes = offset.abs() / 60;
            format!("UTC{sign}{:02}:{:02}", minutes / 60, minutes % 60)
        };
        match &self.name {
            Some(name) => format!("{name} ({fixed})"),
            None => fixed,
        }
    }

    pub(super) fn clock(&self, now: i64) -> format::Clock {
        format::Clock::new(now, self.zone.clone())
    }

    pub(super) fn strftime(&self, ts: i64, pattern: &str) -> String {
        Timestamp::from_second(ts).map_or_else(
            |_| String::new(),
            |at| {
                at.to_zoned_datetime(self.zone.clone())
                    .strftime(pattern)
                    .to_string()
            },
        )
    }

    pub(super) fn day_start(&self, ts: i64) -> i64 {
        Timestamp::from_second(ts)
            .ok()
            .and_then(|at| at.to_zoned_datetime(self.zone.clone()).start_of_day().ok())
            .map_or_else(
                || ts.div_euclid(86_400) * 86_400,
                |start| start.timestamp().as_second(),
            )
    }

    pub(super) fn next_day(&self, day_start: i64) -> i64 {
        self.day_start(day_start + 86_400 + 3 * 3600)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Table {
    pub(super) head: Vec<String>,
    pub(super) rows: Vec<Row>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Row {
    pub(super) from: f64,
    pub(super) to: f64,
    pub(super) cells: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Key {
    pub(super) name: String,
    pub(super) slot: Slot,
    pub(super) mark: KeyMark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum KeyMark {
    Line,
    Dashed,
    Block,
    Hatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Shape {
    Step,
    Straight,
    Projection,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Series {
    pub(super) slot: Slot,
    pub(super) shape: Shape,
    pub(super) points: Vec<(i64, Option<f64>)>,
    pub(super) until: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Band {
    pub(super) slot: Slot,
    pub(super) hatched: bool,
    pub(super) steps: Vec<(i64, i64, f64, f64)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RuleKind {
    Now,
    Reset,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Mark {
    pub(super) at: i64,
    pub(super) label: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct TimeChart {
    pub(super) label: String,
    pub(super) from: i64,
    pub(super) to: i64,
    pub(super) scale: Scale,
    pub(super) max: f64,
    pub(super) width: f64,
    pub(super) height: f64,
    pub(super) bands: Vec<Band>,
    pub(super) series: Vec<Series>,
    pub(super) rules: Vec<(RuleKind, i64)>,
    pub(super) ceiling: Option<f64>,
    pub(super) marks: Vec<Mark>,
    pub(super) shades: Vec<(i64, i64)>,
    pub(super) keys: Vec<Key>,
    pub(super) table: Table,
}

struct Plot {
    full: f64,
    width: f64,
    height: f64,
    top: f64,
}

impl Plot {
    fn new(full: f64, height: f64) -> Self {
        Self {
            full,
            width: full - LEFT - RIGHT,
            height: height - TOP - BOTTOM,
            top: TOP,
        }
    }

    fn bottom(&self) -> f64 {
        self.top + self.height
    }

    fn y(&self, value: f64, max: f64) -> f64 {
        let ratio = if max > 0.0 {
            (value / max).clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.bottom() - ratio * self.height
    }
}

struct Time {
    from: i64,
    to: i64,
    width: f64,
}

impl Time {
    fn x(&self, ts: i64) -> f64 {
        let span = (self.to - self.from).max(1);
        LEFT + float((ts - self.from).clamp(0, span)) / float(span) * self.width
    }
}

fn open(out: &mut String, plot: &Plot, height: f64, label: &str) {
    write!(
        out,
        "<svg class=\"plot {}\" viewBox=\"0 0 {} {}\" role=\"img\" aria-label=\"{}\">",
        if plot.full > HALF { "wide" } else { "half" },
        num(plot.full),
        num(height),
        escape(label)
    )
    .unwrap_or_default();
}

fn y_axis(out: &mut String, plot: &Plot, scale: Scale, top: f64, ticks: &[f64]) {
    out.push_str("<g class=\"grid\">");
    for tick in ticks {
        let y = plot.y(*tick, top);
        write!(
            out,
            "<line x1=\"{}\" x2=\"{}\" y1=\"{y}\" y2=\"{y}\"{}/><text class=\"ytick\" x=\"{}\" y=\"{}\">{}</text>",
            num(LEFT),
            num(plot.full - RIGHT),
            if *tick == 0.0 { " class=\"base\"" } else { "" },
            num(LEFT - 6.0),
            num(y + 4.0),
            escape(&scale.format(*tick)),
            y = num(y),
        )
        .unwrap_or_default();
    }
    out.push_str("</g>");
}

const STEPS: [i64; 9] = [
    3600,
    3 * 3600,
    6 * 3600,
    12 * 3600,
    86_400,
    2 * 86_400,
    7 * 86_400,
    14 * 86_400,
    28 * 86_400,
];

fn time_ticks(from: i64, to: i64, zone: &Zone) -> Vec<(i64, String)> {
    let span = (to - from).max(1);
    let step = STEPS
        .into_iter()
        .find(|step| span / step <= 7)
        .unwrap_or(28 * 86_400);
    let mut tick = if step >= 86_400 {
        zone.next_day(zone.day_start(from))
    } else {
        let offset = zone.offset_at(from);
        (from + offset).div_euclid(step) * step - offset + step
    };
    if step >= 7 * 86_400 {
        while zone.local(tick).div_euclid(86_400).rem_euclid(7) != 4 {
            tick = zone.next_day(tick);
        }
    }
    let mut ticks = Vec::new();
    while tick < to {
        let midnight = zone.day_start(tick) == tick;
        let pattern = match (step >= 86_400, midnight, span > 15 * 86_400) {
            (true, _, true) => "%b %-d",
            (true, _, false) | (false, true, _) => "%a %-d",
            (false, false, _) => "%H:%M",
        };
        ticks.push((tick, zone.strftime(tick, pattern)));
        if step >= 86_400 {
            for _ in 0..step / 86_400 {
                tick = zone.next_day(tick);
            }
        } else {
            tick += step;
        }
    }
    ticks
}

fn x_axis(out: &mut String, plot: &Plot, time: &Time, zone: &Zone) {
    out.push_str("<g class=\"xaxis\">");
    let ticks = time_ticks(time.from, time.to, zone);
    let thin = ticks.len() > 4;
    for (index, (at, label)) in ticks.into_iter().enumerate() {
        let x = time.x(at);
        write!(
            out,
            "<line x1=\"{x}\" x2=\"{x}\" y1=\"{}\" y2=\"{}\"/><text class=\"xtick{}\" x=\"{x}\" y=\"{}\">{}</text>",
            num(plot.bottom()),
            num(plot.bottom() + 4.0),
            if thin && index % 2 == 1 { " odd" } else { "" },
            num(plot.bottom() + 16.0),
            escape(&label),
            x = num(x),
        )
        .unwrap_or_default();
    }
    out.push_str("</g>");
}

fn hits(out: &mut String, plot: &Plot, rows: &[Row]) {
    out.push_str("<g class=\"hits\">");
    for (index, row) in rows.iter().enumerate() {
        write!(
            out,
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" data-i=\"{index}\" data-x=\"{}\"/>",
            num(row.from),
            num(plot.top),
            num((row.to - row.from).max(1.0)),
            num(plot.height),
            num(f64::midpoint(row.from, row.to)),
        )
        .unwrap_or_default();
    }
    write!(
        out,
        "</g><line class=\"crosshair\" x1=\"0\" x2=\"0\" y1=\"{}\" y2=\"{}\" visibility=\"hidden\"/>",
        num(plot.top),
        num(plot.bottom())
    )
    .unwrap_or_default();
}

fn finish(out: &mut String, keys: &[Key], table: &Table) {
    out.push_str("</svg><div class=\"tip\" hidden></div>");
    if keys.len() > 1 {
        legend(out, keys);
    }
    data_table(out, table);
}

pub(super) fn legend(out: &mut String, keys: &[Key]) {
    out.push_str("<ul class=\"legend\">");
    for key in keys {
        let mark = match key.mark {
            KeyMark::Line => "<line x1=\"0\" x2=\"14\" y1=\"5\" y2=\"5\"/>",
            KeyMark::Dashed => "<line class=\"dash\" x1=\"0\" x2=\"14\" y1=\"5\" y2=\"5\"/>",
            KeyMark::Block => "<rect x=\"2\" y=\"0\" width=\"10\" height=\"10\" rx=\"2\"/>",
            KeyMark::Hatch => {
                "<rect class=\"hatch\" x=\"2\" y=\"0\" width=\"10\" height=\"10\" rx=\"2\"/>"
            }
        };
        write!(
            out,
            "<li><svg class=\"key {}\" viewBox=\"0 0 14 10\" aria-hidden=\"true\">{mark}</svg>{}</li>",
            key.slot.class(),
            escape(&key.name)
        )
        .unwrap_or_default();
    }
    out.push_str("</ul>");
}

fn data_table(out: &mut String, table: &Table) {
    out.push_str("<details class=\"data\"><summary>Show data</summary><div class=\"scroll\"><table><thead><tr>");
    for (index, head) in table.head.iter().enumerate() {
        write!(
            out,
            "<th{}>{}</th>",
            if index > 0 { " class=\"num\"" } else { "" },
            escape(head)
        )
        .unwrap_or_default();
    }
    out.push_str("</tr></thead><tbody>");
    for row in &table.rows {
        out.push_str("<tr>");
        for (index, cell) in row.cells.iter().enumerate() {
            write!(
                out,
                "<td{}>{}</td>",
                if index > 0 { " class=\"num\"" } else { "" },
                escape(cell)
            )
            .unwrap_or_default();
        }
        out.push_str("</tr>");
    }
    out.push_str("</tbody></table></div></details>");
}

impl TimeChart {
    pub(super) fn columns(from: i64, to: i64, count: i64) -> Vec<(i64, i64)> {
        let count = count.max(1);
        let span = (to - from).max(1);
        (0..count)
            .map(|index| {
                (
                    from + span * index / count,
                    from + span * (index + 1) / count,
                )
            })
            .collect()
    }

    pub(super) fn row(&self, start: i64, end: i64, cells: Vec<String>) -> Row {
        let time = self.time();
        Row {
            from: time.x(start),
            to: time.x(end),
            cells,
        }
    }

    fn time(&self) -> Time {
        Time {
            from: self.from,
            to: self.to,
            width: self.width - LEFT - RIGHT,
        }
    }

    pub(super) fn render(&self, zone: &Zone) -> Svg {
        let plot = Plot::new(self.width, self.height);
        let time = self.time();
        let (top, ticks) = self.scale.ticks(self.max);
        let mut out = String::from("<figure class=\"chart\" data-chart>");
        open(&mut out, &plot, self.height, &self.label);
        out.push_str(
            "<defs><pattern id=\"hatch\" width=\"6\" height=\"6\" patternUnits=\"userSpaceOnUse\" patternTransform=\"rotate(45)\"><line x1=\"0\" y1=\"0\" x2=\"0\" y2=\"6\"/></pattern></defs>",
        );
        y_axis(&mut out, &plot, self.scale, top, &ticks);
        for (from, to) in &self.shades {
            write!(
                out,
                "<rect class=\"shade\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"/>",
                num(time.x(*from)),
                num(plot.top),
                num((time.x(*to) - time.x(*from)).max(0.0)),
                num(plot.height)
            )
            .unwrap_or_default();
        }
        for band in &self.bands {
            band_path(&mut out, band, &plot, &time, top);
        }
        for (kind, at) in &self.rules {
            let class = match kind {
                RuleKind::Now => "rule now",
                RuleKind::Reset => "rule reset",
            };
            let x = num(time.x(*at));
            write!(
                out,
                "<line class=\"{class}\" x1=\"{x}\" x2=\"{x}\" y1=\"{}\" y2=\"{}\"/>",
                num(plot.top),
                num(plot.bottom())
            )
            .unwrap_or_default();
        }
        if let Some(ceiling) = self.ceiling {
            let y = num(plot.y(ceiling, top));
            write!(
                out,
                "<line class=\"rule ceiling\" x1=\"{}\" x2=\"{}\" y1=\"{y}\" y2=\"{y}\"/>",
                num(LEFT),
                num(plot.full - RIGHT)
            )
            .unwrap_or_default();
        }
        for series in &self.series {
            line_path(&mut out, series, &plot, &time, top);
        }
        for mark in &self.marks {
            let x = time.x(mark.at);
            let y = plot.top - 2.0;
            write!(
                out,
                "<path class=\"mark\" d=\"M{} {}l-5 -8h10z\"><title>{}</title></path>",
                num(x),
                num(y),
                escape(&mark.label)
            )
            .unwrap_or_default();
        }
        x_axis(&mut out, &plot, &time, zone);
        hits(&mut out, &plot, &self.table.rows);
        finish(&mut out, &self.keys, &self.table);
        out.push_str("</figure>");
        Svg(out)
    }
}

fn band_path(out: &mut String, band: &Band, plot: &Plot, time: &Time, top: f64) {
    let mut d = String::new();
    let mut run: Vec<&(i64, i64, f64, f64)> = Vec::new();
    let flush = |d: &mut String, run: &[&(i64, i64, f64, f64)]| {
        let (Some(first), Some(last)) = (run.first(), run.last()) else {
            return;
        };
        write!(d, "M{} {}", num(time.x(first.0)), num(plot.y(first.3, top))).unwrap_or_default();
        for step in run {
            write!(d, "V{}H{}", num(plot.y(step.3, top)), num(time.x(step.1))).unwrap_or_default();
        }
        write!(d, "V{}", num(plot.y(last.2, top))).unwrap_or_default();
        for step in run.iter().rev() {
            write!(d, "V{}H{}", num(plot.y(step.2, top)), num(time.x(step.0))).unwrap_or_default();
        }
        d.push('Z');
    };
    for step in &band.steps {
        if run.last().is_some_and(|last| last.1 != step.0) {
            flush(&mut d, &run);
            run.clear();
        }
        if step.3 > step.2 && step.1 > step.0 {
            run.push(step);
        } else {
            flush(&mut d, &run);
            run.clear();
        }
    }
    flush(&mut d, &run);
    if !d.is_empty() {
        write!(
            out,
            "<path class=\"band {}{}\" d=\"{d}\"/>",
            band.slot.class(),
            if band.hatched { " hatch" } else { "" }
        )
        .unwrap_or_default();
        if band.hatched {
            write!(out, "<path class=\"band-hatch\" d=\"{d}\"/>").unwrap_or_default();
        }
    }
}

fn line_path(out: &mut String, series: &Series, plot: &Plot, time: &Time, top: f64) {
    let mut d = String::new();
    let mut last: Option<f64> = None;
    for (index, (at, value)) in series.points.iter().enumerate() {
        let x = time.x(*at);
        match (value, last, series.shape) {
            (Some(value), Some(_), Shape::Step) => {
                write!(d, "H{}V{}", num(x), num(plot.y(*value, top))).unwrap_or_default();
            }
            (Some(value), Some(_), Shape::Straight | Shape::Projection) => {
                write!(d, "L{} {}", num(x), num(plot.y(*value, top))).unwrap_or_default();
            }
            (Some(value), None, _) => {
                write!(d, "M{} {}", num(x), num(plot.y(*value, top))).unwrap_or_default();
            }
            (None, Some(_), Shape::Step) => {
                write!(d, "H{}", num(x)).unwrap_or_default();
            }
            (None, _, _) => {}
        }
        last = *value;
        let final_point = index + 1 == series.points.len();
        if final_point && series.shape == Shape::Step && last.is_some() {
            write!(d, "H{}", num(time.x(series.until.max(*at)))).unwrap_or_default();
        }
    }
    if series.shape == Shape::Straight {
        let value_at = |index: Option<usize>| {
            index
                .and_then(|index| series.points.get(index))
                .and_then(|(_, value)| *value)
        };
        for (index, (at, value)) in series.points.iter().enumerate() {
            let alone = value.is_some()
                && value_at(index.checked_sub(1)).is_none()
                && value_at(index.checked_add(1)).is_none();
            if alone && let Some(value) = value {
                write!(
                    out,
                    "<circle class=\"dot {}\" cx=\"{}\" cy=\"{}\" r=\"3\"/>",
                    series.slot.class(),
                    num(time.x(*at)),
                    num(plot.y(*value, top))
                )
                .unwrap_or_default();
            }
        }
    }
    if !d.is_empty() {
        write!(
            out,
            "<path class=\"line {}{}\" d=\"{d}\"/>",
            series.slot.class(),
            if series.shape == Shape::Projection {
                " dash"
            } else {
                ""
            }
        )
        .unwrap_or_default();
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Bar {
    pub(super) tick: String,
    pub(super) head: String,
    pub(super) values: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct BarChart {
    pub(super) label: String,
    pub(super) scale: Scale,
    pub(super) width: f64,
    pub(super) height: f64,
    pub(super) keys: Vec<Key>,
    pub(super) bars: Vec<Bar>,
    pub(super) first: String,
}

impl BarChart {
    pub(super) fn render(&self) -> Svg {
        let plot = Plot::new(self.width, self.height);
        let max = self
            .bars
            .iter()
            .map(|bar| bar.values.iter().sum::<f64>())
            .fold(0.0, f64::max);
        let (top, ticks) = self.scale.ticks(max);
        let mut out = String::from("<figure class=\"chart\" data-chart>");
        open(&mut out, &plot, self.height, &self.label);
        y_axis(&mut out, &plot, self.scale, top, &ticks);
        let count = float(i64::try_from(self.bars.len()).unwrap_or(i64::MAX)).max(1.0);
        let slot = plot.width / count;
        let width = (slot * 0.7).clamp(1.0, BAR_MAX);
        let mut rows = Vec::new();
        out.push_str("<g class=\"bars\">");
        for (index, bar) in self.bars.iter().enumerate() {
            let left = LEFT + slot * float(i64::try_from(index).unwrap_or(0));
            let x = left + (slot - width) / 2.0;
            let mut base = 0.0;
            let last = bar.values.iter().rposition(|value| *value > 0.0);
            for (series, value) in bar.values.iter().enumerate() {
                if *value <= 0.0 {
                    continue;
                }
                let y0 = plot.y(base, top);
                let y1 = plot.y(base + value, top);
                base += value;
                let height = (y0 - y1).max(0.5);
                let class = self
                    .keys
                    .get(series)
                    .map_or_else(|| Slot::Muted.class(), |key| key.slot.class());
                bar_rect(&mut out, &class, x, y0, width, height, last == Some(series));
            }
            rows.push(Row {
                from: left,
                to: left + slot,
                cells: std::iter::once(bar.head.clone())
                    .chain(bar.values.iter().map(|value| self.scale.format(*value)))
                    .collect(),
            });
        }
        out.push_str("</g><g class=\"xaxis\">");
        let mut shown = 0;
        for (index, bar) in self.bars.iter().enumerate() {
            if bar.tick.is_empty() {
                continue;
            }
            let x = LEFT + slot * (float(i64::try_from(index).unwrap_or(0)) + 0.5);
            write!(
                out,
                "<text class=\"xtick{}\" x=\"{}\" y=\"{}\">{}</text>",
                if shown % 2 == 1 { " odd" } else { "" },
                num(x),
                num(plot.bottom() + 16.0),
                escape(&bar.tick)
            )
            .unwrap_or_default();
            shown += 1;
        }
        out.push_str("</g>");
        hits(&mut out, &plot, &rows);
        let table = Table {
            head: std::iter::once(self.first.clone())
                .chain(self.keys.iter().map(|key| key.name.clone()))
                .collect(),
            rows,
        };
        finish(&mut out, &self.keys, &table);
        out.push_str("</figure>");
        Svg(out)
    }
}

fn bar_rect(
    out: &mut String,
    class: &str,
    x: f64,
    bottom: f64,
    width: f64,
    height: f64,
    cap: bool,
) {
    let top = bottom - height;
    let r = if cap {
        RADIUS.min(height).min(width / 2.0)
    } else {
        0.0
    };
    write!(
        out,
        "<path class=\"bar {class}\" d=\"M{} {}V{}Q{} {} {} {}H{}Q{} {} {} {}V{}Z\"/>",
        num(x),
        num(bottom),
        num(top + r),
        num(x),
        num(top),
        num(x + r),
        num(top),
        num(x + width - r),
        num(x + width),
        num(top),
        num(x + width),
        num(top + r),
        num(bottom)
    )
    .unwrap_or_default();
}

pub(super) struct Heat<'a> {
    pub(super) label: String,
    pub(super) rows: &'a [String],
    pub(super) columns: &'a [String],
    pub(super) values: &'a [Vec<f64>],
    pub(super) format: Scale,
}

impl Heat<'_> {
    pub(super) fn render(&self) -> Svg {
        let cell = 22.0;
        let left = 40.0;
        let top = 4.0;
        let max = self.values.iter().flatten().copied().fold(0.0, f64::max);
        let columns = float(i64::try_from(self.columns.len()).unwrap_or(0));
        let rows = float(i64::try_from(self.rows.len()).unwrap_or(0));
        let width = left + columns * cell + 4.0;
        let height = top + rows * cell + 20.0;
        let mut out = String::from("<figure class=\"chart heat\">");
        write!(
            out,
            "<svg class=\"plot\" viewBox=\"0 0 {} {}\" role=\"img\" aria-label=\"{}\">",
            num(width),
            num(height),
            escape(&self.label)
        )
        .unwrap_or_default();
        for (r, name) in self.rows.iter().enumerate() {
            let y = top + float(i64::try_from(r).unwrap_or(0)) * cell;
            write!(
                out,
                "<text class=\"ytick\" x=\"{}\" y=\"{}\">{}</text>",
                num(left - 6.0),
                num(y + cell / 2.0 + 4.0),
                escape(name)
            )
            .unwrap_or_default();
            for (c, column) in self.columns.iter().enumerate() {
                let value = self
                    .values
                    .get(r)
                    .and_then(|row| row.get(c))
                    .copied()
                    .unwrap_or_default();
                let level = if value <= 0.0 || max <= 0.0 {
                    0
                } else {
                    1 + whole((value / max * 5.0).ceil() - 1.0).clamp(0, 4)
                };
                write!(
                    out,
                    "<rect class=\"cell q{level}\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"2\"><title>{} {}: {}</title></rect>",
                    num(left + float(i64::try_from(c).unwrap_or(0)) * cell + 1.0),
                    num(y + 1.0),
                    num(cell - 2.0),
                    num(cell - 2.0),
                    escape(name),
                    escape(column),
                    escape(&self.format.format(value))
                )
                .unwrap_or_default();
            }
        }
        for (c, column) in self.columns.iter().enumerate() {
            if c % 3 != 0 {
                continue;
            }
            write!(
                out,
                "<text class=\"xtick\" x=\"{}\" y=\"{}\">{}</text>",
                num(left + float(i64::try_from(c).unwrap_or(0)) * cell + cell / 2.0),
                num(top + rows * cell + 14.0),
                escape(column)
            )
            .unwrap_or_default();
        }
        out.push_str("</svg>");
        data_table(
            &mut out,
            &Table {
                head: std::iter::once(String::new())
                    .chain(self.columns.iter().cloned())
                    .collect(),
                rows: self
                    .rows
                    .iter()
                    .enumerate()
                    .map(|(r, name)| Row {
                        from: 0.0,
                        to: 0.0,
                        cells: std::iter::once(name.clone())
                            .chain((0..self.columns.len()).map(|c| {
                                self.format.format(
                                    self.values
                                        .get(r)
                                        .and_then(|row| row.get(c))
                                        .copied()
                                        .unwrap_or_default(),
                                )
                            }))
                            .collect(),
                    })
                    .collect(),
            },
        );
        out.push_str("</figure>");
        Svg(out)
    }
}

pub(super) fn spark(values: &[f64], label: &str) -> Svg {
    let max = values.iter().copied().fold(0.0, f64::max);
    let count = float(i64::try_from(values.len()).unwrap_or(0)).max(1.0);
    let width = 90.0;
    let height = 16.0;
    let slot = width / count;
    let mut out = format!(
        "<svg class=\"spark\" viewBox=\"0 0 {} {}\" role=\"img\" aria-label=\"{}\">",
        num(width),
        num(height),
        escape(label)
    );
    write!(
        out,
        "<line class=\"base\" x1=\"0\" x2=\"{}\" y1=\"{h}\" y2=\"{h}\"/>",
        num(width),
        h = num(height - 0.5)
    )
    .unwrap_or_default();
    for (index, value) in values.iter().enumerate() {
        if *value <= 0.0 || max <= 0.0 {
            continue;
        }
        let bar = (value / max * (height - 1.0)).max(1.5);
        write!(
            out,
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"/>",
            num(float(i64::try_from(index).unwrap_or(0)) * slot + 0.5),
            num(height - 0.5 - bar),
            num((slot - 1.0).max(0.5)),
            num(bar)
        )
        .unwrap_or_default();
    }
    out.push_str("</svg>");
    Svg(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Tone {
    Normal,
    High,
    Full,
    Stale,
}

pub(super) fn meter(used: f64, tone: Tone, label: &str) -> Svg {
    let width = (used.clamp(0.0, 1.0) * 100.0).max(if used > 0.0 { 1.0 } else { 0.0 });
    let class = match tone {
        Tone::Normal => "",
        Tone::High => " high",
        Tone::Full => " full",
        Tone::Stale => " stale",
    };
    Svg(format!(
        "<svg class=\"meter{class}\" viewBox=\"0 0 100 6\" preserveAspectRatio=\"none\" role=\"img\" aria-label=\"{}\"><rect class=\"track\" width=\"100\" height=\"6\" rx=\"3\"/><rect class=\"fill\" width=\"{}\" height=\"6\" rx=\"3\"/></svg>",
        escape(label),
        num(width)
    ))
}

pub(super) fn share(percent: f64, label: &str) -> Svg {
    Svg(format!(
        "<svg class=\"share\" viewBox=\"0 0 100 6\" preserveAspectRatio=\"none\" role=\"img\" aria-label=\"{}\"><rect class=\"track\" width=\"100\" height=\"6\" rx=\"3\"/><rect class=\"fill\" width=\"{}\" height=\"6\" rx=\"3\"/></svg>",
        escape(label),
        num(percent.clamp(0.0, 100.0))
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_markup_in_labels() {
        assert_eq!(
            escape("<a href=\"x\">&'"),
            "&lt;a href=&quot;x&quot;&gt;&amp;&#39;"
        );
    }

    #[test]
    fn slots_stop_at_eight() {
        assert_eq!(Slot::nth(0), Slot::Series(1));
        assert_eq!(Slot::nth(7), Slot::Series(8));
        assert_eq!(Slot::nth(8), Slot::Muted);
    }

    #[test]
    fn count_ticks_are_round() {
        let (top, ticks) = Scale::Count.ticks(173.0);
        assert!((top - 200.0).abs() < 1e-9);
        assert_eq!(ticks, [0.0, 50.0, 100.0, 150.0, 200.0]);
        let (top, _) = Scale::Percent.ticks(0.2);
        assert!((top - 1.0).abs() < 1e-9);
    }

    #[test]
    fn zone_labels_and_day_starts() {
        let zone = Zone::new(9 * 3600, None);
        assert_eq!(zone.label(0), "UTC+09:00");
        assert_eq!(zone.day_start(15 * 3600), 15 * 3600);
        assert_eq!(zone.day_start(15 * 3600 - 1), 15 * 3600 - 86_400);
        assert_eq!(Zone::new(-90 * 60, None).label(0), "UTC-01:30");
        assert_eq!(Zone::new(3600, Some("Not/AZone")).label(0), "UTC+01:00");
    }

    #[test]
    fn named_zones_follow_daylight_saving() {
        let zone = Zone::new(0, Some("America/New_York"));
        let summer = 1_782_000_000;
        let winter = 1_795_000_000;
        assert_eq!(zone.offset_at(summer), -4 * 3600);
        assert_eq!(zone.offset_at(winter), -5 * 3600);
        assert_eq!(zone.local(zone.day_start(summer)).rem_euclid(86_400), 0);
        assert_eq!(zone.local(zone.day_start(winter)).rem_euclid(86_400), 0);
        let transition = 1_793_512_800;
        let day = zone.day_start(transition);
        assert_eq!(zone.next_day(day) - day, 25 * 3600);
        assert!(
            zone.label(winter)
                .starts_with("America/New_York (UTC-05:00)")
        );
    }

    #[test]
    fn time_ticks_fall_on_local_boundaries() {
        let zone = Zone::new(0, None);
        let ticks = time_ticks(0, 86_400, &zone);
        let ats: Vec<i64> = ticks.iter().map(|(at, _)| *at).collect();
        assert_eq!(ats, [21_600, 43_200, 64_800]);
        assert_eq!(
            ticks.first().map(|(_, label)| label.as_str()),
            Some("06:00")
        );
        let days = time_ticks(0, 7 * 86_400, &zone);
        assert_eq!(days.first().map(|(_, label)| label.as_str()), Some("Fri 2"));
    }

    #[test]
    fn step_lines_hold_until_the_next_reading_and_break_on_gaps() {
        let chart = TimeChart {
            label: "usage".to_owned(),
            from: 0,
            to: 100,
            scale: Scale::Percent,
            max: 1.0,
            width: 640.0,
            height: 120.0,
            bands: Vec::new(),
            series: vec![Series {
                slot: Slot::nth(0),
                shape: Shape::Step,
                points: vec![(0, Some(0.5)), (50, None), (60, Some(0.25))],
                until: 80,
            }],
            rules: vec![(RuleKind::Now, 80)],
            ceiling: None,
            marks: Vec::new(),
            shades: Vec::new(),
            keys: Vec::new(),
            table: Table {
                head: vec!["When".to_owned(), "Used".to_owned()],
                rows: Vec::new(),
            },
        };
        let svg = chart.render(&Zone::new(0, None)).0;
        assert!(svg.contains("d=\"M44 56H338M396.8 77H514.4\""), "{svg}");
        assert_eq!(svg, chart.render(&Zone::new(0, None)).0);
    }

    #[test]
    fn stacked_bars_round_only_the_top() {
        let chart = BarChart {
            label: "requests".to_owned(),
            scale: Scale::Count,
            width: HALF,
            height: 120.0,
            keys: vec![
                Key {
                    name: "ok".to_owned(),
                    slot: Slot::nth(0),
                    mark: KeyMark::Block,
                },
                Key {
                    name: "<error>".to_owned(),
                    slot: Slot::nth(1),
                    mark: KeyMark::Block,
                },
            ],
            bars: vec![Bar {
                tick: "Mon".to_owned(),
                head: "Mon 09:00".to_owned(),
                values: vec![3.0, 1.0],
            }],
            first: "Hour".to_owned(),
        };
        let svg = chart.render().0;
        assert_eq!(svg.matches("class=\"bar ").count(), 2);
        assert!(svg.contains("&lt;error&gt;"));
        assert!(!svg.contains("<error>"));
        assert!(svg.contains("<td class=\"num\">3</td>"));
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(duration(0.0), "0");
        assert_eq!(duration(420.0), "420ms");
        assert_eq!(duration(4_300.0), "4.3s");
        assert_eq!(duration(50_000.0), "50s");
        assert_eq!(duration(100_000.0), "1m 40s");
    }

    #[test]
    fn lone_points_are_drawn_as_dots() {
        let chart = TimeChart {
            label: "cache".to_owned(),
            from: 0,
            to: 100,
            scale: Scale::Percent,
            max: 1.0,
            width: HALF,
            height: 120.0,
            bands: Vec::new(),
            series: vec![Series {
                slot: Slot::nth(0),
                shape: Shape::Straight,
                points: vec![
                    (10, Some(0.5)),
                    (30, None),
                    (50, Some(0.2)),
                    (70, Some(0.4)),
                ],
                until: 100,
            }],
            rules: Vec::new(),
            ceiling: None,
            marks: Vec::new(),
            shades: Vec::new(),
            keys: Vec::new(),
            table: Table {
                head: Vec::new(),
                rows: Vec::new(),
            },
        };
        let svg = chart.render(&Zone::new(0, None)).0;
        assert_eq!(svg.matches("<circle class=\"dot").count(), 1);
    }

    #[test]
    fn meters_clamp_and_name_themselves() {
        let svg = meter(1.4, Tone::Full, "5 hours 140% used").0;
        assert!(svg.contains("width=\"100\" height=\"6\" rx=\"3\"/></svg>"));
        assert!(svg.contains("aria-label=\"5 hours 140% used\""));
    }
}
