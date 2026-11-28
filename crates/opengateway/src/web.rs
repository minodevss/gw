mod accounts;
mod activity;
mod auth;
mod chart;
mod frame;
mod keys;
mod login;
mod models;
mod overview;
mod stats;
mod traffic;
mod usage;
mod view;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use askama::Template;
use axum::Router;
use axum::body::Body;
use axum::extract::{Path, Request};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse as _, Response};
use axum::routing::{get, post};
use url::Url;

use frame::{Ctx, Frame};

const PREFIX: &str = "/admin";
const CSP: &str = "default-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'";
const HTMX: &[u8] = include_bytes!("../assets/htmx.min.js");
const CSS: &[u8] = include_bytes!("../assets/admin.css");
const JS: &[u8] = include_bytes!("../assets/admin.js");

#[derive(Clone)]
pub(crate) struct Web(Arc<Inner>);

struct Inner {
    home: PathBuf,
    http: reqwest::Client,
    sessions: Mutex<HashMap<String, auth::Session>>,
    logins: Mutex<HashMap<String, login::Login>>,
    issued: Mutex<HashMap<String, keys::Issued>>,
}

pub(crate) fn router(home: PathBuf, http: reqwest::Client) -> Router {
    let web = Web(Arc::new(Inner {
        home,
        http,
        sessions: Mutex::default(),
        logins: Mutex::default(),
        issued: Mutex::default(),
    }));
    let signed_in = Router::new()
        .route("/", get(start_page))
        .route("/overview", get(overview::page))
        .route("/live/overview", get(overview::live))
        .route("/usage", get(usage::page))
        .route("/live/usage", get(usage::live))
        .route("/traffic", get(traffic::page))
        .route("/live/traffic", get(traffic::live))
        .route("/activity", get(activity::page))
        .route("/live/activity", get(activity::live))
        .route("/request/{id}", get(activity::request))
        .route("/session/{id}", get(activity::session))
        .route("/accounts", get(accounts::page))
        .route("/live/accounts", get(accounts::live))
        .route("/accounts/new", get(accounts::choose))
        .route(
            "/accounts/new/{provider}/{method}",
            get(accounts::new_form).post(accounts::create),
        )
        .route("/account/{name}", get(accounts::detail))
        .route("/account/{name}/login", post(accounts::sign_in_again))
        .route("/account/{name}/usage", post(accounts::refresh_usage))
        .route(
            "/account/{name}/models",
            post(accounts::refresh_account_models),
        )
        .route("/account/{name}/rename", post(accounts::rename))
        .route("/account/{name}/remove", post(accounts::remove))
        .route("/models", get(models::page))
        .route("/live/models", get(models::live))
        .route("/models/refresh", post(models::refresh))
        .route("/logins/{id}", get(login::page).post(login::paste))
        .route("/logins/{id}/state", get(login::state))
        .route("/logins/{id}/cancel", post(login::cancel))
        .route("/logins/{id}/retry", post(login::retry))
        .route("/keys", get(keys::page))
        .route("/live/keys", get(keys::live))
        .route("/keys/new", get(keys::new_form).post(keys::create))
        .route("/keys/issued/{id}", get(keys::issued))
        .route("/keys/issued/{id}/done", post(keys::done))
        .route("/key/{name}", get(keys::confirm_revoke))
        .route("/key/{name}/revoke", post(keys::revoke))
        .route("/logout", post(auth::logout))
        .fallback(not_found)
        .route_layer(middleware::from_fn_with_state(web.clone(), auth::require));
    Router::new()
        .merge(signed_in)
        .route("/login", get(auth::login_page).post(auth::login))
        .route("/assets/{name}", get(asset))
        .layer(middleware::from_fn(flash_once))
        .layer(middleware::from_fn(guard))
        .with_state(web)
}

async fn start_page() -> Response {
    see_other("/overview")
}

#[derive(Template)]
#[template(path = "admin/missing.html")]
struct Missing {
    frame: Frame,
}

async fn not_found(ctx: Ctx) -> Response {
    missing(&ctx)
}

fn missing(ctx: &Ctx) -> Response {
    html(
        StatusCode::NOT_FOUND,
        &Missing {
            frame: ctx.frame(None, "Not found"),
        },
    )
}

async fn flash_once(request: Request, next: Next) -> Response {
    let had_flash = frame::cookie(request.headers(), frame::FLASH_COOKIE).is_some();
    let htmx = request.headers().contains_key("hx-request");
    let https = request
        .headers()
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|proto| proto.eq_ignore_ascii_case("https"));
    let mut response = next.run(request).await;
    if had_flash && !htmx && response.status().is_success() {
        frame::clear_flash(&mut response);
    }
    if https {
        frame::secure_flash(&mut response);
    }
    response
}

async fn guard(request: Request, next: Next) -> Response {
    let mut response = if cross_origin(request.method(), request.headers()) {
        (StatusCode::FORBIDDEN, "cross-origin request refused").into_response()
    } else {
        next.run(request).await
    };
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers
        .entry(header::CACHE_CONTROL)
        .or_insert(HeaderValue::from_static("no-store"));
    response
}

// Same rule as Go's http.CrossOriginProtection: trust Sec-Fetch-Site when the browser sends it,
// otherwise compare Origin with Host, and let requests with neither through as non-browser clients.
// https://pkg.go.dev/net/http#CrossOriginProtection
fn cross_origin(method: &Method, headers: &HeaderMap) -> bool {
    if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
        return false;
    }
    let text = |name| {
        headers
            .get(name)
            .and_then(|v: &HeaderValue| v.to_str().ok())
    };
    if let Some(site) = text(header::HeaderName::from_static("sec-fetch-site")) {
        return !matches!(site, "same-origin" | "none");
    }
    let Some(origin) = text(header::ORIGIN) else {
        return false;
    };
    let origin_host = Url::parse(origin).ok().and_then(|url| {
        let host = url.host_str()?.to_owned();
        Some(match url.port() {
            Some(port) => format!("{host}:{port}"),
            None => host,
        })
    });
    origin_host.as_deref() != text(header::HOST)
}

// Assets change only with a new OpenGateway build, but under the same URL. no-cache with an ETag makes the
// browser check on every load and costs a 304 when nothing changed, so an upgrade never shows a
// stale stylesheet.
async fn asset(Path(name): Path<String>, headers: HeaderMap) -> Response {
    let (body, kind) = match name.as_str() {
        "htmx.min.js" => (HTMX, "text/javascript; charset=utf-8"),
        "admin.css" => (CSS, "text/css; charset=utf-8"),
        "admin.js" => (JS, "text/javascript; charset=utf-8"),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    let etag = format!("\"{}\"", crate::ops::key::hash_bytes(body));
    let cached = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == etag);
    let status = if cached {
        StatusCode::NOT_MODIFIED
    } else {
        StatusCode::OK
    };
    let body = if cached {
        Body::empty()
    } else {
        Body::from(body)
    };
    (
        status,
        [
            (header::CONTENT_TYPE, kind.to_owned()),
            (header::CACHE_CONTROL, "no-cache".to_owned()),
            (header::ETAG, etag),
        ],
        body,
    )
        .into_response()
}

fn html(status: StatusCode, page: &impl Template) -> Response {
    match page.render() {
        Ok(body) => (
            status,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            body,
        )
            .into_response(),
        Err(error) => failure(&error),
    }
}

// htmx 4 swaps every response except 204 and 304, so a live refresh that fails returns 204 to
// keep the last good render, and polling carries on.
// https://github.com/bigskysoftware/htmx/blob/v4.0.2/www/src/content/docs/whats-new-in-htmx-4.md
fn fragment(rendered: anyhow::Result<String>) -> Response {
    match rendered {
        Ok(body) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            body,
        )
            .into_response(),
        Err(error) => {
            tracing::warn!(%error, "admin live refresh failed");
            StatusCode::NO_CONTENT.into_response()
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic while holding the lock leaves the map itself intact, so keep using it.
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn redirect_with(path: &str, flash: &frame::Flash) -> Response {
    frame::with_flash(see_other(path), flash)
}

fn failure(error: &dyn std::fmt::Display) -> Response {
    tracing::warn!(%error, "admin page failed");
    (StatusCode::INTERNAL_SERVER_ERROR, "admin page failed").into_response()
}

// The address this browser used to reach OpenGateway, which is usually the one clients use too.
fn origin(headers: &HeaderMap) -> Option<String> {
    let host = headers.get(header::HOST)?.to_str().ok()?;
    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .filter(|proto| proto.eq_ignore_ascii_case("https"))
        .map_or("http", |_| "https");
    Some(format!("{proto}://{host}"))
}

// Percent-encodes one path segment. Account and key names made through the CLI are not limited
// to URL-safe characters.
fn segment(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                char::from(byte).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

fn see_other(path: &str) -> Response {
    let location = format!("{PREFIX}{path}");
    (StatusCode::SEE_OTHER, [(header::LOCATION, location)]).into_response()
}

#[cfg(test)]
mod tests;
