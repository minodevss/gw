use askama::Template;
use axum::Form;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse as _, Response};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;

use super::{PREFIX, Web, failure, html, see_other};
use crate::clock;
use crate::ops;
use crate::ops::key::hash;

const COOKIE: &str = "gw_admin";
const SESSION_SECS: i64 = 7 * 86_400;

pub(super) struct Session {
    created_at: i64,
    token_hash: String,
}

#[derive(Template)]
#[template(path = "admin/login.html")]
struct LoginPage {
    has_token: bool,
    error: Option<&'static str>,
}

#[derive(Deserialize)]
pub(super) struct LoginForm {
    token: String,
}

pub(super) async fn require(State(web): State<Web>, request: Request, next: Next) -> Response {
    let token = match current_token(&web).await {
        Ok(token) => token,
        Err(error) => return failure(&error),
    };
    let signed_in = token.is_some_and(|token| {
        session_id(request.headers()).is_some_and(|id| web.valid(&id, &hash(&token)))
    });
    if signed_in {
        return next.run(request).await;
    }
    // htmx 4 swaps every response into the page, so an expired session on a refresh must turn into
    // a full navigation instead of a login form inside the table. HX-Redirect needs a 2xx status.
    // https://github.com/bigskysoftware/htmx/blob/v4.0.2/www/src/content/docs/whats-new-in-htmx-4.md
    // https://github.com/bigskysoftware/htmx/blob/v4.0.2/www/src/content/reference/02-headers/12-HX-Redirect.md
    if request.headers().contains_key("hx-request") {
        return (StatusCode::OK, [("hx-redirect", format!("{PREFIX}/login"))]).into_response();
    }
    see_other("/login")
}

pub(super) async fn login_page(State(web): State<Web>) -> Response {
    match current_token(&web).await {
        Ok(token) => html(
            StatusCode::OK,
            &LoginPage {
                has_token: token.is_some(),
                error: None,
            },
        ),
        Err(error) => failure(&error),
    }
}

pub(super) async fn login(
    State(web): State<Web>,
    headers: HeaderMap,
    Form(form): Form<LoginForm>,
) -> Response {
    let token = match current_token(&web).await {
        Ok(token) => token,
        Err(error) => return failure(&error),
    };
    let Some(token) = token else {
        return html(
            StatusCode::OK,
            &LoginPage {
                has_token: false,
                error: None,
            },
        );
    };
    let token_hash = hash(&token);
    if hash(form.token.trim()) != token_hash {
        return html(
            StatusCode::UNAUTHORIZED,
            &LoginPage {
                has_token: true,
                error: Some("That token is not right."),
            },
        );
    }
    let mut bytes = [0_u8; 32];
    if let Err(error) = getrandom::fill(&mut bytes) {
        return failure(&error);
    }
    let id = URL_SAFE_NO_PAD.encode(bytes);
    web.start(&id, token_hash);
    let cookie = format!(
        "{COOKIE}={id}; Path={PREFIX}; HttpOnly; SameSite=Strict; Max-Age={SESSION_SECS}{}",
        secure(&headers)
    );
    with_cookie(see_other("/accounts"), &cookie)
}

pub(super) async fn logout(State(web): State<Web>, headers: HeaderMap) -> Response {
    if let Some(id) = session_id(&headers) {
        web.end(&id);
    }
    let cookie = format!(
        "{COOKIE}=; Path={PREFIX}; HttpOnly; SameSite=Strict; Max-Age=0{}",
        secure(&headers)
    );
    with_cookie(see_other("/login"), &cookie)
}

impl Web {
    fn start(&self, id: &str, token_hash: String) {
        let now = clock::now();
        let mut sessions = self.lock();
        sessions.retain(|_, session| now - session.created_at < SESSION_SECS);
        sessions.insert(
            hash(id),
            Session {
                created_at: now,
                token_hash,
            },
        );
    }

    fn valid(&self, id: &str, token_hash: &str) -> bool {
        let now = clock::now();
        self.lock().get(&hash(id)).is_some_and(|session| {
            session.token_hash == token_hash && now - session.created_at < SESSION_SECS
        })
    }

    fn end(&self, id: &str) {
        self.lock().remove(&hash(id));
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, std::collections::HashMap<String, Session>> {
        // A panic while holding the lock leaves the map itself intact, so keep using it.
        self.0
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

async fn current_token(web: &Web) -> anyhow::Result<Option<String>> {
    ops::with_store(&web.0.home, |store| ops::admin::current(store)).await
}

fn session_id(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .find_map(|pair| pair.trim().strip_prefix(COOKIE)?.strip_prefix('='))
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
}

// gw has no TLS of its own; a TLS-terminating proxy says so with X-Forwarded-Proto. Plain HTTP over
// a private network such as Tailscale must keep working, so Secure is not always set.
fn secure(headers: &HeaderMap) -> &'static str {
    let https = headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|proto| proto.eq_ignore_ascii_case("https"));
    if https { "; Secure" } else { "" }
}

fn with_cookie(mut response: Response, cookie: &str) -> Response {
    match HeaderValue::from_str(cookie) {
        Ok(value) => {
            response.headers_mut().insert(header::SET_COOKIE, value);
            response
        }
        Err(error) => failure(&error),
    }
}
