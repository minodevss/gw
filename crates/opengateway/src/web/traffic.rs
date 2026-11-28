use std::collections::HashMap;

use askama::Template;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Response;

use super::chart::{
    Bar, BarChart, Heat, Key, KeyMark, Scale, Series, Shape, Slot, Svg, Table, TimeChart, duration,
    float,
};
use super::frame::{Ctx, Frame, Tab};
use super::stats::{self, BYS, Buckets, By, Grid, OTHER, RANGES, Range, Totals};
use super::{Web, failure, fragment, html};
use crate::format;
use crate::ops;
use crate::store::Point;

const GROUPS: usize = 7;
const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

#[derive(Template)]
#[template(path = "admin/traffic.html")]
struct Page {
    frame: Frame,
    live: String,
}

struct Choice {
    query: String,
    label: &'static str,
    current: bool,
}

#[derive(Template)]
#[template(path = "admin/traffic_live.html")]
struct Live {
    query: String,
    ranges: Vec<Choice>,
    bys: Vec<Choice>,
    range: &'static str,
    by: &'static str,
    bucket: &'static str,
    empty: bool,
    facts: Vec<(String, String)>,
    requests: Svg,
    tokens: Svg,
    rows: Vec<GroupRow>,
    cache: Svg,
    latency: Svg,
    heat: Svg,
}

struct GroupRow {
    name: String,
    href: Option<String>,
    requests: String,
    input: String,
    cached: String,
    output: String,
    limits: String,
    errors: String,
    p50: String,
    p95: String,
    share: Svg,
}

pub(super) async fn page(
    State(web): State<Web>,
    ctx: Ctx,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    match build(&web, &ctx, &query).await {
        Ok(live) => html(
            StatusCode::OK,
            &Page {
                frame: ctx.frame(Some(Tab::Traffic), "Traffic"),
                live,
            },
        ),
        Err(error) => failure(&error),
    }
}

pub(super) async fn live(
    State(web): State<Web>,
    ctx: Ctx,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    fragment(build(&web, &ctx, &query).await)
}

async fn build(web: &Web, ctx: &Ctx, query: &HashMap<String, String>) -> anyhow::Result<String> {
    let range = Range::parse(query.get("range").map(String::as_str));
    let by = By::parse(query.get("by").map(String::as_str));
    let buckets = Buckets::new(range, ctx.now, &ctx.zone);
    let since = buckets.since();
    let points = ops::with_store(&web.0.home, move |store| store.log_points(since)).await?;
    let groups = stats::top_groups(&points, by, GROUPS);
    let grid = Grid::new(&points, &buckets, by, &groups);
    let mut total = stats::total(&points);
    let mut facts = vec![
        ("requests".to_owned(), format::count(total.requests)),
        ("tokens in".to_owned(), format::compact(total.input())),
        ("tokens out".to_owned(), format::compact(total.output)),
    ];
    if let Some(ratio) = total.cache_ratio() {
        facts.push((
            "of input read from cache".to_owned(),
            format!("{}%", format::percent(ratio)),
        ));
    }
    if total.limits > 0 {
        facts.push(("limits".to_owned(), format::count(total.limits)));
    }
    if total.errors > 0 {
        facts.push(("errors".to_owned(), format::count(total.errors)));
    }
    if let Some(p50) = total.percentile(0.5) {
        facts.push(("median request".to_owned(), duration(float(p50))));
    }
    let keys: Vec<Key> = groups
        .iter()
        .enumerate()
        .map(|(index, name)| Key {
            name: name.clone(),
            slot: slot(by, name, index),
            mark: KeyMark::Block,
        })
        .collect();
    let href = |range: Range, by: By| format!("range={}&by={}", range.query(), by.query());
    let rows = by_group_rows(&points, by);
    let live = Live {
        query: href(range, by),
        ranges: RANGES
            .into_iter()
            .map(|choice| Choice {
                query: href(choice, by),
                label: choice.label(),
                current: choice == range,
            })
            .collect(),
        bys: BYS
            .into_iter()
            .map(|choice| Choice {
                query: href(range, choice),
                label: choice.label(),
                current: choice == by,
            })
            .collect(),
        range: range.label(),
        by: by.label(),
        bucket: bucket_name(buckets.size),
        empty: points.is_empty(),
        facts,
        requests: bars(&grid, &buckets, &keys, range, ctx, Measure::Requests),
        tokens: bars(&grid, &buckets, &keys, range, ctx, Measure::Tokens),
        rows,
        cache: cache_chart(&grid, &buckets, ctx),
        latency: latency_chart(&grid, &buckets, ctx),
        heat: heat(&points, ctx, range),
    };
    Ok(live.render()?)
}

fn slot(by: By, name: &str, index: usize) -> Slot {
    match (by, name) {
        (By::Outcome, "ok") | (By::Key | By::Model | By::Account, OTHER) => Slot::Muted,
        (By::Outcome, "limit") => Slot::Warn,
        (By::Outcome, _) => Slot::Bad,
        (By::Key | By::Model | By::Account, _) => Slot::nth(index),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Measure {
    Requests,
    Tokens,
}

fn tick(start: i64, range: Range, ctx: &Ctx) -> String {
    let local = ctx.zone.local(start);
    match range {
        Range::Day if local.rem_euclid(6 * 3600) == 0 => ctx.zone.strftime(start, "%H:%M"),
        Range::Week if local.rem_euclid(86_400) == 0 => ctx.zone.strftime(start, "%a %-d"),
        Range::Month if (local.div_euclid(86_400) + 3).rem_euclid(7) == 0 => {
            ctx.zone.strftime(start, "%b %-d")
        }
        Range::Day | Range::Week | Range::Month => String::new(),
    }
}

fn head(start: i64, end: i64, ctx: &Ctx) -> String {
    if end - start >= 23 * 3600 {
        ctx.zone.strftime(start, "%a %b %-d")
    } else {
        format!(
            "{}–{}",
            ctx.zone.strftime(start, "%a %b %-d %H:%M"),
            ctx.zone.strftime(end, "%H:%M")
        )
    }
}

fn bars(
    grid: &Grid,
    buckets: &Buckets,
    keys: &[Key],
    range: Range,
    ctx: &Ctx,
    measure: Measure,
) -> Svg {
    let bars = buckets
        .starts
        .iter()
        .zip(&grid.cells)
        .enumerate()
        .map(|(index, (start, cells))| Bar {
            tick: tick(*start, range, ctx),
            head: head(*start, buckets.bucket_end(index), ctx),
            values: cells
                .iter()
                .map(|cell| match measure {
                    Measure::Requests => float(cell.requests),
                    Measure::Tokens => float(cell.tokens()),
                })
                .collect(),
        })
        .collect();
    let (label, scale) = match measure {
        Measure::Requests => ("Requests", Scale::Count),
        Measure::Tokens => ("Tokens, input and output", Scale::Tokens),
    };
    BarChart {
        label: format!("{label} per {}", bucket_name(buckets.size)),
        scale,
        width: super::chart::HALF,
        height: 200.0,
        keys: keys.to_vec(),
        bars,
        first: "Period".to_owned(),
    }
    .render()
}

fn bucket_name(size: i64) -> &'static str {
    match size {
        3600 => "hour",
        86_400 => "day",
        _ => "6 hours",
    }
}

fn line_chart(
    label: String,
    scale: Scale,
    buckets: &Buckets,
    lines: &[(String, Slot, Vec<Option<f64>>)],
    ctx: &Ctx,
) -> Svg {
    let from = buckets.since();
    let to = buckets.end;
    let max = lines
        .iter()
        .flat_map(|(_, _, values)| values.iter().flatten())
        .copied()
        .fold(0.0, f64::max);
    let mut chart = TimeChart {
        label,
        from,
        to,
        scale,
        max,
        width: super::chart::HALF,
        height: 180.0,
        bands: Vec::new(),
        series: lines
            .iter()
            .map(|(_, slot, values)| Series {
                slot: *slot,
                shape: Shape::Straight,
                points: buckets
                    .starts
                    .iter()
                    .zip(values)
                    .enumerate()
                    .map(|(index, (start, value))| {
                        (i64::midpoint(*start, buckets.bucket_end(index)), *value)
                    })
                    .collect(),
                until: to,
            })
            .collect(),
        rules: Vec::new(),
        ceiling: None,
        marks: Vec::new(),
        shades: Vec::new(),
        keys: lines
            .iter()
            .map(|(name, slot, _)| Key {
                name: name.clone(),
                slot: *slot,
                mark: KeyMark::Line,
            })
            .collect(),
        table: Table {
            head: Vec::new(),
            rows: Vec::new(),
        },
    };
    let rows = buckets
        .starts
        .iter()
        .enumerate()
        .map(|(index, start)| {
            chart.row(
                *start,
                buckets.bucket_end(index),
                std::iter::once(head(*start, buckets.bucket_end(index), ctx))
                    .chain(lines.iter().map(|(_, _, values)| {
                        values
                            .get(index)
                            .copied()
                            .flatten()
                            .map_or_else(|| "—".to_owned(), |value| scale.format(value))
                    }))
                    .collect(),
            )
        })
        .collect();
    chart.table = Table {
        head: std::iter::once("Period".to_owned())
            .chain(lines.iter().map(|(name, _, _)| name.clone()))
            .collect(),
        rows,
    };
    chart.render(&ctx.zone)
}

fn complete(buckets: &Buckets, ctx: &Ctx, values: Vec<Option<f64>>) -> Vec<Option<f64>> {
    values
        .into_iter()
        .enumerate()
        .map(|(index, value)| value.filter(|_| buckets.bucket_end(index) <= ctx.now))
        .collect()
}

fn cache_chart(grid: &Grid, buckets: &Buckets, ctx: &Ctx) -> Svg {
    let values = complete(
        buckets,
        ctx,
        grid.bucket_totals()
            .iter()
            .map(Totals::cache_ratio)
            .collect(),
    );
    line_chart(
        "Share of input tokens read from cache".to_owned(),
        Scale::Percent,
        buckets,
        &[("read from cache".to_owned(), Slot::nth(0), values)],
        ctx,
    )
}

fn latency_chart(grid: &Grid, buckets: &Buckets, ctx: &Ctx) -> Svg {
    let mut totals = grid.bucket_totals();
    let p50 = complete(
        buckets,
        ctx,
        totals
            .iter_mut()
            .map(|total| total.percentile(0.5).map(float))
            .collect(),
    );
    let p95 = complete(
        buckets,
        ctx,
        totals
            .iter_mut()
            .map(|total| total.percentile(0.95).map(float))
            .collect(),
    );
    line_chart(
        "Request time, median and 95th percentile".to_owned(),
        Scale::Millis,
        buckets,
        &[
            ("median".to_owned(), Slot::nth(0), p50),
            ("95th percentile".to_owned(), Slot::nth(1), p95),
        ],
        ctx,
    )
}

fn heat(points: &[Point], ctx: &Ctx, range: Range) -> Svg {
    let values = stats::week_hours(points, &ctx.zone);
    let rows: Vec<String> = WEEKDAYS.map(str::to_owned).to_vec();
    let columns: Vec<String> = (0..24).map(|hour| format!("{hour:02}")).collect();
    Heat {
        label: format!(
            "Requests by weekday and hour, {}",
            range.label().to_lowercase()
        ),
        rows: &rows,
        columns: &columns,
        values: &values,
        format: Scale::Count,
    }
    .render()
}

fn by_group_rows(points: &[Point], by: By) -> Vec<GroupRow> {
    let groups = stats::by_group(points, by);
    let max = groups
        .iter()
        .map(|(_, totals)| totals.tokens())
        .max()
        .unwrap_or(0);
    groups
        .into_iter()
        .map(|(name, mut totals)| GroupRow {
            href: activity_filter(by, &name),
            requests: format::count(totals.requests),
            input: format::compact(totals.input()),
            cached: totals.cache_ratio().map_or_else(
                || "—".to_owned(),
                |ratio| format!("{}%", format::percent(ratio)),
            ),
            output: format::compact(totals.output),
            limits: format::count(totals.limits),
            errors: format::count(totals.errors),
            p50: totals
                .percentile(0.5)
                .map_or_else(|| "—".to_owned(), |ms| duration(float(ms))),
            p95: totals
                .percentile(0.95)
                .map_or_else(|| "—".to_owned(), |ms| duration(float(ms))),
            share: share(totals.tokens(), max, &name),
            name,
        })
        .collect()
}

fn activity_filter(by: By, name: &str) -> Option<String> {
    let field = match by {
        By::Key => "key",
        By::Model => "model",
        By::Account => "account",
        By::Outcome => {
            return Some(format!("/admin/activity?outcome={name}"));
        }
    };
    (!name.starts_with('(')).then(|| {
        format!(
            "/admin/activity?q={}",
            url::form_urlencoded::byte_serialize(format!("{field}:{name}").as_bytes())
                .collect::<String>()
        )
    })
}

fn share(value: i64, max: i64, name: &str) -> Svg {
    let width = if max > 0 {
        float(value) / float(max) * 100.0
    } else {
        0.0
    };
    super::chart::share(width, &format!("{name}: {} tokens", format::compact(value)))
}
