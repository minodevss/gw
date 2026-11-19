use std::collections::HashMap;
use std::sync::MutexGuard;

use askama::Template;
use axum::Form;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use url::Url;

use super::chart::{self, Svg};
use super::frame::{Ctx, Flash, Frame, Tab};
use super::view::Stamp;
use super::{Web, failure, fragment, html, origin, redirect_with, see_other, segment};
use crate::format;
use crate::ops;
use crate::ops::key::Snippet;
use crate::store::Point;

const DAY: i64 = 86_400;
const UNUSED_DAYS: i64 = 30;
const ISSUED_SECS: i64 = 10 * 60;

pub(super) struct Issued {
    created_at: i64,
    name: String,
    key: String,
    snippets: Vec<Snippet>,
}

#[derive(Template)]
#[template(path = "admin/keys.html")]
struct ListPage {
    frame: Frame,
    live: String,
}

#[derive(Template)]
#[template(path = "admin/keys_live.html")]
struct ListLive {
    rows: Vec<KeyRow>,
    unused: usize,
}

struct KeyRow {
    name: String,
    path: String,
    created: Stamp,
    last_used: Option<Stamp>,
    idle: Option<String>,
    requests: String,
    tokens: String,
    model: String,
    spark: Svg,
}

#[derive(Template)]
#[template(path = "admin/key_new.html")]
struct NewPage {
    frame: Frame,
    name: String,
    url: String,
    detected: bool,
    error: Option<(&'static str, String)>,
}

#[derive(Template)]
#[template(path = "admin/key_issued.html")]
struct IssuedPage {
    frame: Frame,
    id: String,
    name: String,
    key: String,
    snippets: Vec<Snippet>,
    left: String,
}

#[derive(Template)]
#[template(path = "admin/key_revoke.html")]
struct RevokePage {
    frame: Frame,
    name: String,
    path: String,
    last_used: Option<Stamp>,
    recent: i64,
    error: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct NewForm {
    name: String,
    url: String,
}

#[derive(Deserialize)]
pub(super) struct ConfirmForm {
    confirm: String,
}

pub(super) async fn page(State(web): State<Web>, ctx: Ctx) -> Response {
    match list(&web, &ctx).await {
        Ok(live) => html(
            StatusCode::OK,
            &ListPage {
                frame: ctx.frame(Some(Tab::Keys), "Keys"),
                live,
            },
        ),
        Err(error) => failure(&error),
    }
}

pub(super) async fn live(State(web): State<Web>, ctx: Ctx) -> Response {
    fragment(list(&web, &ctx).await)
}

async fn list(web: &Web, ctx: &Ctx) -> anyhow::Result<String> {
    let since = ctx.zone.day_start(ctx.now) - (UNUSED_DAYS - 1) * DAY;
    let (keys, last_used, points) = ops::with_store(&web.0.home, move |store| {
        Ok((
            store.keys()?,
            store.key_last_used()?,
            store.log_points(since)?,
        ))
    })
    .await?;
    let rows: Vec<KeyRow> = keys
        .into_iter()
        .map(|key| {
            let last = last_used
                .iter()
                .find(|(name, _)| *name == key.name)
                .map(|(_, at)| *at);
            let mine: Vec<&Point> = points
                .iter()
                .filter(|point| point.key.as_deref() == Some(key.name.as_str()))
                .collect();
            let mut days = vec![0.0; usize::try_from(UNUSED_DAYS).unwrap_or(30)];
            for point in &mine {
                let index = usize::try_from((point.ts - since).div_euclid(DAY)).ok();
                if let Some(day) = index.and_then(|index| days.get_mut(index)) {
                    *day += 1.0;
                }
            }
            let tokens: i64 = mine
                .iter()
                .map(|point| point.input + point.cache_read + point.cache_write + point.output)
                .sum();
            let idle_since = last.unwrap_or(key.created_at);
            KeyRow {
                path: segment(&key.name),
                created: Stamp::new(key.created_at, ctx.now, &ctx.zone),
                last_used: last.map(|at| Stamp::new(at, ctx.now, &ctx.zone)),
                idle: (ctx.now - idle_since > UNUSED_DAYS * DAY).then(|| {
                    if last.is_some() {
                        format!("Unused for {} days", (ctx.now - idle_since) / DAY)
                    } else {
                        format!("Never used in {} days", (ctx.now - idle_since) / DAY)
                    }
                }),
                requests: format::count(i64::try_from(mine.len()).unwrap_or(i64::MAX)),
                tokens: format::compact(tokens),
                model: mine.last().map_or_default(|point| point.model.clone()),
                spark: chart::spark(
                    &days,
                    &format!(
                        "{}: {} in the last 30 days, one bar per day",
                        key.name,
                        format::noun(
                            i64::try_from(mine.len()).unwrap_or(i64::MAX),
                            "request",
                            "requests"
                        )
                    ),
                ),
                name: key.name,
            }
        })
        .collect();
    let unused = rows.iter().filter(|row| row.idle.is_some()).count();
    Ok(ListLive { rows, unused }.render()?)
}

pub(super) async fn new_form(ctx: Ctx, headers: HeaderMap) -> Response {
    let url = origin(&headers).unwrap_or_default();
    new_page(&ctx, String::new(), url, true, None, StatusCode::OK)
}

pub(super) async fn create(
    State(web): State<Web>,
    ctx: Ctx,
    Form(form): Form<NewForm>,
) -> Response {
    let name = form.name.trim().to_owned();
    let url = form.url.trim().trim_end_matches('/').to_owned();
    let invalid = |field: &'static str, message: String| {
        new_page(
            &ctx,
            name.clone(),
            url.clone(),
            false,
            Some((field, message)),
            StatusCode::UNPROCESSABLE_ENTITY,
        )
    };
    let taken: Vec<String> = match ops::with_store(&web.0.home, |store| store.keys()).await {
        Ok(keys) => keys.into_iter().map(|key| key.name).collect(),
        Err(error) => return failure(&error),
    };
    if let Err(error) = ops::name::check(&name, &taken) {
        return invalid("name", format!("Name: {error}"));
    }
    if !Url::parse(&url).is_ok_and(|url| matches!(url.scheme(), "http" | "https")) {
        return invalid(
            "url",
            "Enter the http or https address clients use to reach gw.".to_owned(),
        );
    }
    let (key_name, base) = (name.clone(), url.clone());
    let issued = ops::with_store(&web.0.home, move |store| {
        Ok(ops::key::create(
            store,
            &key_name,
            &base,
            crate::clock::now(),
        ))
    })
    .await;
    let issued = match issued {
        Ok(Ok(issued)) => issued,
        Ok(Err(error @ ops::key::Error::Exists(_))) => {
            return invalid("name", format!("Name: {error}"));
        }
        Ok(Err(error @ (ops::key::Error::Random | ops::key::Error::Store(_)))) => {
            return failure(&error);
        }
        Err(error) => return failure(&error),
    };
    let mut bytes = [0_u8; 16];
    if let Err(error) = getrandom::fill(&mut bytes) {
        return failure(&error);
    }
    let id = URL_SAFE_NO_PAD.encode(bytes);
    {
        let mut shown = web.issued();
        shown.insert(
            id.clone(),
            Issued {
                created_at: ctx.now,
                name,
                key: issued.key,
                snippets: issued.snippets,
            },
        );
    }
    see_other(&format!("/keys/issued/{id}"))
}

pub(super) async fn issued(State(web): State<Web>, ctx: Ctx, Path(id): Path<String>) -> Response {
    let page = {
        let shown = web.issued();
        shown
            .get(&id)
            .filter(|issued| ctx.now - issued.created_at < ISSUED_SECS)
            .map(|issued| IssuedPage {
                frame: ctx.frame(Some(Tab::Keys), format!("Key {}", issued.name)),
                id: id.clone(),
                name: issued.name.clone(),
                key: issued.key.clone(),
                snippets: issued.snippets.clone(),
                left: format::remaining(ISSUED_SECS - (ctx.now - issued.created_at)),
            })
    };
    match page {
        Some(page) => html(StatusCode::OK, &page),
        None => redirect_with(
            "/keys",
            &Flash::failed(
                "That key is no longer shown. gw keeps only its hash. If you did not save it, revoke the key and create a new one.",
            ),
        ),
    }
}

pub(super) async fn done(State(web): State<Web>, Path(id): Path<String>) -> Response {
    let name = web.issued().remove(&id).map(|issued| issued.name);
    match name {
        Some(name) => redirect_with(
            "/keys",
            &Flash::done(format!("Created key {name}. gw no longer shows it.")),
        ),
        None => see_other("/keys"),
    }
}

pub(super) async fn confirm_revoke(
    State(web): State<Web>,
    ctx: Ctx,
    Path(name): Path<String>,
) -> Response {
    revoke_page(&web, &ctx, name, None, StatusCode::OK).await
}

pub(super) async fn revoke(
    State(web): State<Web>,
    ctx: Ctx,
    Path(name): Path<String>,
    Form(form): Form<ConfirmForm>,
) -> Response {
    if form.confirm.trim() != name {
        return revoke_page(
            &web,
            &ctx,
            name.clone(),
            Some(format!(
                "Not revoked. The name you typed does not match {name}."
            )),
            StatusCode::UNPROCESSABLE_ENTITY,
        )
        .await;
    }
    let wanted = name.clone();
    match ops::with_store(&web.0.home, move |store| store.revoke_key(&wanted)).await {
        Ok(true) => redirect_with(
            "/keys",
            &Flash::done(format!(
                "Revoked {name}. Clients using it now get 401 responses."
            )),
        ),
        Ok(false) => super::missing(&ctx),
        Err(error) => failure(&error),
    }
}

async fn revoke_page(
    web: &Web,
    ctx: &Ctx,
    name: String,
    error: Option<String>,
    status: StatusCode,
) -> Response {
    let since = ctx.now - DAY;
    let wanted = name.clone();
    let loaded = ops::with_store(&web.0.home, move |store| {
        Ok((
            store.keys()?.iter().any(|key| key.name == wanted),
            store.key_last_used()?,
            store.log_points(since)?,
        ))
    })
    .await;
    let (exists, last_used, points) = match loaded {
        Ok(loaded) => loaded,
        Err(error) => return failure(&error),
    };
    if !exists {
        return super::missing(ctx);
    }
    let recent = points
        .iter()
        .filter(|point| point.key.as_deref() == Some(name.as_str()))
        .count();
    html(
        status,
        &RevokePage {
            frame: ctx.frame(Some(Tab::Keys), "Revoke key"),
            path: segment(&name),
            last_used: last_used
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, at)| Stamp::new(*at, ctx.now, &ctx.zone)),
            recent: i64::try_from(recent).unwrap_or(i64::MAX),
            name,
            error,
        },
    )
}

fn new_page(
    ctx: &Ctx,
    name: String,
    url: String,
    detected: bool,
    error: Option<(&'static str, String)>,
    status: StatusCode,
) -> Response {
    html(
        status,
        &NewPage {
            frame: ctx.frame(Some(Tab::Keys), "Create key"),
            name,
            url,
            detected,
            error,
        },
    )
}

impl Web {
    fn issued(&self) -> MutexGuard<'_, HashMap<String, Issued>> {
        let mut issued = super::lock(&self.0.issued);
        let now = crate::clock::now();
        issued.retain(|_, shown| now - shown.created_at < ISSUED_SECS);
        issued
    }
}
