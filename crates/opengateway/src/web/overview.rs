use askama::Template;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Response;

use super::chart::{Bar, BarChart, Key, KeyMark, Scale, Slot, Svg, duration};
use super::frame::{Ctx, Frame, Tab};
use super::stats::{self, Buckets, By, Range};
use super::view::{self, AccountRow, Pool};
use super::{Web, failure, fragment, html, segment};
use crate::account::Account;
use crate::catalog::Catalog;
use crate::display::{Health, PROVIDERS, health, provider_title};
use crate::format;
use crate::ops::{self, model};
use crate::store::Point;

const HOUR: i64 = 3600;
const DAY: i64 = 86_400;
const ROUTES: usize = 8;

#[derive(Template)]
#[template(path = "admin/overview.html")]
struct Page {
    frame: Frame,
    live: String,
}

#[derive(Template)]
#[template(path = "admin/overview_live.html")]
struct Live {
    verdict: String,
    verdict_tone: &'static str,
    hour: String,
    attention: Vec<Attention>,
    onboarding: Onboarding,
    routes: Vec<Route>,
    more_routes: usize,
    groups: Vec<Group>,
    day: Svg,
    day_facts: Vec<(String, String)>,
    updated: String,
}

struct Onboarding {
    accounts: bool,
    keys: bool,
}

struct Attention {
    tone: &'static str,
    what: String,
    why: String,
    link: Option<(&'static str, String)>,
}

struct Route {
    model: String,
    requests: String,
    next: String,
    blocked: bool,
}

struct Group {
    title: &'static str,
    pools: Vec<Pool>,
    rows: Vec<AccountRow>,
}

struct Data {
    accounts: Vec<Account>,
    catalog: Catalog,
    keys: usize,
    points: Vec<Point>,
}

async fn load(web: &Web, now: i64) -> anyhow::Result<Data> {
    let (accounts, catalog, keys, points) = ops::with_store(&web.0.home, move |store| {
        Ok((
            store.accounts()?,
            store.catalog()?,
            store.keys()?.len(),
            store.log_points(now - DAY - HOUR)?,
        ))
    })
    .await?;
    Ok(Data {
        accounts,
        catalog,
        keys,
        points,
    })
}

pub(super) async fn page(State(web): State<Web>, ctx: Ctx) -> Response {
    match load(&web, ctx.now)
        .await
        .and_then(|data| render(&data, &ctx))
    {
        Ok(live) => html(
            StatusCode::OK,
            &Page {
                frame: ctx.frame(Some(Tab::Overview), "Overview"),
                live,
            },
        ),
        Err(error) => failure(&error),
    }
}

pub(super) async fn live(State(web): State<Web>, ctx: Ctx) -> Response {
    fragment(
        load(&web, ctx.now)
            .await
            .and_then(|data| render(&data, &ctx)),
    )
}

fn render(data: &Data, ctx: &Ctx) -> anyhow::Result<String> {
    let now = ctx.now;
    let zone = &ctx.zone;
    let routes = routes(data, ctx);
    let attention = attention(data, &routes, ctx);
    let usable = data
        .accounts
        .iter()
        .filter(|account| {
            matches!(health(account, now), Health::Ready)
                && data
                    .catalog
                    .models()
                    .any(|(name, _, _)| name == account.name)
        })
        .count();
    let total = data.accounts.len();
    let failed = data
        .points
        .iter()
        .filter(|point| point.ts >= now - HOUR && point.failed && !point.limit)
        .count();
    let (verdict, verdict_tone) = match (total, total.saturating_sub(usable)) {
        (0, _) => ("No accounts yet".to_owned(), "idle"),
        (all, 0) => (
            format!(
                "{} ready",
                if all == 1 {
                    "The account is".to_owned()
                } else {
                    format!("All {all} accounts are")
                }
            ),
            "ok",
        ),
        (_, 1) => ("1 account needs attention".to_owned(), "warn"),
        (_, many) => (format!("{many} accounts need attention"), "warn"),
    };
    let (verdict, verdict_tone) = match (failed, verdict_tone) {
        (0, _) => (verdict, verdict_tone),
        (count, tone) => (
            format!(
                "{verdict}, and {} failed in the last hour",
                format::noun(
                    i64::try_from(count).unwrap_or(i64::MAX),
                    "request",
                    "requests"
                )
            ),
            if tone == "ok" { "warn" } else { tone },
        ),
    };
    let last_hour: Vec<Point> = data
        .points
        .iter()
        .filter(|point| point.ts >= now - HOUR)
        .cloned()
        .collect();
    let hour = stats::total(&last_hour);
    let hour = format!(
        "Last hour: {}, {}, {}",
        format::noun(hour.requests, "request", "requests"),
        format::noun(hour.limits, "limit", "limits"),
        format::noun(hour.errors, "error", "errors")
    );
    let since = Buckets::new(Range::Day, now, zone).since();
    let day_points: Vec<Point> = data
        .points
        .iter()
        .filter(|point| point.ts >= since)
        .cloned()
        .collect();
    let mut day = stats::total(&day_points);
    let mut day_facts = vec![
        ("requests".to_owned(), format::count(day.requests)),
        ("tokens in".to_owned(), format::compact(day.input())),
        ("tokens out".to_owned(), format::compact(day.output)),
    ];
    if let Some(ratio) = day.cache_ratio() {
        day_facts.push((
            "of input cached".to_owned(),
            format!("{}%", format::percent(ratio)),
        ));
    }
    if let Some(p50) = day.percentile(0.5) {
        day_facts.push((
            "median request".to_owned(),
            duration(super::chart::float(p50)),
        ));
    }
    let live = Live {
        verdict,
        verdict_tone,
        hour,
        onboarding: Onboarding {
            accounts: data.accounts.is_empty(),
            keys: data.keys == 0,
        },
        attention,
        more_routes: routes.len().saturating_sub(ROUTES),
        routes: routes.into_iter().take(ROUTES).collect(),
        groups: PROVIDERS
            .into_iter()
            .filter_map(|provider| {
                let rows: Vec<AccountRow> = view::sorted(&data.accounts, provider, now)
                    .into_iter()
                    .map(|account| {
                        view::account_row(account, view::serves(&data.catalog, account), now, zone)
                    })
                    .collect();
                (!rows.is_empty()).then(|| Group {
                    title: provider_title(provider),
                    pools: view::pools(&data.accounts, provider, now, zone)
                        .into_iter()
                        .filter(|pool| pool.count > 1)
                        .collect(),
                    rows,
                })
            })
            .collect(),
        day: day_chart(&day_points, ctx),
        day_facts,
        updated: zone.strftime(now, "%H:%M:%S"),
    };
    Ok(live.render()?)
}

fn attention(data: &Data, routes: &[Route], ctx: &Ctx) -> Vec<Attention> {
    let now = ctx.now;
    let mut items = Vec::new();
    for account in &data.accounts {
        let href = format!("/admin/account/{}", segment(&account.name));
        match health(account, now) {
            Health::SignedOut(_) => {
                let state = view::state(account, true, now, &ctx.zone);
                items.push(Attention {
                    tone: "bad",
                    what: format!("{} cannot take requests", account.name),
                    why: state.text,
                    link: state.fix.map(|fix| (fix.label, fix.action)),
                });
            }
            Health::Limited { .. } => {
                let state = view::state(account, true, now, &ctx.zone);
                items.push(Attention {
                    tone: "warn",
                    what: account.name.clone(),
                    why: state.text,
                    link: Some(("Open", href.clone())),
                });
            }
            Health::Ready => {}
        }
        let serves = data
            .catalog
            .models()
            .any(|(name, _, _)| name == account.name);
        if !serves {
            items.push(Attention {
                tone: "warn",
                what: format!("{} serves no models", account.name),
                why: "OpenGateway sends it nothing until its model list is fetched.".to_owned(),
                link: Some(("Open", href)),
            });
        }
    }
    let failed = data
        .points
        .iter()
        .filter(|point| point.ts >= now - HOUR && point.failed && !point.limit)
        .count();
    if failed > 0 {
        items.push(Attention {
            tone: "bad",
            what: format!(
                "{} failed in the last hour",
                format::noun(
                    i64::try_from(failed).unwrap_or(i64::MAX),
                    "request",
                    "requests"
                )
            ),
            why: "Limits are not counted here.".to_owned(),
            link: Some(("Show them", "/admin/activity?outcome=error".to_owned())),
        });
    }
    for route in routes.iter().filter(|route| route.blocked) {
        items.push(Attention {
            tone: "warn",
            what: route.model.clone(),
            why: route.next.clone(),
            link: Some(("Models", "/admin/models".to_owned())),
        });
    }
    items
}

fn routes(data: &Data, ctx: &Ctx) -> Vec<Route> {
    let mut used: Vec<(String, i64)> = Vec::new();
    for point in data.points.iter().filter(|point| point.ts >= ctx.now - DAY) {
        match used.iter_mut().find(|(name, _)| *name == point.model) {
            Some((_, count)) => *count += 1,
            None => used.push((point.model.clone(), 1)),
        }
    }
    used.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    used.into_iter()
        .map(|(id, count)| {
            let (next, blocked) = view::route(
                model::next_in(&data.accounts, &data.catalog, &id, ctx.now),
                ctx.now,
                &ctx.zone,
            );
            Route {
                model: id,
                requests: format::noun(count, "request", "requests"),
                next,
                blocked,
            }
        })
        .collect()
}

fn day_chart(points: &[Point], ctx: &Ctx) -> Svg {
    let buckets = Buckets::new(Range::Day, ctx.now, &ctx.zone);
    let groups: Vec<String> = ["ok", "limit", "error"].map(str::to_owned).to_vec();
    let grid = stats::Grid::new(points, &buckets, By::Outcome, &groups);
    let bars = buckets
        .starts
        .iter()
        .zip(&grid.cells)
        .map(|(start, cells)| Bar {
            tick: if ctx.zone.local(*start).rem_euclid(6 * HOUR) == 0 {
                ctx.zone.strftime(*start, "%H:%M")
            } else {
                String::new()
            },
            head: format!(
                "{}–{}",
                ctx.zone.strftime(*start, "%a %H:%M"),
                ctx.zone.strftime(start + HOUR, "%H:%M")
            ),
            values: cells
                .iter()
                .map(|cell| super::chart::float(cell.requests))
                .collect(),
        })
        .collect();
    BarChart {
        label: "Requests per hour over the last 24 hours, by outcome".to_owned(),
        scale: Scale::Count,
        width: super::chart::WIDE,
        height: 160.0,
        keys: vec![
            Key {
                name: "ok".to_owned(),
                slot: Slot::Muted,
                mark: KeyMark::Block,
            },
            Key {
                name: "limit".to_owned(),
                slot: Slot::Warn,
                mark: KeyMark::Block,
            },
            Key {
                name: "error".to_owned(),
                slot: Slot::Bad,
                mark: KeyMark::Block,
            },
        ],
        bars,
        first: "Hour".to_owned(),
    }
    .render()
}
