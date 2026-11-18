use askama::Template;
use axum::Form;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse as _, Response};
use serde::Deserialize;
use url::Url;

use super::login::{self, Target};
use super::pages::{AccountRow, Link, Stamp, Tab, account_row, nav};
use super::{Web, failure, html, see_other};
use crate::account::Secret;
use crate::clock;
use crate::display::{PROVIDERS, kind_label, provider_title};
use crate::ops;
use crate::provider::{Billing, Kind, Method, Provider, can_query_usage};

const APIS: [&str; 3] = ["messages", "responses", "chat"];

#[derive(Template)]
#[template(path = "admin/account_choose.html")]
struct ChoosePage {
    title: &'static str,
    tabs: Vec<Link>,
    groups: Vec<KindGroup>,
}

struct KindGroup {
    title: &'static str,
    options: Vec<KindOption>,
}

struct KindOption {
    label: &'static str,
    billing: &'static str,
    path: String,
}

#[derive(Template)]
#[template(path = "admin/account_new.html")]
struct NewPage {
    title: &'static str,
    tabs: Vec<Link>,
    heading: String,
    action: String,
    name: String,
    secret_label: Option<&'static str>,
    custom: bool,
    url: String,
    api: String,
    apis: [&'static str; 3],
    error: Option<String>,
}

#[derive(Template)]
#[template(path = "admin/account.html")]
struct DetailPage {
    title: &'static str,
    tabs: Vec<Link>,
    row: AccountRow,
    provider: &'static str,
    method: &'static str,
    added: Stamp,
    stats: Vec<(&'static str, String)>,
    errors: Vec<(Stamp, String)>,
    sign_in: SignIn,
    usage: bool,
    error: Option<String>,
}

enum SignIn {
    Flow,
    Paste(&'static str),
    None,
}

#[derive(Template)]
#[template(path = "admin/result.html")]
struct ResultPage {
    title: &'static str,
    tabs: Vec<Link>,
    heading: &'static str,
    lines: Vec<String>,
    back: &'static str,
    back_label: &'static str,
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

pub(super) async fn choose() -> Response {
    let groups = PROVIDERS
        .into_iter()
        .map(|provider| KindGroup {
            title: provider_title(provider),
            options: provider
                .methods()
                .iter()
                .filter_map(|method| Kind::new(provider, *method).ok())
                .map(|kind| KindOption {
                    label: kind_label(kind),
                    billing: match (provider, kind.billing()) {
                        (Provider::Custom, _) => "",
                        (_, Billing::Sub) => "subscription",
                        (_, Billing::Api) => "pay as you go",
                    },
                    path: format!("{provider}/{}", kind.method()),
                })
                .collect(),
        })
        .collect();
    html(
        StatusCode::OK,
        &ChoosePage {
            title: "Add account",
            tabs: nav(Tab::Accounts),
            groups,
        },
    )
}

pub(super) async fn new_form(
    State(web): State<Web>,
    Path((provider, method)): Path<(String, String)>,
) -> Response {
    let Some(kind) = parse_kind(&provider, &method) else {
        return StatusCode::NOT_FOUND.into_response();
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
    new_page(kind, &form, None, StatusCode::OK)
}

pub(super) async fn create(
    State(web): State<Web>,
    Path((provider, method)): Path<(String, String)>,
    Form(form): Form<NewForm>,
) -> Response {
    let Some(kind) = parse_kind(&provider, &method) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let invalid =
        |message: String| new_page(kind, &form, Some(message), StatusCode::UNPROCESSABLE_ENTITY);
    let name = form.name.trim().to_owned();
    let taken: Vec<String> = match ops::account::list(&web.0.home).await {
        Ok(accounts) => accounts.into_iter().map(|account| account.name).collect(),
        Err(error) => return failure(&error),
    };
    if let Err(error) = ops::name::check(&name, &taken) {
        return invalid(crate::format::capitalize(&error.to_string()));
    }
    let secret = if kind.provider() == Provider::Custom {
        let url = form.url.trim();
        if !Url::parse(url).is_ok_and(|url| matches!(url.scheme(), "http" | "https")) {
            return invalid("Enter an http or https URL.".to_owned());
        }
        if crate::forward::parse_api(&form.api).is_none() {
            return invalid("Choose the API the endpoint speaks.".to_owned());
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
            return invalid(format!("Paste the {}.", secret_label(kind).to_lowercase()));
        }
        ops::account::pasted_secret(kind, value.to_owned(), clock::now())
    } else {
        return match login::start(&web, kind, Target::New(name)).await {
            Ok(id) => see_other(&format!("/logins/{id}")),
            Err(error) => invalid(format!("Could not start the sign-in: {error:#}")),
        };
    };
    if let Err(error) = ops::account::add(&web.0.home, name.clone(), kind, secret).await {
        return invalid(format!("{error:#}"));
    }
    let message = added(&web, &name).await;
    html(
        StatusCode::OK,
        &ResultPage {
            title: "Account added",
            tabs: nav(Tab::Accounts),
            heading: "Account added",
            lines: vec![message],
            back: "/admin/accounts",
            back_label: "Back to accounts",
        },
    )
}

// A new account is useful only once gw knows its models, so adding one fetches them right away,
// as the CLI and the TUI do.
pub(super) async fn added(web: &Web, name: &str) -> String {
    match ops::model::refresh(&web.0.home, &web.0.http, Some(name)).await {
        Ok(report) => match report.into_iter().next().map(|refreshed| refreshed.result) {
            Some(Ok(count)) => format!("Added {name} with {count} models."),
            Some(Err(error)) => format!("Added {name}, but its model list failed: {error}"),
            None => format!("Added {name}."),
        },
        Err(error) => format!("Added {name}, but its model list failed: {error:#}"),
    }
}

pub(super) async fn detail(State(web): State<Web>, Path(name): Path<String>) -> Response {
    detail_page(&web, &name, None).await
}

pub(super) async fn sign_in_again(
    State(web): State<Web>,
    Path(name): Path<String>,
    Form(form): Form<SecretForm>,
) -> Response {
    let Ok(account) = ops::account::find(&web.0.home, &name).await else {
        return see_other("/accounts");
    };
    match sign_in(account.kind) {
        SignIn::Flow => match login::start(&web, account.kind, Target::Again(name.clone())).await {
            Ok(id) => see_other(&format!("/logins/{id}")),
            Err(error) => {
                detail_page(
                    &web,
                    &name,
                    Some(format!("Could not start the sign-in: {error:#}")),
                )
                .await
            }
        },
        SignIn::Paste(label) => {
            let value = form.secret.trim();
            if value.is_empty() {
                return detail_page(
                    &web,
                    &name,
                    Some(format!("Paste the {}.", label.to_lowercase())),
                )
                .await;
            }
            let secret = ops::account::pasted_secret(account.kind, value.to_owned(), clock::now());
            match ops::account::relogin(&web.0.home, &name, secret).await {
                Ok(()) => see_other(&account_path(&name)),
                Err(error) => detail_page(&web, &name, Some(format!("{error:#}"))).await,
            }
        }
        SignIn::None => see_other(&account_path(&name)),
    }
}

pub(super) async fn refresh_usage(State(web): State<Web>, Path(name): Path<String>) -> Response {
    match ops::account::refresh_usage(&web.0.home, &web.0.http, &name).await {
        Ok(()) => see_other(&account_path(&name)),
        Err(error) => {
            detail_page(
                &web,
                &name,
                Some(format!("Usage refresh failed: {error:#}")),
            )
            .await
        }
    }
}

pub(super) async fn rename(
    State(web): State<Web>,
    Path(name): Path<String>,
    Form(form): Form<RenameForm>,
) -> Response {
    let new_name = form.new_name.trim();
    match ops::account::rename(&web.0.home, &name, new_name).await {
        Ok(()) => see_other(&account_path(new_name)),
        Err(error) => {
            detail_page(
                &web,
                &name,
                Some(crate::format::capitalize(&format!("{error:#}"))),
            )
            .await
        }
    }
}

pub(super) async fn remove(
    State(web): State<Web>,
    Path(name): Path<String>,
    Form(form): Form<ConfirmForm>,
) -> Response {
    if form.confirm.trim() != name {
        return detail_page(&web, &name, Some(format!("Type {name} to remove it."))).await;
    }
    match ops::account::remove(&web.0.home, &name).await {
        Ok(()) => see_other("/accounts"),
        Err(error) => detail_page(&web, &name, Some(format!("{error:#}"))).await,
    }
}

pub(super) async fn refresh_models(State(web): State<Web>) -> Response {
    let lines = match ops::model::refresh(&web.0.home, &web.0.http, None).await {
        Ok(report) if report.is_empty() => vec!["There are no accounts to refresh.".to_owned()],
        Ok(report) => report
            .into_iter()
            .map(|refreshed| match refreshed.result {
                Ok(count) => format!("{}: {count} models", refreshed.account),
                Err(error) => format!("{}: failed, {error}", refreshed.account),
            })
            .collect(),
        Err(error) => vec![format!("Refresh failed: {error:#}")],
    };
    html(
        StatusCode::OK,
        &ResultPage {
            title: "Models refreshed",
            tabs: nav(Tab::Models),
            heading: "Models refreshed",
            lines,
            back: "/admin/models",
            back_label: "Back to models",
        },
    )
}

async fn detail_page(web: &Web, name: &str, error: Option<String>) -> Response {
    let Ok(account) = ops::account::find(&web.0.home, name).await else {
        return see_other("/accounts");
    };
    let detail = match ops::account::detail(&web.0.home, name).await {
        Ok(detail) => detail,
        Err(error) => return failure(&error),
    };
    let now = clock::now();
    let status = if error.is_some() {
        StatusCode::UNPROCESSABLE_ENTITY
    } else {
        StatusCode::OK
    };
    let page = DetailPage {
        title: "Account",
        tabs: nav(Tab::Accounts),
        provider: provider_title(account.kind.provider()),
        method: account.kind.method().as_str(),
        added: Stamp::ago(Some(account.created_at), now),
        stats: vec![
            ("Models", detail.models.to_string()),
            ("Requests in 24 hours", detail.requests_24h.to_string()),
            (
                "Tokens in 5 hours",
                format!(
                    "{} in ({} cached), {} out",
                    crate::format::compact(detail.input_5h),
                    crate::format::compact(detail.cache_5h),
                    crate::format::compact(detail.output_5h)
                ),
            ),
        ],
        errors: detail
            .errors
            .into_iter()
            .map(|(at, message)| (Stamp::ago(Some(at), now), message))
            .collect(),
        sign_in: sign_in(account.kind),
        usage: can_query_usage(account.kind),
        row: account_row(&account, now),
        error,
    };
    html(status, &page)
}

fn new_page(kind: Kind, form: &NewForm, error: Option<String>, status: StatusCode) -> Response {
    let custom = kind.provider() == Provider::Custom;
    html(
        status,
        &NewPage {
            title: "Add account",
            tabs: nav(Tab::Accounts),
            heading: format!("{} · {}", provider_title(kind.provider()), kind_label(kind)),
            action: format!("/admin/accounts/new/{}/{}", kind.provider(), kind.method()),
            name: form.name.clone(),
            secret_label: (custom || ops::account::needs_paste(kind)).then(|| secret_label(kind)),
            custom,
            url: form.url.clone(),
            api: form.api.clone(),
            apis: APIS,
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

fn account_path(name: &str) -> String {
    format!("/account/{}", super::segment(name))
}
