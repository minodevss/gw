use std::collections::HashMap;

use askama::Template;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::Response;

use super::frame::{Ctx, Flash, Frame, Tab};
use super::{Web, failure, fragment, html, redirect_with};
use crate::catalog::view;
use crate::format;
use crate::ops::{self, model};

const WEEK: i64 = 7 * 86_400;

#[derive(Template)]
#[template(path = "admin/models.html")]
struct Page {
    frame: Frame,
    q: String,
    live: String,
}

#[derive(Template)]
#[template(path = "admin/models_live.html")]
struct Live {
    poll: String,
    rows: Vec<Row>,
    total: usize,
    filtered: bool,
}

struct Row {
    id: String,
    next: String,
    blocked: bool,
    context: String,
    max_output: String,
    efforts: String,
    traits: String,
    used: String,
    accounts: Vec<(String, &'static str)>,
}

pub(super) async fn page(
    State(web): State<Web>,
    ctx: Ctx,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let q = query.get("q").cloned().unwrap_or_default();
    match build(&web, &ctx, &q).await {
        Ok(live) => html(
            StatusCode::OK,
            &Page {
                frame: ctx.frame(Some(Tab::Models), "Models"),
                q,
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
    let q = query.get("q").cloned().unwrap_or_default();
    fragment(build(&web, &ctx, &q).await)
}

async fn build(web: &Web, ctx: &Ctx, q: &str) -> anyhow::Result<String> {
    let since = ctx.now - WEEK;
    let (accounts, catalog, points) = ops::with_store(&web.0.home, move |store| {
        Ok((
            store.accounts()?,
            store.catalog()?,
            store.log_points(since)?,
        ))
    })
    .await?;
    let mut used: HashMap<String, i64> = HashMap::new();
    for point in &points {
        let (model, _) = crate::select::parse_model(&point.model);
        *used.entry(model.to_owned()).or_default() += 1;
    }
    let words: Vec<String> = q.split_whitespace().map(str::to_lowercase).collect();
    let views = view::models(&catalog, &accounts);
    let total = views.len();
    let mut rows: Vec<Row> = views
        .into_iter()
        .filter(|view| {
            let haystack = format!("{} {}", view.id, view.accounts.join(" ")).to_lowercase();
            words.iter().all(|word| haystack.contains(word.as_str()))
        })
        .map(|view| {
            let (next, blocked) = super::view::route(
                model::next_in(&accounts, &catalog, &view.id, ctx.now),
                ctx.now,
                &ctx.zone,
            );
            let mut traits = Vec::new();
            if view.image {
                traits.push("images");
            }
            if view.fast {
                traits.push("fast mode");
            }
            Row {
                next,
                blocked,
                context: view.context.map_or_else(
                    || "—".to_owned(),
                    |v| format::compact(i64::try_from(v).unwrap_or(i64::MAX)),
                ),
                max_output: view
                    .max_output
                    .map_or_else(|| "—".to_owned(), |v| format::compact(i64::from(v))),
                efforts: view
                    .efforts
                    .as_ref()
                    .map_or_else(String::new, |efforts| efforts.join(", ")),
                traits: traits.join(", "),
                used: used
                    .get(&view.id)
                    .map_or_else(String::new, |count| format::count(*count)),
                accounts: view
                    .accounts
                    .iter()
                    .map(|name| {
                        let tone = accounts
                            .iter()
                            .find(|account| account.name == *name)
                            .map_or("idle", |account| {
                                super::view::state(account, true, ctx.now, &ctx.zone).tone
                            });
                        (name.clone(), tone)
                    })
                    .collect(),
                id: view.id,
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        let count = |row: &Row| used.get(&row.id).copied().unwrap_or_default();
        count(b).cmp(&count(a)).then_with(|| a.id.cmp(&b.id))
    });
    let poll = if q.trim().is_empty() {
        "/admin/live/models".to_owned()
    } else {
        format!(
            "/admin/live/models?q={}",
            url::form_urlencoded::byte_serialize(q.as_bytes()).collect::<String>()
        )
    };
    Ok(Live {
        poll,
        rows,
        total,
        filtered: !words.is_empty(),
    }
    .render()?)
}

pub(super) async fn refresh(State(web): State<Web>) -> Response {
    let flash = match model::refresh(&web.0.home, &web.0.http, None).await {
        Ok(report) if report.is_empty() => Flash::failed("There are no accounts to refresh."),
        Ok(report) => {
            let failed: Vec<String> = report
                .iter()
                .filter_map(|refreshed| {
                    refreshed
                        .result
                        .as_ref()
                        .err()
                        .map(|error| format!("{}: {error}", refreshed.account))
                })
                .collect();
            let fine = report.len() - failed.len();
            let models: usize = report
                .iter()
                .filter_map(|refreshed| refreshed.result.as_ref().ok())
                .sum();
            let summary = format!(
                "Refreshed {} with {} in all.",
                format::noun(i64::try_from(fine).unwrap_or(0), "account", "accounts"),
                format::noun(i64::try_from(models).unwrap_or(0), "model", "models")
            );
            if failed.is_empty() {
                Flash::done(summary)
            } else {
                Flash::failed(format!("{summary} Failed: {}.", failed.join("; ")))
            }
        }
        Err(error) => Flash::failed(format!("Refresh failed: {error:#}")),
    };
    redirect_with("/models", &flash)
}
