use askama::Template;
use axum::Form;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde::Deserialize;
use url::Url;

use super::chart::duration;
use super::frame::{Ctx, Flash, Frame, Tab};
use super::login::{self, Target};
use super::stats;
use super::usage::{self, AccountChart};
use super::view::{self, AccountRow, Pool, Stamp};
use super::{Web, failure, fragment, html, redirect_with, see_other, segment};
use crate::account::Secret;
use crate::clock;
use crate::display::{PROVIDERS, kind_label, method_text, provider_title};
use crate::format;
use crate::ops;
use crate::provider::{Billing, Kind, Method, Provider, can_query_usage};

const APIS: [(&str, &str); 3] = [
    ("chat", "OpenAI Chat Completions (/v1/chat/completions)"),
    ("responses", "OpenAI Responses (/v1/responses)"),
    ("messages", "Anthropic Messages (/v1/messages)"),
];
const DAY: i64 = 86_400;
const FIVE_HOURS: i64 = 5 * 3600;
const RECENT_ROWS: u32 = 500;

#[derive(Template)]
#[template(path = "admin/accounts.html")]
struct ListPage {
    frame: Frame,
    live: String,
}

#[derive(Template)]
#[template(path = "admin/accounts_live.html")]
struct ListLive {
    groups: Vec<Group>,
    updated: String,
}

struct Group {
    title: &'static str,
    pools: Vec<Pool>,
    rows: Vec<AccountRow>,
}

#[derive(Template)]
#[template(path = "admin/account_choose.html")]
struct ChoosePage {
    frame: Frame,
    groups: Vec<KindGroup>,
}

struct KindGroup {
    title: &'static str,
    options: Vec<KindOption>,
}

struct KindOption {
    label: &'static str,
    hint: String,
    billing: &'static str,
    path: String,
}

#[derive(Template)]
#[template(path = "admin/account_new.html")]
struct NewPage {
    frame: Frame,
    heading: String,
    hint: String,
    action: String,
    name: String,
    secret_label: Option<&'static str>,
    secret_hint: &'static str,
    custom: bool,
    url: String,
    api: String,
    apis: [(&'static str, &'static str); 3],
    flow: bool,
    error: Option<Problem>,
}

struct Problem {
    field: &'static str,
    text: String,
}

#[derive(Template)]
#[template(path = "admin/account.html")]
struct DetailPage {
    frame: Frame,
    name: String,
    path: String,
    provider: &'static str,
    kind: &'static str,
    added: Stamp,
    state: view::StateView,
    windows: Vec<view::WindowRow>,
    read: Option<Stamp>,
    charts: Vec<AccountChart>,
    facts: Vec<(String, String)>,
    sessions: Vec<SessionLine>,
    errors: Vec<(Stamp, String, String)>,
    models: Vec<String>,
    sign_in: SignIn,
    usage: bool,
    billing: &'static str,
}

struct SessionLine {
    href: String,
    short: String,
    key: String,
    model: String,
    at: Stamp,
}

enum SignIn {
    Flow,
    Paste(&'static str),
    None,
}

#[derive(Deserialize)]
pub(super) struct NewForm {
    name: String,
    #[serde(default)]
    secret: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    api: String,
}

#[derive(Deserialize)]
pub(super) struct SecretForm {
    #[serde(default)]
    secret: String,
}

#[derive(Deserialize)]
pub(super) struct RenameForm {
    new_name: String,
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
                frame: ctx.frame(Some(Tab::Accounts), "Accounts"),
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
    let (accounts, catalog) = ops::with_store(&web.0.home, |store| {
        Ok((store.accounts()?, store.catalog()?))
    })
    .await?;
    let groups = PROVIDERS
        .into_iter()
        .filter_map(|provider| {
            let rows: Vec<AccountRow> = view::sorted(&accounts, provider, ctx.now)
                .into_iter()
                .map(|account| {
                    view::account_row(account, view::serves(&catalog, account), ctx.now, &ctx.zone)
                })
                .collect();
            (!rows.is_empty()).then(|| Group {
                title: provider_title(provider),
                pools: view::pools(&accounts, provider, ctx.now, &ctx.zone)
                    .into_iter()
                    .filter(|pool| pool.count > 1)
                    .collect(),
                rows,
            })
        })
        .collect();
    Ok(ListLive {
        groups,
        updated: ctx.zone.strftime(ctx.now, "%H:%M:%S"),
    }
    .render()?)
}

pub(super) async fn choose(ctx: Ctx) -> Response {
    let groups = PROVIDERS
        .into_iter()
        .map(|provider| KindGroup {
            title: provider_title(provider),
            options: provider
                .methods()
                .iter()
                .filter_map(|method| Kind::new(provider, *method).ok())
                .map(|kind| {
                    let (label, hint) = method_text(provider, kind.method());
                    KindOption {
                        label,
                        hint: format::capitalize(hint),
                        billing: match (provider, kind.billing()) {
                            (Provider::Custom, _) => "",
                            (_, Billing::Sub) => "Subscription",
                            (_, Billing::Api) => "Pay as you go",
                        },
                        path: format!("{provider}/{}", kind.method()),
                    }
                })
                .collect(),
        })
        .collect();
    html(
        StatusCode::OK,
        &ChoosePage {
            frame: ctx.frame(Some(Tab::Accounts), "Add account"),
            groups,
        },
    )
}

pub(super) async fn new_form(
    State(web): State<Web>,
    ctx: Ctx,
    Path((provider, method)): Path<(String, String)>,
) -> Response {
    let Some(kind) = parse_kind(&provider, &method) else {
        return super::missing(&ctx);
    };
    let name = match ops::account::list(&web.0.home).await {
        Ok(accounts) => ops::account::default_name(&accounts, kind),
        Err(error) => return failure(&error),
    };
    let form = NewForm {
        name,
        secret: String::new(),
        url: String::new(),
        api: "chat".to_owned(),
    };
    new_page(&ctx, kind, &form, None, StatusCode::OK)
}

pub(super) async fn create(
    State(web): State<Web>,
    ctx: Ctx,
    Path((provider, method)): Path<(String, String)>,
    Form(form): Form<NewForm>,
) -> Response {
    let Some(kind) = parse_kind(&provider, &method) else {
        return super::missing(&ctx);
    };
    let invalid = |field: &'static str, text: String| {
        new_page(
            &ctx,
            kind,
            &form,
            Some(Problem { field, text }),
            StatusCode::UNPROCESSABLE_ENTITY,
        )
    };
    let name = form.name.trim().to_owned();
    let taken: Vec<String> = match ops::account::list(&web.0.home).await {
        Ok(accounts) => accounts.into_iter().map(|account| account.name).collect(),
        Err(error) => return failure(&error),
    };
    if let Err(error) = ops::name::check(&name, &taken) {
        return invalid("name", format!("Name: {error}"));
    }
    let secret = if kind.provider() == Provider::Custom {
        let url = form.url.trim();
        if !Url::parse(url).is_ok_and(|url| matches!(url.scheme(), "http" | "https")) {
            return invalid("url", "Enter an http or https URL.".to_owned());
        }
        if crate::forward::parse_api(&form.api).is_none() {
            return invalid("api", "Choose the API the endpoint speaks.".to_owned());
        }
        let key = form.secret.trim();
        Secret::Custom {
            url: url.to_owned(),
            api: form.api.clone(),
            key: (!key.is_empty()).then(|| key.to_owned()),
        }
    } else if ops::account::needs_paste(kind) {
        let value = form.secret.trim();
        if value.is_empty() {
            return invalid(
                "secret",
                format!("Paste the {}.", secret_label(kind).to_lowercase()),
            );
        }
        ops::account::pasted_secret(kind, value.to_owned(), clock::now())
    } else {
        return match login::start(&web, kind, Target::New(name)).await {
            Ok(id) => see_other(&format!("/logins/{id}")),
            Err(error) => invalid("flow", format!("Could not start the sign-in: {error:#}")),
        };
    };
    if let Err(error) = ops::account::add(&web.0.home, name.clone(), kind, secret).await {
        return invalid("name", format!("{error:#}"));
    }
    let flash = added(&web, &name).await;
    redirect_with(&account_path(&name), &flash)
}

// A new account is useful only once gw knows its models, so adding one fetches them right away,
// as the CLI and the TUI do.
pub(super) async fn added(web: &Web, name: &str) -> Flash {
    match ops::model::refresh(&web.0.home, &web.0.http, Some(name)).await {
        Ok(report) => match report.into_iter().next().map(|refreshed| refreshed.result) {
            Some(Ok(count)) => Flash::done(format!(
                "Added {name} with {}.",
                format::noun(i64::try_from(count).unwrap_or(i64::MAX), "model", "models")
            )),
            Some(Err(error)) => Flash::failed(format!(
                "Added {name}. Fetching its model list failed, so gw sends it nothing yet: {error}"
            )),
            None => Flash::done(format!("Added {name}.")),
        },
        Err(error) => Flash::failed(format!(
            "Added {name}. Fetching its model list failed, so gw sends it nothing yet: {error:#}"
        )),
    }
}

pub(super) async fn detail(State(web): State<Web>, ctx: Ctx, Path(name): Path<String>) -> Response {
    detail_page(&web, &ctx, &name).await
}

pub(super) async fn sign_in_again(
    State(web): State<Web>,
    ctx: Ctx,
    Path(name): Path<String>,
    Form(form): Form<SecretForm>,
) -> Response {
    let Ok(account) = ops::account::find(&web.0.home, &name).await else {
        return super::missing(&ctx);
    };
    let back = account_path(&name);
    match sign_in(account.kind) {
        SignIn::Flow => match login::start(&web, account.kind, Target::Again(name.clone())).await {
            Ok(id) => see_other(&format!("/logins/{id}")),
            Err(error) => redirect_with(
                &back,
                &Flash::failed(format!("Could not start the sign-in: {error:#}")),
            ),
        },
        SignIn::Paste(label) => {
            let value = form.secret.trim();
            if value.is_empty() {
                return redirect_with(
                    &back,
                    &Flash::failed(format!("Paste the new {}.", label.to_lowercase())),
                );
            }
            let secret = ops::account::pasted_secret(account.kind, value.to_owned(), clock::now());
            let flash = match ops::account::relogin(&web.0.home, &name, secret).await {
                Ok(()) => Flash::done(format!("Replaced the {} for {name}.", label.to_lowercase())),
                Err(error) => Flash::failed(format!("{error:#}")),
            };
            redirect_with(&back, &flash)
        }
        SignIn::None => see_other(&back),
    }
}

pub(super) async fn refresh_usage(State(web): State<Web>, Path(name): Path<String>) -> Response {
    let flash = match ops::account::refresh_usage(&web.0.home, &web.0.http, &name).await {
        Ok(()) => Flash::done("Usage refreshed."),
        Err(error) => Flash::failed(format!("Usage refresh failed: {error:#}")),
    };
    redirect_with(&account_path(&name), &flash)
}

pub(super) async fn refresh_account_models(
    State(web): State<Web>,
    Path(name): Path<String>,
) -> Response {
    let flash = match ops::model::refresh(&web.0.home, &web.0.http, Some(&name)).await {
        Ok(report) => match report.into_iter().next().map(|refreshed| refreshed.result) {
            Some(Ok(count)) => Flash::done(format!(
                "Fetched {}.",
                format::noun(i64::try_from(count).unwrap_or(i64::MAX), "model", "models")
            )),
            Some(Err(error)) => Flash::failed(format!("Model list failed: {error}")),
            None => Flash::failed(format!("There is no account named {name}.")),
        },
        Err(error) => Flash::failed(format!("Model list failed: {error:#}")),
    };
    redirect_with(&account_path(&name), &flash)
}

pub(super) async fn rename(
    State(web): State<Web>,
    Path(name): Path<String>,
    Form(form): Form<RenameForm>,
) -> Response {
    let new_name = form.new_name.trim();
    if new_name == name {
        return see_other(&account_path(&name));
    }
    match ops::account::rename(&web.0.home, &name, new_name).await {
        Ok(()) => redirect_with(
            &account_path(new_name),
            &Flash::done(format!("Renamed {name} to {new_name}.")),
        ),
        Err(error) => redirect_with(
            &account_path(&name),
            &Flash::failed(format!("Could not rename: {error:#}")),
        ),
    }
}

pub(super) async fn remove(
    State(web): State<Web>,
    Path(name): Path<String>,
    Form(form): Form<ConfirmForm>,
) -> Response {
    if form.confirm.trim() != name {
        return redirect_with(
            &account_path(&name),
            &Flash::failed(format!(
                "Not removed. The name you typed does not match {name}."
            )),
        );
    }
    match ops::account::remove(&web.0.home, &name).await {
        Ok(()) => redirect_with("/accounts", &Flash::done(format!("Removed {name}."))),
        Err(error) => redirect_with(
            &account_path(&name),
            &Flash::failed(format!("Could not remove: {error:#}")),
        ),
    }
}

async fn detail_page(web: &Web, ctx: &Ctx, name: &str) -> Response {
    let Ok(account) = ops::account::find(&web.0.home, name).await else {
        return super::missing(ctx);
    };
    let wanted = name.to_owned();
    let wanted_rows = name.to_owned();
    let since = ctx.now - DAY;
    let loaded = ops::with_store(&web.0.home, move |store| {
        Ok((
            store.catalog()?,
            store.log_points(since)?,
            store.log_for_account(&wanted_rows, RECENT_ROWS)?,
        ))
    })
    .await;
    let (catalog, points, recent) = match loaded {
        Ok(loaded) => loaded,
        Err(error) => return failure(&error),
    };
    let charts = match usage::account_charts(web, ctx, &account).await {
        Ok(charts) => charts,
        Err(error) => return failure(&error),
    };
    let mine: Vec<_> = points
        .into_iter()
        .filter(|point| point.account.as_deref() == Some(wanted.as_str()))
        .collect();
    let mut day = stats::total(&mine);
    let five: Vec<_> = mine
        .iter()
        .filter(|point| point.ts >= ctx.now - FIVE_HOURS)
        .cloned()
        .collect();
    let five = stats::total(&five);
    let mut facts = vec![
        (
            "Last 24 hours".to_owned(),
            format!(
                "{}, {}, {}",
                format::noun(day.requests, "request", "requests"),
                format::noun(day.limits, "limit", "limits"),
                format::noun(day.errors, "error", "errors")
            ),
        ),
        (
            "Tokens, last 5 hours".to_owned(),
            format!(
                "{} in{}, {} out",
                format::compact(five.input()),
                five.cache_ratio().map_or_else(String::new, |ratio| format!(
                    " ({}% cached)",
                    format::percent(ratio)
                )),
                format::compact(five.output)
            ),
        ),
    ];
    if let Some(p50) = day.percentile(0.5) {
        let p95 = day.percentile(0.95).unwrap_or(p50);
        facts.push((
            "Request time, 24 hours".to_owned(),
            format!(
                "{} median, {} at p95",
                duration(super::chart::float(p50)),
                duration(super::chart::float(p95))
            ),
        ));
    }
    let mut sessions: Vec<SessionLine> = Vec::new();
    let mut errors = Vec::new();
    for row in recent
        .iter()
        .filter(|row| row.account.as_deref() == Some(wanted.as_str()))
    {
        if let Some(session) = &row.session
            && sessions.len() < 5
            && row.ts >= ctx.now - DAY
            && sessions.iter().all(|line| line.short != short_id(session))
        {
            sessions.push(SessionLine {
                href: format!("/admin/session/{}", segment(session)),
                short: short_id(session),
                key: row.key.clone().unwrap_or_default(),
                model: row.model.clone(),
                at: Stamp::new(row.ts, ctx.now, &ctx.zone),
            });
        }
        if let Some(message) = &row.error
            && errors.len() < 5
        {
            errors.push((
                Stamp::new(row.ts, ctx.now, &ctx.zone),
                message.clone(),
                format!("/admin/request/{}", row.id),
            ));
        }
    }
    let mut models: Vec<String> = catalog
        .models()
        .filter(|(owner, _, _)| *owner == wanted)
        .map(|(_, model, _)| model.to_owned())
        .collect();
    models.sort();
    let row = view::account_row(&account, !models.is_empty(), ctx.now, &ctx.zone);
    let page = DetailPage {
        frame: ctx.frame(Some(Tab::Accounts), account.name.clone()),
        path: segment(&account.name),
        provider: provider_title(account.kind.provider()),
        kind: kind_label(account.kind),
        added: Stamp::new(account.created_at, ctx.now, &ctx.zone),
        state: row.state,
        windows: row.windows,
        read: row.read,
        charts,
        facts,
        sessions,
        errors,
        models,
        sign_in: sign_in(account.kind),
        usage: can_query_usage(account.kind),
        billing: row.billing,
        name: account.name,
    };
    html(StatusCode::OK, &page)
}

fn short_id(session: &str) -> String {
    session.chars().take(8).collect()
}

fn new_page(
    ctx: &Ctx,
    kind: Kind,
    form: &NewForm,
    error: Option<Problem>,
    status: StatusCode,
) -> Response {
    let custom = kind.provider() == Provider::Custom;
    let (label, hint) = method_text(kind.provider(), kind.method());
    html(
        status,
        &NewPage {
            frame: ctx.frame(Some(Tab::Accounts), "Add account"),
            heading: format!("{} {}", provider_title(kind.provider()), label),
            hint: format::capitalize(hint),
            action: format!("/admin/accounts/new/{}/{}", kind.provider(), kind.method()),
            name: form.name.clone(),
            secret_label: (custom || ops::account::needs_paste(kind)).then(|| secret_label(kind)),
            secret_hint: secret_hint(kind),
            custom,
            url: form.url.clone(),
            api: form.api.clone(),
            apis: APIS,
            flow: !custom && !ops::account::needs_paste(kind),
            error,
        },
    )
}

fn parse_kind(provider: &str, method: &str) -> Option<Kind> {
    Kind::new(provider.parse().ok()?, method.parse().ok()?).ok()
}

fn sign_in(kind: Kind) -> SignIn {
    match kind.method() {
        Method::Oauth | Method::Siwc | Method::Codex => SignIn::Flow,
        Method::SetupToken | Method::Plan | Method::Key => SignIn::Paste(secret_label(kind)),
        Method::Custom => SignIn::None,
    }
}

fn secret_label(kind: Kind) -> &'static str {
    match kind.method() {
        Method::SetupToken => "Token from claude setup-token",
        Method::Custom => "Key (optional)",
        Method::Oauth | Method::Siwc | Method::Codex | Method::Plan | Method::Key => "Key",
    }
}

fn secret_hint(kind: Kind) -> &'static str {
    match kind.method() {
        Method::SetupToken => {
            "Run claude setup-token on any machine with Claude Code and paste what it prints."
        }
        Method::Custom => "Only if the endpoint asks for one. It is sent as a bearer token.",
        Method::Oauth | Method::Siwc | Method::Codex | Method::Plan | Method::Key => {
            "Stored on the gw server as you enter it and never shown again."
        }
    }
}

fn account_path(name: &str) -> String {
    format!("/account/{}", segment(name))
}
