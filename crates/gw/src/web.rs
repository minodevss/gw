mod auth;
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
    sessions: Mutex<HashMap<String, auth::Session>>,
}

pub(crate) fn router(home: PathBuf) -> Router {
    let web = Web(Arc::new(Inner {
        home,
        sessions: Mutex::default(),
    }));
    let signed_in = Router::new()
        .route("/", get(pages::home))
        .route("/{tab}", get(pages::page))
        .route("/{tab}/live", get(pages::live))
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

async fn asset(Path(name): Path<String>) -> Response {
    let (body, kind) = match name.as_str() {
        "htmx.min.js" => (HTMX, "text/javascript; charset=utf-8"),
        "admin.css" => (CSS, "text/css; charset=utf-8"),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    (
        [
            (header::CONTENT_TYPE, kind),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        Body::from(body),
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

fn see_other(path: &str) -> Response {
    let location = format!("{PREFIX}{path}");
    (StatusCode::SEE_OTHER, [(header::LOCATION, location)]).into_response()
}

#[cfg(test)]
mod tests;
