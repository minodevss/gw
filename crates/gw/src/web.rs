mod accounts;
mod auth;
mod keys;
mod login;
mod pages;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use askama::Template;
use axum::Router;
use axum::body::Body;
use axum::extract::{Path, Request};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse as _, Response};
use axum::routing::{get, post};
use url::Url;

const PREFIX: &str = "/admin";
const CSP: &str = "default-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'";
const HTMX: &[u8] = include_bytes!("../assets/htmx.min.js");
const CSS: &[u8] = include_bytes!("../assets/admin.css");

#[derive(Clone)]
pub(crate) struct Web(Arc<Inner>);

struct Inner {
    home: PathBuf,
    http: reqwest::Client,
    sessions: Mutex<HashMap<String, auth::Session>>,
    logins: Mutex<HashMap<String, login::Login>>,
}

pub(crate) fn router(home: PathBuf, http: reqwest::Client) -> Router {
    let web = Web(Arc::new(Inner {
        home,
        http,
        sessions: Mutex::default(),
        logins: Mutex::default(),
    }));
    let signed_in = Router::new()
        .route("/", get(pages::home))
        .route("/{tab}", get(pages::page))
        .route("/live/{tab}", get(pages::live))
        .route("/accounts/new", get(accounts::choose))
        .route(
            "/accounts/new/{provider}/{method}",
            get(accounts::new_form).post(accounts::create),
        )
        .route("/account/{name}", get(accounts::detail))
        .route("/account/{name}/login", post(accounts::sign_in_again))
        .route("/account/{name}/usage", post(accounts::refresh_usage))
        .route("/account/{name}/rename", post(accounts::rename))
        .route("/account/{name}/remove", post(accounts::remove))
        .route("/models/refresh", post(accounts::refresh_models))
        .route("/logins/{id}", get(login::page).post(login::paste))
        .route("/logins/{id}/state", get(login::state))
        .route("/keys/new", get(keys::new_form).post(keys::create))
        .route("/key/{name}", get(keys::confirm_revoke))
        .route("/key/{name}/revoke", post(keys::revoke))
        .route("/logout", post(auth::logout))
        .route_layer(middleware::from_fn_with_state(web.clone(), auth::require));
    Router::new()
        .merge(signed_in)
        .route("/login", get(auth::login_page).post(auth::login))
        .route("/assets/{name}", get(asset))
        .layer(middleware::from_fn(guard))
        .with_state(web)
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

// Assets change only with a new gw build, but under the same URL. no-cache with an ETag makes the
// browser check on every load and costs a 304 when nothing changed, so an upgrade never shows a
// stale stylesheet.
async fn asset(Path(name): Path<String>, headers: HeaderMap) -> Response {
    let (body, kind) = match name.as_str() {
        "htmx.min.js" => (HTMX, "text/javascript; charset=utf-8"),
        "admin.css" => (CSS, "text/css; charset=utf-8"),
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

fn failure(error: &dyn std::fmt::Display) -> Response {
    tracing::warn!(%error, "admin page failed");
    (StatusCode::INTERNAL_SERVER_ERROR, "admin page failed").into_response()
}

// The address this browser used to reach gw, which is usually the one clients use too.
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
