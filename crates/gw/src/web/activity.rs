use std::collections::HashMap;

use askama::Template;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Response;

use super::chart::{Bar, BarChart, Key, KeyMark, Scale, Slot, Svg, duration, float};
use super::frame::{Ctx, Frame, Tab};
use super::view::Stamp;
use super::{Web, failure, fragment, html, segment};
use crate::format;
use crate::ops;
use crate::ops::log::Filter;
use crate::store::LogRow;

const PAGE_ROWS: usize = 100;
const SCAN_CHUNK: u32 = 1000;
const SCAN_LIMIT: usize = if cfg!(test) { 3_000 } else { 50_000 };
const SESSION_ROWS: u32 = 1000;
const NEIGHBOURS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    All,
    Errors,
    Limits,
}

const OUTCOMES: [Outcome; 3] = [Outcome::All, Outcome::Errors, Outcome::Limits];

impl Outcome {
    fn parse(text: Option<&str>) -> Self {
        OUTCOMES
            .into_iter()
            .find(|outcome| Some(outcome.query()) == text)
            .unwrap_or(Self::All)
    }

    fn query(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Errors => "error",
            Self::Limits => "limit",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Errors => "Errors",
            Self::Limits => "Limits",
        }
    }

    fn matches(self, row: &LogRow) -> bool {
        match self {
            Self::All => true,
            Self::Errors => kind(row) == Kind::Error,
            Self::Limits => kind(row) == Kind::Limit,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Ok,
    Limit,
    Error,
}

fn kind(row: &LogRow) -> Kind {
    if row.reason == "limit" {
        Kind::Limit
    } else if row.status >= 400 || row.error.is_some() {
        Kind::Error
    } else {
        Kind::Ok
    }
}

struct Line {
    href: String,
    at: String,
    at_title: String,
    key: String,
    model: String,
    account: String,
    why: String,
    result: String,
    tone: &'static str,
    input: String,
    cached: String,
    output: String,
    took: String,
    error: Option<String>,
}

fn line(row: &LogRow, ctx: &Ctx) -> Line {
    let clock = ctx.zone.clock(ctx.now);
    let today = ctx.zone.day_start(row.ts) == ctx.zone.day_start(ctx.now);
    let input = row.input.unwrap_or_default()
        + row.cache_read.unwrap_or_default()
        + row.cache_write.unwrap_or_default();
    let (result, tone) = match kind(row) {
        Kind::Limit => ("limit".to_owned(), "warn"),
        Kind::Error if row.status < 400 => ("stream failed".to_owned(), "bad"),
        Kind::Error => (format!("{} error", row.status), "bad"),
        Kind::Ok => (row.status.to_string(), "ok"),
    };
    let has_tokens = row.input.is_some() || row.output.is_some();
    Line {
        href: format!("/admin/request/{}", row.id),
        at: if today {
            clock.seconds(row.ts)
        } else {
            clock.when(row.ts)
        },
        at_title: format!("{}, {}", clock.when(row.ts), format::ago(ctx.now - row.ts)),
        key: row.key.clone().unwrap_or_default(),
        model: row.model.clone(),
        account: row.account.clone().unwrap_or_else(|| "—".to_owned()),
        why: why(&row.reason),
        result,
        tone,
        input: if has_tokens {
            format::compact(input)
        } else {
            "—".to_owned()
        },
        cached: match (input, row.cache_read) {
            (total, Some(read)) if total > 0 && read > 0 => {
                format!("{}%", format::percent(float(read) / float(total)))
            }
            _ => String::new(),
        },
        output: row.output.map_or_else(|| "—".to_owned(), format::compact),
        took: duration(float(row.duration_ms)),
        error: row.error.clone(),
    }
}

fn why(reason: &str) -> String {
    match reason {
        "reused" => "same conversation".to_owned(),
        "new" => "new conversation".to_owned(),
        "limit" => "limit, tried the next account".to_owned(),
        "rejected" => "rejected before sending".to_owned(),
        other => other.to_owned(),
    }
}

struct Scan {
    rows: Vec<LogRow>,
    next: Option<i64>,
    gave_up: bool,
}

struct Choice {
    query: String,
    label: &'static str,
    current: bool,
}

#[derive(Template)]
#[template(path = "admin/activity.html")]
struct Page {
    frame: Frame,
    q: String,
    outcome: &'static str,
    outcomes: Vec<Choice>,
    live: String,
}

#[derive(Template)]
#[template(path = "admin/activity_live.html")]
struct Live {
    poll: Option<String>,
    lines: Vec<Line>,
    older: Option<String>,
    newest: Option<String>,
    filtered: bool,
    scanned_out: bool,
    updated: String,
}

pub(super) async fn page(
    State(web): State<Web>,
    ctx: Ctx,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let q = query.get("q").cloned().unwrap_or_default();
    let outcome = Outcome::parse(query.get("outcome").map(String::as_str));
    let filter_query = |outcome: Outcome| {
        let mut parts = Vec::new();
        if !q.trim().is_empty() {
            parts.push(format!("q={}", encode(&q)));
        }
        if outcome != Outcome::All {
            parts.push(format!("outcome={}", outcome.query()));
        }
        parts.join("&")
    };
    match build(&web, &ctx, &query).await {
        Ok(live) => html(
            StatusCode::OK,
            &Page {
                frame: ctx.frame(Some(Tab::Activity), "Activity"),
                outcomes: OUTCOMES
                    .into_iter()
                    .map(|choice| Choice {
                        query: filter_query(choice),
                        label: choice.label(),
                        current: choice == outcome,
                    })
                    .collect(),
                q,
                outcome: outcome.query(),
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

fn encode(text: &str) -> String {
    url::form_urlencoded::byte_serialize(text.as_bytes()).collect()
}

async fn build(web: &Web, ctx: &Ctx, query: &HashMap<String, String>) -> anyhow::Result<String> {
    let q = query.get("q").cloned().unwrap_or_default();
    let outcome = Outcome::parse(query.get("outcome").map(String::as_str));
    let before = query
        .get("before")
        .and_then(|text| text.parse::<i64>().ok());
    let filtered = !q.trim().is_empty() || outcome != Outcome::All;
    let filter = Filter::parse_loose(&q);
    let start = before.unwrap_or(i64::MAX);
    let chunk_rows = if filtered {
        SCAN_CHUNK
    } else {
        u32::try_from(PAGE_ROWS + 1).unwrap_or(SCAN_CHUNK)
    };
    let scan = ops::with_store(&web.0.home, move |store| {
        let mut found: Vec<LogRow> = Vec::new();
        let mut cursor = start;
        let mut scanned = 0;
        loop {
            let chunk = store.log_before(cursor, chunk_rows)?;
            if chunk.is_empty() {
                return Ok(Scan {
                    rows: found,
                    next: None,
                    gave_up: false,
                });
            }
            for row in chunk {
                cursor = row.id;
                scanned += 1;
                if filter.matches(&row) && outcome.matches(&row) {
                    if found.len() == PAGE_ROWS {
                        let next = found.last().map(|last| last.id);
                        return Ok(Scan {
                            rows: found,
                            next,
                            gave_up: false,
                        });
                    }
                    found.push(row);
                }
                if scanned >= SCAN_LIMIT {
                    return Ok(Scan {
                        rows: found,
                        next: Some(cursor),
                        gave_up: true,
                    });
                }
            }
        }
    })
    .await?;
    let mut base = Vec::new();
    if !q.trim().is_empty() {
        base.push(format!("q={}", encode(&q)));
    }
    if outcome != Outcome::All {
        base.push(format!("outcome={}", outcome.query()));
    }
    let with = |extra: Option<String>| {
        let mut parts = base.clone();
        parts.extend(extra);
        parts.join("&")
    };
    let older = scan
        .next
        .map(|id| format!("/admin/activity?{}", with(Some(format!("before={id}")))));
    Ok(Live {
        poll: (before.is_none() && !filtered)
            .then(|| format!("/admin/live/activity?{}", with(None))),
        newest: before.map(|_| format!("/admin/activity?{}", with(None))),
        lines: scan.rows.iter().map(|row| line(row, ctx)).collect(),
        older,
        filtered,
        scanned_out: scan.gave_up,
        updated: ctx.zone.strftime(ctx.now, "%H:%M:%S"),
    }
    .render()?)
}

#[derive(Template)]
#[template(path = "admin/request.html")]
struct RequestPage {
    frame: Frame,
    id: i64,
    line: Line,
    when: Stamp,
    utc: String,
    session: Option<(String, String)>,
    api: String,
    reason: String,
    status: u16,
    note: Option<String>,
    tokens: Vec<(&'static str, String)>,
    neighbours: Vec<(bool, Line)>,
}

pub(super) async fn request(State(web): State<Web>, ctx: Ctx, Path(id): Path<String>) -> Response {
    let Ok(id) = id.parse::<i64>() else {
        return super::missing(&ctx);
    };
    let loaded = ops::with_store(&web.0.home, move |store| {
        let Some(row) = store.log_row(id)? else {
            return Ok(None);
        };
        let session = match &row.session {
            Some(session) => store.log_session(session, SESSION_ROWS)?,
            None => Vec::new(),
        };
        Ok(Some((row, session)))
    })
    .await;
    let (row, session) = match loaded {
        Ok(Some(found)) => found,
        Ok(None) => return super::missing(&ctx),
        Err(error) => return failure(&error),
    };
    let position = session.iter().position(|other| other.id == row.id);
    let neighbours = position.map_or_else(Vec::new, |index| {
        let first = index.saturating_sub(NEIGHBOURS);
        session
            .iter()
            .skip(first)
            .take(NEIGHBOURS * 2 + 1)
            .map(|other| (other.id == row.id, line(other, &ctx)))
            .collect()
    });
    let count = |value: Option<i64>| value.map_or_else(|| "not reported".to_owned(), format::count);
    let page = RequestPage {
        frame: ctx.frame(Some(Tab::Activity), format!("Request {}", row.id)),
        id: row.id,
        when: Stamp::new(row.ts, ctx.now, &ctx.zone),
        utc: crate::clock::format_utc(row.ts),
        session: row.session.as_ref().map(|session| {
            (
                format!("/admin/session/{}", segment(session)),
                session.clone(),
            )
        }),
        api: row.api.clone(),
        reason: why(&row.reason),
        status: row.status,
        note: row.note.clone(),
        tokens: vec![
            ("Uncached input", count(row.input)),
            ("Cache read", count(row.cache_read)),
            ("Cache write", count(row.cache_write)),
            ("Output", count(row.output)),
        ],
        line: line(&row, &ctx),
        neighbours,
    };
    html(StatusCode::OK, &page)
}

#[derive(Template)]
#[template(path = "admin/session.html")]
struct SessionPage {
    frame: Frame,
    id: String,
    facts: Vec<(String, String)>,
    chart: Svg,
    lines: Vec<(Option<String>, Line)>,
    truncated: bool,
}

pub(super) async fn session(State(web): State<Web>, ctx: Ctx, Path(id): Path<String>) -> Response {
    let wanted = id.clone();
    let rows = match ops::with_store(&web.0.home, move |store| {
        store.log_session(&wanted, SESSION_ROWS)
    })
    .await
    {
        Ok(rows) => rows,
        Err(error) => return failure(&error),
    };
    let (Some(first), Some(last)) = (rows.first(), rows.last()) else {
        return super::missing(&ctx);
    };
    let mut accounts: Vec<String> = Vec::new();
    let mut moves = 0;
    let mut previous: Option<&str> = None;
    let mut lines = Vec::new();
    for row in &rows {
        let account = row.account.as_deref();
        let moved = match (previous, account) {
            (Some(before), Some(now)) if before != now => {
                moves += 1;
                Some(format!("Moved from {before} to {now}"))
            }
            _ => None,
        };
        if let Some(account) = account {
            if !accounts.iter().any(|known| known == account) {
                accounts.push(account.to_owned());
            }
            previous = Some(account);
        }
        lines.push((moved, line(row, &ctx)));
    }
    lines.reverse();
    let read: i64 = rows.iter().filter_map(|row| row.cache_read).sum();
    let input: i64 = rows
        .iter()
        .map(|row| {
            row.input.unwrap_or_default()
                + row.cache_read.unwrap_or_default()
                + row.cache_write.unwrap_or_default()
        })
        .sum();
    let clock = ctx.zone.clock(ctx.now);
    let mut facts = vec![
        (
            "Requests".to_owned(),
            format::count(i64::try_from(rows.len()).unwrap_or(i64::MAX)),
        ),
        (
            "Time".to_owned(),
            format!("{} to {}", clock.when(first.ts), clock.when(last.ts)),
        ),
        ("Key".to_owned(), last.key.clone().unwrap_or_default()),
        ("Accounts".to_owned(), accounts.join(", ")),
    ];
    if moves > 0 {
        facts.push((
            "Moves".to_owned(),
            format!("{moves}, each one starts the prompt cache over"),
        ));
    }
    if input > 0 {
        facts.push((
            "Input read from cache".to_owned(),
            format!("{}%", format::percent(float(read) / float(input))),
        ));
    }
    let chart = BarChart {
        label: "Input tokens per request in this conversation, split by cache use".to_owned(),
        scale: Scale::Tokens,
        width: super::chart::WIDE,
        height: 200.0,
        keys: vec![
            Key {
                name: "cache read".to_owned(),
                slot: Slot::nth(0),
                mark: KeyMark::Block,
            },
            Key {
                name: "cache write".to_owned(),
                slot: Slot::nth(1),
                mark: KeyMark::Block,
            },
            Key {
                name: "uncached".to_owned(),
                slot: Slot::Muted,
                mark: KeyMark::Block,
            },
        ],
        bars: rows
            .iter()
            .enumerate()
            .map(|(index, row)| Bar {
                tick: if index == 0 || (index + 1) % 10 == 0 {
                    (index + 1).to_string()
                } else {
                    String::new()
                },
                head: format!(
                    "#{} at {}{}",
                    index + 1,
                    clock.seconds(row.ts),
                    row.account
                        .as_ref()
                        .map_or_else(String::new, |account| format!(", {account}"))
                ),
                values: vec![
                    float(row.cache_read.unwrap_or_default()),
                    float(row.cache_write.unwrap_or_default()),
                    float(row.input.unwrap_or_default()),
                ],
            })
            .collect(),
        first: "Request".to_owned(),
    }
    .render();
    let truncated = rows.len() >= usize::try_from(SESSION_ROWS).unwrap_or(usize::MAX);
    html(
        StatusCode::OK,
        &SessionPage {
            frame: ctx.frame(Some(Tab::Activity), "Conversation"),
            id,
            facts,
            chart,
            lines,
            truncated,
        },
    )
}
