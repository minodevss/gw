use std::collections::HashMap;

use askama::Template;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Response;

use super::chart::{
    Band, HALF, Key, KeyMark, Mark, Row, RuleKind, Scale, Series, Shape, Slot, Svg, Table,
    TimeChart, WIDE, Zone, float,
};
use super::frame::{Ctx, Frame, Tab};
use super::view::{self, Stamp, span_of, span_secs, window_order};
use super::{Web, failure, fragment, html, segment};
use crate::account::Account;
use crate::catalog::Catalog;
use crate::display::{Health, health};
use crate::display::{PROVIDERS, provider_title, window_label};
use crate::format;
use crate::ops;
use crate::provider::Provider;
use crate::store::{Point, Sample};

const DAY: i64 = 86_400;
const COLUMNS: i64 = 96;
const LEDGER_CYCLES: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Span {
    TwoDays,
    Week,
    TwoWeeks,
    Month,
}

const SPANS: [Span; 4] = [Span::TwoDays, Span::Week, Span::TwoWeeks, Span::Month];

impl Span {
    fn query(self) -> &'static str {
        match self {
            Self::TwoDays => "2d",
            Self::Week => "7d",
            Self::TwoWeeks => "14d",
            Self::Month => "30d",
        }
    }

    fn secs(self) -> i64 {
        match self {
            Self::TwoDays => 2 * DAY,
            Self::Week => 7 * DAY,
            Self::TwoWeeks => 14 * DAY,
            Self::Month => 30 * DAY,
        }
    }
}

struct Choice {
    query: String,
    label: String,
    current: bool,
}

#[derive(Template)]
#[template(path = "admin/usage.html")]
struct Page {
    frame: Frame,
    live: String,
}

#[derive(Template)]
#[template(path = "admin/usage_live.html")]
struct Live {
    query: String,
    providers: Vec<Choice>,
    windows: Vec<Choice>,
    ranges: Vec<Choice>,
    provider: &'static str,
    window: String,
    pooled: Option<(Svg, String)>,
    panels: Vec<Panel>,
    ledger: Vec<LedgerRow>,
    ledger_heads: Vec<String>,
    since: String,
}

struct Panel {
    name: String,
    href: String,
    state: view::StateView,
    says: Vec<String>,
    chart: Svg,
}

struct LedgerRow {
    name: String,
    cells: Vec<String>,
    average: String,
}

struct Data {
    accounts: Vec<Account>,
    catalog: Catalog,
    history: Vec<Sample>,
    limits: Vec<Point>,
    first: Option<i64>,
}

async fn load(web: &Web, since: i64) -> anyhow::Result<Data> {
    let (accounts, catalog, history, limits, first) = ops::with_store(&web.0.home, move |store| {
        Ok((
            store.accounts()?,
            store.catalog()?,
            store.usage_history(since)?,
            store.limit_points(since)?,
            store.usage_history_start()?,
        ))
    })
    .await?;
    Ok(Data {
        accounts,
        catalog,
        history,
        limits,
        first,
    })
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
                frame: ctx.frame(Some(Tab::Usage), "Usage"),
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

fn reported_labels(accounts: &[Account], history: &[Sample], provider: Provider) -> Vec<String> {
    let mut labels: Vec<String> = accounts
        .iter()
        .filter(|account| account.kind.provider() == provider)
        .flat_map(|account| {
            account
                .usage
                .iter()
                .flat_map(|usage| usage.windows.iter().map(|window| window.label.clone()))
                .chain(
                    history
                        .iter()
                        .filter(|sample| sample.account == account.name)
                        .map(|sample| sample.label.clone()),
                )
                .collect::<Vec<_>>()
        })
        .collect();
    labels.sort_by(|a, b| window_order(a, b));
    labels.dedup();
    labels
}

async fn build(web: &Web, ctx: &Ctx, query: &HashMap<String, String>) -> anyhow::Result<String> {
    let longest = SPANS.into_iter().map(Span::secs).max().unwrap_or(30 * DAY);
    let data = load(web, ctx.now - longest.max(LEDGER_CYCLES_SECS)).await?;
    let with_windows: Vec<Provider> = PROVIDERS
        .into_iter()
        .filter(|provider| !reported_labels(&data.accounts, &data.history, *provider).is_empty())
        .collect();
    let provider = query
        .get("provider")
        .and_then(|name| name.parse::<Provider>().ok())
        .filter(|provider| with_windows.contains(provider))
        .or_else(|| with_windows.first().copied())
        .unwrap_or(Provider::Anthropic);
    let labels = reported_labels(&data.accounts, &data.history, provider);
    let mut spans: Vec<String> = labels
        .iter()
        .map(|label| span_of(label).to_owned())
        .collect();
    spans.dedup();
    let window = query
        .get("window")
        .filter(|window| spans.contains(window))
        .cloned()
        .or_else(|| spans.iter().find(|span| *span == "7d").cloned())
        .or_else(|| spans.first().cloned())
        .unwrap_or_else(|| "7d".to_owned());
    let window_secs = span_secs(&window).unwrap_or(7 * DAY);
    let default_range = if window_secs <= DAY {
        Span::TwoDays
    } else {
        Span::TwoWeeks
    };
    let range = query
        .get("range")
        .and_then(|text| SPANS.into_iter().find(|span| span.query() == text))
        .unwrap_or(default_range);
    let href = |provider: Provider, window: &str, range: Span| {
        format!(
            "provider={provider}&window={}&range={}",
            segment(window),
            range.query()
        )
    };
    let from = ctx.now - range.secs();
    let in_span: Vec<&String> = labels
        .iter()
        .filter(|label| span_of(label) == window)
        .collect();
    let accounts = view::sorted(&data.accounts, provider, ctx.now);
    let accounts: Vec<&Account> = accounts
        .into_iter()
        .filter(|account| {
            account
                .usage
                .iter()
                .flat_map(|usage| &usage.windows)
                .any(|w| span_of(&w.label) == window)
                || data
                    .history
                    .iter()
                    .any(|s| s.account == account.name && span_of(&s.label) == window)
        })
        .collect();
    let panels = accounts
        .iter()
        .map(|account| panel(account, &in_span, &data, from, HALF, ctx))
        .collect();
    let pooled = pooled(provider, &accounts, &window, &data, from, ctx);
    let (ledger_heads, ledger) = ledger(&accounts, &window, &data, ctx);
    let live = Live {
        query: href(provider, &window, range),
        providers: with_windows
            .iter()
            .map(|p| Choice {
                query: href(*p, &window, range),
                label: provider_title(*p).to_owned(),
                current: *p == provider,
            })
            .collect(),
        windows: spans
            .iter()
            .map(|span| Choice {
                query: href(provider, span, range),
                label: window_label(span),
                current: *span == window,
            })
            .collect(),
        ranges: SPANS
            .into_iter()
            .map(|span| Choice {
                query: href(provider, &window, span),
                label: format!("Last {}", span.query()),
                current: span == range,
            })
            .collect(),
        provider: provider_title(provider),
        window: window_label(&window),
        pooled,
        panels,
        ledger,
        ledger_heads,
        since: data
            .first
            .map_or_else(String::new, |at| Stamp::new(at, ctx.now, &ctx.zone).local),
    };
    Ok(live.render()?)
}

pub(super) struct AccountChart {
    pub(super) title: String,
    pub(super) href: String,
    pub(super) says: Vec<String>,
    pub(super) chart: Svg,
}

pub(super) async fn account_charts(
    web: &Web,
    ctx: &Ctx,
    account: &Account,
) -> anyhow::Result<Vec<AccountChart>> {
    let data = load(web, ctx.now - 14 * DAY).await?;
    let labels = reported_labels(
        std::slice::from_ref(account),
        &data.history,
        account.kind.provider(),
    );
    let mut spans: Vec<String> = labels
        .iter()
        .map(|label| span_of(label).to_owned())
        .collect();
    spans.dedup();
    Ok(spans
        .into_iter()
        .map(|span| {
            let range = if span_secs(&span).unwrap_or(DAY) <= DAY {
                Span::TwoDays
            } else {
                Span::TwoWeeks
            };
            let in_span: Vec<&String> = labels
                .iter()
                .filter(|label| span_of(label) == span)
                .collect();
            let panel = panel(account, &in_span, &data, ctx.now - range.secs(), WIDE, ctx);
            AccountChart {
                title: format!("{} windows, last {}", window_label(&span), range.query()),
                href: format!(
                    "/admin/usage?provider={}&window={}&range={}",
                    account.kind.provider(),
                    segment(&span),
                    range.query()
                ),
                says: panel.says,
                chart: panel.chart,
            }
        })
        .collect())
}

const LEDGER_CYCLES_SECS: i64 = 6 * 7 * DAY;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Level {
    Known(f64),
    Reset,
    None,
}

fn samples<'a>(data: &'a Data, account: &str, label: &str) -> Vec<&'a Sample> {
    data.history
        .iter()
        .filter(|sample| sample.account == account && sample.label == label)
        .collect()
}

fn level_at(samples: &[&Sample], at: i64) -> Level {
    match samples.iter().rev().find(|sample| sample.at <= at) {
        Some(sample) if sample.reset_at.is_some_and(|reset| reset <= at) => Level::Reset,
        Some(sample) => Level::Known(sample.used),
        None => Level::None,
    }
}

fn step_points(samples: &[&Sample], from: i64, now: i64) -> (Vec<(i64, Option<f64>)>, i64) {
    let mut points = Vec::new();
    for (index, sample) in samples.iter().enumerate() {
        let start = sample.at.max(from);
        let next = samples.get(index + 1).map_or(now, |next| next.at);
        if next < from || sample.reset_at.is_some_and(|reset| reset <= start) {
            continue;
        }
        points.push((start, Some(sample.used)));
        if let Some(reset) = sample.reset_at
            && reset < next
            && reset >= start
        {
            points.push((reset, None));
        }
    }
    let until = samples
        .last()
        .and_then(|last| last.reset_at)
        .map_or(now, |reset| reset.min(now));
    (points, until)
}

fn seen(account: &Account, label: &str, samples: &[&Sample]) -> Option<i64> {
    let last = samples.last()?;
    let current = account
        .usage
        .iter()
        .flat_map(|usage| &usage.windows)
        .find(|window| window.label == label);
    match (current, account.usage_at) {
        (Some(window), Some(at))
            if (window.used - last.used).abs() < 1e-9 && window.reset_at == last.reset_at =>
        {
            Some(at.max(last.at))
        }
        _ => Some(last.at),
    }
}

struct Outlook {
    says: String,
    line: Option<(i64, f64, i64, f64)>,
}

fn outlook(samples: &[&Sample], seen: i64, label: &str, now: i64, zone: &Zone) -> Option<Outlook> {
    let last = samples.last()?;
    let reset = last.reset_at.filter(|reset| *reset > now)?;
    let span = span_secs(label)?;
    let start = reset - span;
    let name = window_label(label);
    let used = last.used;
    let until = Stamp::until(reset, now, zone);
    let read = Stamp::new(seen, now, zone);
    let age = now - seen;
    if age > span / 4 {
        return Some(Outlook {
            says: format!(
                "{name}: {}% used when last read {} ({}). Too old to project from.",
                format::percent(used),
                read.local,
                read.ago
            ),
            line: None,
        });
    }
    let as_of = if age > view::STALE_SECS {
        format!(" as of the reading {}", read.ago)
    } else {
        String::new()
    };
    if used >= 1.0 {
        return Some(Outlook {
            says: format!(
                "{name}: used up{as_of}. It resets {} ({}).",
                until.local, until.ago
            ),
            line: None,
        });
    }
    let elapsed = seen - start;
    if used <= 0.0 {
        return Some(Outlook {
            says: format!(
                "{name}: not used yet in this window{as_of}. It resets {} ({}).",
                until.local, until.ago
            ),
            line: None,
        });
    }
    if elapsed < span / 10 {
        return Some(Outlook {
            says: format!(
                "{name}: {}% used{as_of}, resets {} ({}). Too early in the window to project.",
                format::percent(used),
                until.local,
                until.ago
            ),
            line: None,
        });
    }
    let rate = used / float(elapsed);
    let at_reset = used + rate * float(reset - seen);
    let started = zone.strftime(start, "%a %b %-d %H:%M");
    if at_reset >= 1.0 {
        let out = seen + whole_secs((1.0 - used) / rate);
        let runs_out = Stamp::until(out, now, zone);
        let when = if out <= now {
            "may already be used up".to_owned()
        } else {
            format!(
                "runs out {} ({}), {} before its reset",
                runs_out.local,
                runs_out.ago,
                format::remaining(reset - out)
            )
        };
        Some(Outlook {
            says: format!(
                "{name}: {}% used{as_of}. At the average pace since {started} it {when}.",
                format::percent(used),
            ),
            line: Some((seen, used, out, 1.0)),
        })
    } else {
        Some(Outlook {
            says: format!(
                "{name}: {}% used{as_of}. At the average pace since {started} it reaches about {}% by its reset {} ({}).",
                format::percent(used),
                format::percent(at_reset),
                until.local,
                until.ago
            ),
            line: Some((seen, used, reset, at_reset)),
        })
    }
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "projected seconds within one window fit in i64"
)]
fn whole_secs(value: f64) -> i64 {
    value.round() as i64
}

fn columns_table(
    chart: &mut TimeChart,
    head: Vec<String>,
    cells: impl Fn(i64) -> Vec<String>,
    zone: &Zone,
) {
    let span = chart.to - chart.from;
    let pattern = if span <= 2 * DAY {
        "%a %H:%M"
    } else {
        "%b %-d %H:%M"
    };
    let rows: Vec<Row> = TimeChart::columns(chart.from, chart.to, COLUMNS)
        .into_iter()
        .map(|(start, end)| {
            let at = i64::midpoint(start, end);
            let row_cells = std::iter::once(zone.strftime(at, pattern))
                .chain(cells(at))
                .collect();
            chart.row(start, end, row_cells)
        })
        .collect();
    chart.table = Table { head, rows };
}

fn panel(
    account: &Account,
    labels: &[&String],
    data: &Data,
    from: i64,
    width: f64,
    ctx: &Ctx,
) -> Panel {
    let now = ctx.now;
    let mine: Vec<(String, Vec<&Sample>)> = labels
        .iter()
        .map(|label| ((*label).clone(), samples(data, &account.name, label)))
        .collect();
    let primary = mine.first();
    let signed_out = matches!(health(account, now), Health::SignedOut(_));
    let mut series = Vec::new();
    let mut keys = Vec::new();
    for (index, (label, samples)) in mine.iter().enumerate() {
        let (points, until) = step_points(samples, from, now);
        let until = match seen(account, label, samples) {
            Some(seen) if signed_out => until.min(seen),
            Some(_) | None => until,
        };
        series.push(Series {
            slot: Slot::nth(index),
            shape: Shape::Step,
            points,
            until,
        });
        keys.push(Key {
            name: window_label(label),
            slot: Slot::nth(index),
            mark: KeyMark::Line,
        });
    }
    let mut says = Vec::new();
    for (index, (label, samples)) in mine.iter().enumerate() {
        let Some(seen) = seen(account, label, samples) else {
            continue;
        };
        if let Some(outlook) = outlook(samples, seen, label, now, &ctx.zone) {
            if let Some((x0, y0, x1, y1)) = outlook.line {
                series.push(Series {
                    slot: Slot::nth(index),
                    shape: Shape::Projection,
                    points: vec![(x0, Some(y0)), (x1, Some(y1))],
                    until: x1,
                });
            }
            if index == 0 {
                says.push(outlook.says);
            }
        }
    }
    let current = |label: &str, samples: &[&Sample]| {
        let fresh = seen(account, label, samples)
            .is_some_and(|seen| now - seen <= span_secs(label).unwrap_or(DAY) / 4);
        samples
            .last()
            .filter(|last| fresh && last.reset_at.is_none_or(|reset| reset > now))
            .map(|last| last.used)
    };
    let primary_used = mine
        .first()
        .and_then(|(label, samples)| current(label, samples));
    let highest = mine
        .iter()
        .skip(1)
        .filter_map(|(label, samples)| current(label, samples).map(|used| (label, used)))
        .max_by(|a, b| a.1.total_cmp(&b.1));
    if let (Some((label, used)), Some(primary)) = (highest, primary_used)
        && used > primary + 0.005
    {
        says.push(format!(
            "{} is higher, at {}%.",
            window_label(label),
            format::percent(used)
        ));
    }
    if series.iter().any(|s| s.shape == Shape::Projection) {
        keys.push(Key {
            name: "projection".to_owned(),
            slot: Slot::Muted,
            mark: KeyMark::Dashed,
        });
    }
    let mut rules: Vec<(RuleKind, i64)> = primary.map_or_default(|(_, samples)| {
        let mut resets: Vec<i64> = samples
            .iter()
            .filter_map(|sample| sample.reset_at)
            .filter(|reset| *reset > from)
            .collect();
        resets.sort_unstable();
        resets.dedup();
        resets.into_iter().map(|at| (RuleKind::Reset, at)).collect()
    });
    rules.push((RuleKind::Now, now));
    let to = now + (now - from) / 4;
    rules.retain(|(_, at)| *at < to);
    for series in series.iter_mut().filter(|s| s.shape == Shape::Projection) {
        if let [(x0, Some(y0)), (x1, Some(y1))] = series.points[..]
            && x1 > to
            && x1 > x0
        {
            let y = y0 + (y1 - y0) * float(to - x0) / float(x1 - x0);
            series.points = vec![(x0, Some(y0)), (to, Some(y))];
            series.until = to;
        }
    }
    let marks = data
        .limits
        .iter()
        .filter(|point| point.account.as_deref() == Some(account.name.as_str()) && point.ts >= from)
        .map(|point| Mark {
            at: point.ts,
            label: format!(
                "The provider reported a limit {} for {}. It does not say which window.",
                ctx.zone.clock(now).when(point.ts),
                point.model
            ),
        })
        .collect::<Vec<_>>();
    if !marks.is_empty() {
        keys.push(Key {
            name: "limit reported, any window".to_owned(),
            slot: Slot::Bad,
            mark: KeyMark::Block,
        });
    }
    let mut chart = TimeChart {
        label: format!(
            "{}: {} usage over time, 0 to 100 percent",
            account.name,
            labels
                .iter()
                .map(|label| window_label(label))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        from,
        to,
        scale: Scale::Percent,
        max: 1.0,
        width,
        height: if width > HALF { 200.0 } else { 190.0 },
        bands: Vec::new(),
        series,
        rules,
        ceiling: None,
        marks,
        shades: Vec::new(),
        keys,
        table: Table {
            head: Vec::new(),
            rows: Vec::new(),
        },
    };
    let head = std::iter::once("Time".to_owned())
        .chain(mine.iter().map(|(label, _)| window_label(label)))
        .collect();
    columns_table(
        &mut chart,
        head,
        |at| {
            mine.iter()
                .map(|(_, samples)| match level_at(samples, at) {
                    _ if at > now => "—".to_owned(),
                    Level::Known(used) => format!("{}%", format::percent(used)),
                    Level::Reset => "reset, not read".to_owned(),
                    Level::None => "—".to_owned(),
                })
                .collect()
        },
        &ctx.zone,
    );
    Panel {
        name: account.name.clone(),
        href: format!("/admin/account/{}", segment(&account.name)),
        state: view::state(
            account,
            view::serves(&data.catalog, account),
            now,
            &ctx.zone,
        ),
        says,
        chart: chart.render(&ctx.zone),
    }
}

fn pooled(
    provider: Provider,
    accounts: &[&Account],
    window: &str,
    data: &Data,
    from: i64,
    ctx: &Ctx,
) -> Option<(Svg, String)> {
    let now = ctx.now;
    let series: Vec<(&Account, Vec<&Sample>)> = accounts
        .iter()
        .map(|account| (*account, samples(data, &account.name, window)))
        .filter(|(_, samples)| !samples.is_empty())
        .collect();
    let ends: Vec<i64> = series
        .iter()
        .map(|(account, samples)| {
            if matches!(health(account, now), Health::SignedOut(_)) {
                seen(account, window, samples).unwrap_or(now)
            } else {
                i64::MAX
            }
        })
        .collect();
    let level = |index: usize, samples: &[&Sample], at: i64| {
        if ends.get(index).is_some_and(|end| at >= *end) {
            Level::None
        } else {
            level_at(samples, at)
        }
    };
    if series.len() < 2 {
        return None;
    }
    let mut cuts: Vec<i64> = vec![from, now];
    cuts.extend(
        ends.iter()
            .filter(|end| **end < now)
            .map(|end| (*end).max(from)),
    );
    for (_, samples) in &series {
        for sample in samples {
            cuts.push(sample.at.clamp(from, now));
            if let Some(reset) = sample.reset_at {
                cuts.push(reset.clamp(from, now));
            }
        }
    }
    cuts.sort_unstable();
    cuts.dedup();
    let mut known: Vec<Band> = Vec::new();
    let mut assumed: Vec<Band> = Vec::new();
    for (index, _) in series.iter().enumerate() {
        known.push(Band {
            slot: Slot::ramp(index),
            hatched: false,
            steps: Vec::new(),
        });
        assumed.push(Band {
            slot: Slot::ramp(index),
            hatched: true,
            steps: Vec::new(),
        });
    }
    for pair in cuts.windows(2) {
        let (Some(start), Some(end)) = (pair.first().copied(), pair.get(1).copied()) else {
            continue;
        };
        let mut base = 0.0;
        for (index, (_, samples)) in series.iter().enumerate() {
            let (value, hatched) = match level(index, samples, start) {
                Level::Known(used) => ((1.0 - used).clamp(0.0, 1.0), false),
                Level::Reset => (1.0, true),
                Level::None => (0.0, false),
            };
            let band = if hatched {
                assumed.get_mut(index)
            } else {
                known.get_mut(index)
            };
            if let Some(band) = band {
                band.steps.push((start, end, base, base + value));
            }
            base += value;
        }
    }
    let count = series.len();
    let mut keys: Vec<Key> = series
        .iter()
        .enumerate()
        .map(|(index, (account, _))| Key {
            name: account.name.clone(),
            slot: Slot::ramp(index),
            mark: KeyMark::Block,
        })
        .collect();
    if assumed.iter().any(|band| !band.steps.is_empty()) {
        keys.push(Key {
            name: "reset passed, not read since (counted as full)".to_owned(),
            slot: Slot::Muted,
            mark: KeyMark::Hatch,
        });
    }
    let total = |at: i64| {
        let levels: Vec<Level> = series
            .iter()
            .enumerate()
            .map(|(index, (_, samples))| level(index, samples, at))
            .collect();
        if levels.iter().all(|level| *level == Level::None) {
            return "—".to_owned();
        }
        let sum: f64 = levels
            .iter()
            .map(|level| match level {
                Level::Known(used) => (1.0 - used).clamp(0.0, 1.0),
                Level::Reset => 1.0,
                Level::None => 0.0,
            })
            .sum();
        format_accounts(sum)
    };
    let says = format!(
        "Now: {}.",
        view::pool_text(&data.accounts, provider, window, now, &ctx.zone)
    );
    let first = series
        .iter()
        .filter_map(|(_, samples)| samples.first().map(|sample| sample.at))
        .min()
        .unwrap_or(from);
    let shades = if first > from {
        keys.push(Key {
            name: "before the first reading".to_owned(),
            slot: Slot::Muted,
            mark: KeyMark::Hatch,
        });
        vec![(from, first.min(now))]
    } else {
        Vec::new()
    };
    let mut bands = known;
    bands.extend(assumed);
    let mut chart = TimeChart {
        label: format!("Capacity left across accounts, {}", window_label(window)),
        from,
        to: now,
        scale: Scale::Accounts,
        max: float(i64::try_from(count).unwrap_or(0)),
        width: WIDE,
        height: 220.0,
        bands,
        series: Vec::new(),
        rules: vec![(RuleKind::Now, now)],
        ceiling: Some(float(i64::try_from(count).unwrap_or(0))),
        marks: Vec::new(),
        shades,
        keys,
        table: Table {
            head: Vec::new(),
            rows: Vec::new(),
        },
    };
    let head = std::iter::once("Time".to_owned())
        .chain(std::iter::once("All accounts".to_owned()))
        .chain(series.iter().map(|(account, _)| account.name.clone()))
        .collect();
    columns_table(
        &mut chart,
        head,
        |at| {
            std::iter::once(total(at))
                .chain(series.iter().enumerate().map(|(index, (_, samples))| {
                    match level(index, samples, at) {
                        Level::Known(used) => {
                            format!("{}%", format::percent((1.0 - used).clamp(0.0, 1.0)))
                        }
                        Level::Reset => "reset, not read".to_owned(),
                        Level::None => "—".to_owned(),
                    }
                }))
                .collect()
        },
        &ctx.zone,
    );
    Some((chart.render(&ctx.zone), says))
}

fn format_accounts(value: f64) -> String {
    format!("{value:.1}")
}

fn ledger(
    accounts: &[&Account],
    window: &str,
    data: &Data,
    ctx: &Ctx,
) -> (Vec<String>, Vec<LedgerRow>) {
    let mut rows = Vec::new();
    let mut widest = 0;
    for account in accounts {
        let samples = samples(data, &account.name, window);
        let mut ended: Vec<(i64, f64)> = Vec::new();
        for pair in samples.windows(2) {
            let (Some(before), Some(after)) = (pair.first(), pair.get(1)) else {
                continue;
            };
            if let Some(reset) = before.reset_at
                && reset <= after.at
                && before.reset_at != after.reset_at
            {
                ended.push((reset, (1.0 - before.used).clamp(0.0, 1.0)));
            }
        }
        if let Some(last) = samples.last()
            && let Some(reset) = last.reset_at
            && reset <= ctx.now
        {
            ended.push((reset, (1.0 - last.used).clamp(0.0, 1.0)));
        }
        let recent: Vec<(i64, f64)> = ended
            .iter()
            .rev()
            .take(LEDGER_CYCLES)
            .rev()
            .copied()
            .collect();
        if recent.is_empty() {
            continue;
        }
        widest = widest.max(recent.len());
        let average = recent.iter().map(|(_, left)| left).sum::<f64>()
            / float(i64::try_from(recent.len()).unwrap_or(1));
        rows.push(LedgerRow {
            name: account.name.clone(),
            cells: recent
                .iter()
                .map(|(at, left)| {
                    format!(
                        "{}% · {}",
                        format::percent(*left),
                        ctx.zone.strftime(*at, "%b %-d")
                    )
                })
                .collect(),
            average: format!("{}%", format::percent(average)),
        });
    }
    let heads = (1..=widest)
        .rev()
        .map(|back| match back {
            1 => "Last reset".to_owned(),
            n => format!("{n} resets ago"),
        })
        .collect();
    for row in &mut rows {
        while row.cells.len() < widest {
            row.cells.insert(0, String::new());
        }
    }
    (heads, rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(at: i64, used: f64, reset_at: i64) -> Sample {
        Sample {
            account: "a".to_owned(),
            label: "7d".to_owned(),
            at,
            used,
            reset_at: Some(reset_at),
        }
    }

    #[test]
    fn levels_become_unknown_after_a_reset_until_the_next_reading() {
        let samples = [sample(10, 0.4, 50), sample(80, 0.1, 200)];
        let refs: Vec<&Sample> = samples.iter().collect();
        assert_eq!(level_at(&refs, 5), Level::None);
        assert_eq!(level_at(&refs, 20), Level::Known(0.4));
        assert_eq!(level_at(&refs, 60), Level::Reset);
        assert_eq!(level_at(&refs, 90), Level::Known(0.1));
        let (points, until) = step_points(&refs, 0, 100);
        assert_eq!(points, [(10, Some(0.4)), (50, None), (80, Some(0.1))]);
        assert_eq!(until, 100);
        let (points, _) = step_points(&refs, 60, 100);
        assert_eq!(
            points,
            [(80, Some(0.1))],
            "a reading whose window reset before the range is unknown"
        );
    }

    #[test]
    fn outlook_projects_the_average_pace_to_the_reset() {
        let week = 7 * DAY;
        let start = 1_000_000;
        let now = start + week / 2;
        let samples = [sample(now - 60, 0.6, start + week)];
        let refs: Vec<&Sample> = samples.iter().collect();
        let zone = Zone::new(0, None);
        let outlook = outlook(&refs, now - 60, "7d", now, &zone).unwrap();
        assert!(outlook.says.contains("runs out"), "{}", outlook.says);
        let (_, _, out, full) = outlook.line.unwrap();
        assert!((out - (now - 60 + (week / 2 - 60) * 2 / 3)).abs() <= 1);
        assert!((full - 1.0).abs() < 1e-9);

        let calm = [sample(now - 60, 0.2, start + week)];
        let refs: Vec<&Sample> = calm.iter().collect();
        let outlook = super::outlook(&refs, now - 60, "7d", now, &zone).unwrap();
        assert!(outlook.says.contains("about 40%"), "{}", outlook.says);

        let stale = [sample(now - 2 * DAY, 0.3, start + week)];
        let refs: Vec<&Sample> = stale.iter().collect();
        let outlook = super::outlook(&refs, now - 2 * DAY, "7d", now, &zone).unwrap();
        assert!(outlook.line.is_none());
        assert!(
            outlook.says.contains("Too old to project"),
            "{}",
            outlook.says
        );
        let outlook = super::outlook(&refs, now - 60, "7d", now, &zone).unwrap();
        let (seen, _, _, _) = outlook.line.unwrap();
        assert_eq!(
            seen,
            now - 60,
            "an unchanged recent reading counts as fresh"
        );

        let full = [sample(now - 60, 1.0, start + week)];
        let refs: Vec<&Sample> = full.iter().collect();
        let outlook = super::outlook(&refs, now - 60, "7d", now, &zone).unwrap();
        assert!(outlook.says.contains("used up"), "{}", outlook.says);

        let early = [sample(start + 60, 0.05, start + week)];
        let refs: Vec<&Sample> = early.iter().collect();
        let outlook = super::outlook(&refs, start + 60, "7d", start + 120, &zone).unwrap();
        assert!(outlook.line.is_none());
    }
}
