use std::path::PathBuf;

use axum::body::{Body, to_bytes};
use axum::http::request::Builder;
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use tower::ServiceExt as _;

use crate::ops;
use crate::store::Store;

struct Home(PathBuf);

impl Home {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("opengateway-web-{name}-{}", std::process::id()));
        Store::open(&crate::home::database(&dir).unwrap()).unwrap();
        Self(dir)
    }

    fn token(&self, rotate: bool) -> String {
        let store = Store::open(&crate::home::database(&self.0).unwrap()).unwrap();
        ops::admin::token(&store, rotate).unwrap()
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap_or_default();
    }
}

async fn send(router: &axum::Router, request: Request<Body>) -> Response {
    router.clone().oneshot(request).await.unwrap()
}

fn get(path: &str) -> Builder {
    Request::get(path)
}

fn post_form(path: &str, body: &str) -> Request<Body> {
    Request::post(path)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header("sec-fetch-site", "same-origin")
        .body(Body::from(body.to_owned()))
        .unwrap()
}

async fn text(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

fn location(response: &Response) -> &str {
    response.headers()[header::LOCATION].to_str().unwrap()
}

async fn sign_in(router: &axum::Router, token: &str) -> String {
    let response = send(router, post_form("/admin/login", &format!("token={token}"))).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
    cookie.split(';').next().unwrap().to_owned()
}

fn app(home: &Home) -> axum::Router {
    axum::Router::new().nest(
        "/admin",
        super::router(home.0.clone(), reqwest::Client::new()),
    )
}

#[tokio::test]
async fn without_a_token_every_page_leads_to_the_setup_hint() {
    let home = Home::new("setup");
    let router = app(&home);
    let response = send(&router, get("/admin/accounts").body(Body::empty()).unwrap()).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&response), "/admin/login?next=%2Fadmin%2Faccounts");
    let page = text(send(&router, get("/admin/login").body(Body::empty()).unwrap()).await).await;
    assert!(page.contains("opengateway admin token"));
    assert!(!page.contains("<form"));
}

#[tokio::test]
async fn signing_in_opens_the_pages_and_a_wrong_token_does_not() {
    let home = Home::new("signin");
    let token = home.token(false);
    let router = app(&home);

    let wrong = send(&router, post_form("/admin/login", "token=gwa_nope")).await;
    assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
    assert!(!wrong.headers().contains_key(header::SET_COOKIE));

    let response = send(
        &router,
        post_form("/admin/login", &format!("token={token}")),
    )
    .await;
    assert_eq!(location(&response), "/admin/overview");
    let set_cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
    for part in ["Path=/admin", "HttpOnly", "SameSite=Strict"] {
        assert!(set_cookie.contains(part), "{set_cookie}");
    }
    assert!(!set_cookie.contains("Secure"));
    let cookie = set_cookie.split(';').next().unwrap();

    for tab in [
        "overview", "usage", "traffic", "activity", "accounts", "models", "keys",
    ] {
        let page = send(
            &router,
            get(&format!("/admin/{tab}"))
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(page.status(), StatusCode::OK, "{tab}");
        let body = text(page).await;
        assert!(
            body.contains(&format!("hx-get=\"/admin/live/{tab}")),
            "{tab}"
        );
    }
    let missing = send(
        &router,
        get("/admin/nope")
            .header(header::COOKIE, cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert!(
        text(missing)
            .await
            .contains("There is no page at this address")
    );
}

#[tokio::test]
async fn secure_cookie_behind_https_proxy() {
    let home = Home::new("secure");
    let token = home.token(false);
    let request = Request::post("/admin/login")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header("x-forwarded-proto", "https")
        .body(Body::from(format!("token={token}")))
        .unwrap();
    let response = send(&app(&home), request).await;
    let set_cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
    assert!(set_cookie.ends_with("; Secure"), "{set_cookie}");
}

#[tokio::test]
async fn rotating_the_token_ends_sessions() {
    let home = Home::new("rotate");
    let token = home.token(false);
    let router = app(&home);
    let cookie = sign_in(&router, &token).await;
    home.token(true);
    let response = send(
        &router,
        get("/admin/accounts")
            .header(header::COOKIE, &cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn an_expired_session_on_a_live_refresh_redirects_the_whole_page() {
    let home = Home::new("htmx");
    home.token(false);
    let response = send(
        &app(&home),
        get("/admin/live/accounts")
            .header("hx-request", "true")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["hx-redirect"], "/admin/login");
}

#[tokio::test]
async fn signing_out_ends_the_session() {
    let home = Home::new("logout");
    let token = home.token(false);
    let router = app(&home);
    let cookie = sign_in(&router, &token).await;
    let mut request = post_form("/admin/logout", "");
    request
        .headers_mut()
        .insert(header::COOKIE, cookie.parse().unwrap());
    let response = send(&router, request).await;
    assert_eq!(location(&response), "/admin/login");
    assert!(
        response.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .contains("Max-Age=0")
    );
    let after = send(
        &router,
        get("/admin/accounts")
            .header(header::COOKIE, &cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(after.status(), StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn cross_origin_posts_are_refused() {
    let home = Home::new("csrf");
    let token = home.token(false);
    let router = app(&home);
    let form = |site: Option<&str>, origin: Option<&str>| {
        let mut request = Request::post("/admin/login")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header(header::HOST, "gateway.example:8787");
        if let Some(site) = site {
            request = request.header("sec-fetch-site", site);
        }
        if let Some(origin) = origin {
            request = request.header(header::ORIGIN, origin);
        }
        request.body(Body::from(format!("token={token}"))).unwrap()
    };
    for (site, origin) in [
        (Some("cross-site"), None),
        (Some("same-site"), None),
        (None, Some("http://evil.example")),
        (None, Some("null")),
    ] {
        let response = send(&router, form(site, origin)).await;
        assert_eq!(
            response.status(),
            StatusCode::FORBIDDEN,
            "{site:?} {origin:?}"
        );
    }
    for (site, origin) in [
        (Some("same-origin"), None),
        (None, Some("http://gateway.example:8787")),
        (None, None),
    ] {
        let response = send(&router, form(site, origin)).await;
        assert_eq!(
            response.status(),
            StatusCode::SEE_OTHER,
            "{site:?} {origin:?}"
        );
    }
}

#[tokio::test]
async fn every_response_carries_the_security_headers() {
    let home = Home::new("headers");
    let router = app(&home);
    for path in ["/admin/login", "/admin/accounts", "/admin/assets/admin.css"] {
        let response = send(&router, get(path).body(Body::empty()).unwrap()).await;
        let headers = response.headers();
        assert!(
            headers[header::CONTENT_SECURITY_POLICY]
                .to_str()
                .unwrap()
                .starts_with("default-src 'self'"),
            "{path}"
        );
        assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff", "{path}");
    }
    let page = send(&router, get("/admin/login").body(Body::empty()).unwrap()).await;
    assert_eq!(page.headers()[header::CACHE_CONTROL], "no-store");
    let asset = send(
        &router,
        get("/admin/assets/htmx.min.js")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(asset.status(), StatusCode::OK);
    assert_eq!(asset.headers()[header::CACHE_CONTROL], "no-cache");
    let etag = asset.headers()[header::ETAG].to_str().unwrap().to_owned();
    let again = send(
        &router,
        get("/admin/assets/htmx.min.js")
            .header(header::IF_NONE_MATCH, &etag)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(again.status(), StatusCode::NOT_MODIFIED);
}

async fn signed_in(name: &str) -> (Home, axum::Router, String) {
    let home = Home::new(name);
    let token = home.token(false);
    let router = app(&home);
    let cookie = sign_in(&router, &token).await;
    (home, router, cookie)
}

async fn post_as(router: &axum::Router, cookie: &str, path: &str, body: &str) -> Response {
    let mut request = post_form(path, body);
    request
        .headers_mut()
        .insert(header::COOKIE, cookie.parse().unwrap());
    send(router, request).await
}

async fn get_as(router: &axum::Router, cookie: &str, path: &str) -> Response {
    send(
        router,
        get(path)
            .header(header::COOKIE, cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await
}

fn flash_cookie(response: &Response) -> Option<String> {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with("opengateway_flash=") && !value.contains("Max-Age=0"))
        .and_then(|value| value.split(';').next())
        .map(str::to_owned)
}

async fn follow(router: &axum::Router, cookie: &str, response: &Response) -> String {
    let cookie = match flash_cookie(response) {
        Some(flash) => format!("{cookie}; {flash}"),
        None => cookie.to_owned(),
    };
    let landed = get_as(router, &cookie, location(response)).await;
    assert_eq!(landed.status(), StatusCode::OK);
    let clears = landed
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .any(|value| value.starts_with("opengateway_flash=;"));
    assert_eq!(clears, cookie.contains("opengateway_flash="));
    text(landed).await
}

fn accounts(home: &Home) -> Vec<String> {
    let store = Store::open(&crate::home::database(&home.0).unwrap()).unwrap();
    store
        .accounts()
        .unwrap()
        .into_iter()
        .map(|account| account.name)
        .collect()
}

#[tokio::test]
async fn adding_renaming_and_removing_a_custom_account() {
    let (home, router, cookie) = signed_in("account").await;
    let form = "name=local&url=http%3A%2F%2F127.0.0.1%3A9&api=chat&secret=";

    let bad = post_as(
        &router,
        &cookie,
        "/admin/accounts/new/custom/custom",
        "name=bad%20name&url=http%3A%2F%2F127.0.0.1%3A9&api=chat",
    )
    .await;
    assert_eq!(bad.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(text(bad).await.contains("Name: use letters, digits"));

    let added = post_as(&router, &cookie, "/admin/accounts/new/custom/custom", form).await;
    assert_eq!(location(&added), "/admin/account/local");
    let page = follow(&router, &cookie, &added).await;
    assert!(
        page.contains("Added local. Fetching its model list failed"),
        "{page}"
    );
    assert_eq!(accounts(&home), ["local"]);

    let again = post_as(&router, &cookie, "/admin/accounts/new/custom/custom", form).await;
    assert_eq!(again.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let page = text(get_as(&router, &cookie, "/admin/account/local").await).await;
    assert!(page.contains("Type <span class=\"mono\">local</span> to remove it"));
    assert!(!page.contains("Sign in again"));
    assert!(!page.contains("Added local"), "a flash shows only once");

    let renamed = post_as(
        &router,
        &cookie,
        "/admin/account/local/rename",
        "new_name=ollama",
    )
    .await;
    assert_eq!(location(&renamed), "/admin/account/ollama");
    assert!(
        follow(&router, &cookie, &renamed)
            .await
            .contains("Renamed local to ollama.")
    );
    assert_eq!(accounts(&home), ["ollama"]);

    let refused = post_as(
        &router,
        &cookie,
        "/admin/account/ollama/remove",
        "confirm=nope",
    )
    .await;
    assert_eq!(location(&refused), "/admin/account/ollama");
    assert!(
        follow(&router, &cookie, &refused)
            .await
            .contains("Not removed. The name you typed does not match ollama.")
    );
    assert_eq!(accounts(&home), ["ollama"]);
    let removed = post_as(
        &router,
        &cookie,
        "/admin/account/ollama/remove",
        "confirm=ollama",
    )
    .await;
    assert_eq!(location(&removed), "/admin/accounts");
    assert!(
        follow(&router, &cookie, &removed)
            .await
            .contains("Removed ollama.")
    );
    assert_eq!(accounts(&home), Vec::<String>::new());
}

#[tokio::test]
async fn a_paste_code_sign_in_runs_to_its_outcome() {
    let (home, router, cookie) = signed_in("flow").await;
    let started = post_as(
        &router,
        &cookie,
        "/admin/accounts/new/anthropic/oauth",
        "name=claude",
    )
    .await;
    assert_eq!(started.status(), StatusCode::SEE_OTHER);
    let page_path = location(&started).to_owned();
    assert!(page_path.starts_with("/admin/logins/"));

    let page = text(get_as(&router, &cookie, &page_path).await).await;
    assert!(page.contains("https://claude.com/"), "{page}");
    assert!(page.contains("name=\"value\""));
    assert!(page.contains("Waiting for you to finish signing in"));
    assert!(page.contains("/cancel"));

    let pasted = post_as(&router, &cookie, &page_path, "value=not-a-code").await;
    assert_eq!(location(&pasted), page_path);
    let state_path = format!("{page_path}/state");
    let mut state = String::new();
    for _ in 0..50 {
        state = text(get_as(&router, &cookie, &state_path).await).await;
        if !state.contains("hx-trigger") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(state.contains("Sign-in failed: "), "{state}");
    assert!(state.contains("code#state"), "{state}");
    assert!(state.contains("/retry"), "{state}");
    assert_eq!(accounts(&home), Vec::<String>::new());
    let after = text(get_as(&router, &cookie, &page_path).await).await;
    assert!(!after.contains("name=\"value\""));
}

#[tokio::test]
async fn creating_and_revoking_a_key() {
    let (home, router, cookie) = signed_in("keys").await;
    let form = get_as(&router, &cookie, "/admin/keys/new").await;
    assert_eq!(form.status(), StatusCode::OK);

    let created = post_as(
        &router,
        &cookie,
        "/admin/keys/new",
        "name=macbook&url=http%3A%2F%2F100.64.0.2%3A8787",
    )
    .await;
    assert_eq!(created.status(), StatusCode::SEE_OTHER);
    let issued = location(&created).to_owned();
    assert!(issued.starts_with("/admin/keys/issued/"), "{issued}");
    for _ in 0..2 {
        let shown = get_as(&router, &cookie, &issued).await;
        assert_eq!(shown.status(), StatusCode::OK);
        assert_eq!(shown.headers()[header::CACHE_CONTROL], "no-store");
        let page = text(shown).await;
        assert!(page.contains("og_"));
        assert!(page.contains("http://100.64.0.2:8787"));
        assert!(page.contains("data-copy=\"key\""));
    }
    let saved = post_as(&router, &cookie, &format!("{issued}/done"), "").await;
    assert_eq!(location(&saved), "/admin/keys");
    assert!(
        follow(&router, &cookie, &saved)
            .await
            .contains("Created key macbook.")
    );
    let gone = get_as(&router, &cookie, &issued).await;
    assert_eq!(location(&gone), "/admin/keys");
    assert!(!follow(&router, &cookie, &gone).await.contains("og_"));

    let duplicate = post_as(
        &router,
        &cookie,
        "/admin/keys/new",
        "name=macbook&url=http%3A%2F%2Fx",
    )
    .await;
    assert_eq!(duplicate.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let keys = || {
        let store = Store::open(&crate::home::database(&home.0).unwrap()).unwrap();
        store.keys().unwrap().len()
    };
    assert_eq!(keys(), 1);
    let refused = post_as(
        &router,
        &cookie,
        "/admin/key/macbook/revoke",
        "confirm=other",
    )
    .await;
    assert_eq!(refused.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let revoked = post_as(
        &router,
        &cookie,
        "/admin/key/macbook/revoke",
        "confirm=macbook",
    )
    .await;
    assert_eq!(location(&revoked), "/admin/keys");
    assert_eq!(keys(), 0);
    let page = follow(&router, &cookie, &revoked).await;
    assert!(page.contains("Revoked macbook."));
    assert!(page.contains("No keys yet."));
}

#[tokio::test]
async fn actions_need_a_session() {
    let home = Home::new("guarded");
    home.token(false);
    let router = app(&home);
    for path in [
        "/admin/accounts/new/custom/custom",
        "/admin/keys/new",
        "/admin/models/refresh",
    ] {
        let response = send(
            &router,
            post_form(path, "name=x&url=http%3A%2F%2Fx&api=chat"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER, "{path}");
        assert_eq!(location(&response), "/admin/login", "{path}");
    }
    assert_eq!(accounts(&home), Vec::<String>::new());
}

fn seed(home: &Home) -> i64 {
    use crate::account::{Account, Secret, State};
    use crate::provider::{Kind, Method, Provider, Usage, Window};
    use crate::store::LogRow;

    let mut store = Store::open(&crate::home::database(&home.0).unwrap()).unwrap();
    let now = crate::clock::now();
    let day = 86_400;
    for (index, name) in ["claude-a", "claude-b"].into_iter().enumerate() {
        store
            .insert_account(&Account {
                name: name.to_owned(),
                kind: Kind::new(Provider::Anthropic, Method::Oauth).unwrap(),
                secret: Secret::Key {
                    key: "secret-value".to_owned(),
                },
                state: if index == 1 {
                    State::Inactive {
                        reason: "refresh failed".to_owned(),
                    }
                } else {
                    State::Active
                },
                usage: None,
                usage_at: None,
                created_at: now - 30 * day,
            })
            .unwrap();
        if index == 1 {
            store
                .set_state(
                    name,
                    &State::Inactive {
                        reason: "refresh failed".to_owned(),
                    },
                )
                .unwrap();
        }
        for step in 0..40 {
            let at = now - 12 * day + step * 7 * 3600;
            let cycle_reset = now - 5 * day + if at > now - 5 * day { 7 * day } else { 0 };
            let used = f64::from(u8::try_from(step % 20).unwrap()) / 25.0;
            let usage = Usage {
                windows: vec![
                    Window {
                        label: "5h".to_owned(),
                        used: used / 2.0,
                        reset_at: Some(at + 3 * 3600),
                    },
                    Window {
                        label: "7d".to_owned(),
                        used,
                        reset_at: Some(cycle_reset),
                    },
                    Window {
                        label: "7d_fable".to_owned(),
                        used: used * 1.2,
                        reset_at: Some(cycle_reset),
                    },
                ],
            };
            store.set_usage(name, &usage, at).unwrap();
        }
    }
    for step in 0..120 {
        let limit = step % 17 == 0;
        let failed = step % 23 == 0;
        store
            .insert_log(&LogRow {
                ts: now - step * 1800,
                key: Some(if step % 3 == 0 { "desktop" } else { "macbook" }.to_owned()),
                session: Some(format!("session-{}", step % 4)),
                api: "messages".to_owned(),
                model: if step % 2 == 0 {
                    "claude-opus-5-5"
                } else {
                    "claude-fable-5-2"
                }
                .to_owned(),
                account: Some("claude-a".to_owned()),
                reason: if limit { "limit" } else { "reused" }.to_owned(),
                status: if limit {
                    429
                } else if failed {
                    529
                } else {
                    200
                },
                error: failed.then(|| "<b>overloaded</b>".to_owned()),
                note: None,
                input: Some(1200 + step),
                cache_read: Some(40_000 + step * 100),
                cache_write: Some(if step % 10 == 0 { 30_000 } else { 0 }),
                output: Some(800),
                duration_ms: 4000 + step * 37,
                ..LogRow::default()
            })
            .unwrap();
    }
    store
        .insert_key("macbook", "hash-1", now - 40 * day)
        .unwrap();
    store
        .insert_key("old-phone", "hash-2", now - 90 * day)
        .unwrap();
    now
}

#[tokio::test]
async fn every_page_renders_real_data_in_the_browser_time_zone() {
    let (home, router, cookie) = signed_in("data").await;
    seed(&home);
    let cookie = format!("{cookie}; opengateway_tz=32400");
    let pages = [
        (
            "/admin/overview",
            vec![
                "2 accounts need attention",
                "claude-b cannot take requests",
                "Sign in again",
                "Where the next request goes",
                "claude-opus-5-5",
            ],
        ),
        (
            "/admin/usage",
            vec![
                "Anthropic capacity left, 7 days",
                "7 days fable",
                "counts accounts, not tokens",
                "Left at the last reading before each reset",
                "Show data",
            ],
        ),
        (
            "/admin/usage?provider=anthropic&window=5h&range=2d",
            vec!["5 hours"],
        ),
        (
            "/admin/traffic",
            vec!["By key", "desktop", "macbook", "When you work"],
        ),
        (
            "/admin/traffic?range=30d&by=outcome",
            vec!["limit", "error"],
        ),
        (
            "/admin/activity",
            vec![
                "claude-fable-5-2",
                "529 error",
                "&#60;b&#62;overloaded",
                "limit, tried the next account",
            ],
        ),
        ("/admin/activity?outcome=limit", vec!["limit"]),
        ("/admin/activity?q=key%3Adesktop", vec!["desktop"]),
        (
            "/admin/accounts",
            vec!["claude-a", "Signed out: refresh failed", "7 days fable"],
        ),
        (
            "/admin/account/claude-a",
            vec![
                "Usage now",
                "Recent conversations",
                "Recent errors",
                "Remove account",
            ],
        ),
        ("/admin/models", vec!["No models yet"]),
        (
            "/admin/keys",
            vec!["old-phone", "Never used in 90 days", "Revoke"],
        ),
        (
            "/admin/request/1",
            vec!["Request 1", "session-0", "Cache read"],
        ),
        (
            "/admin/session/session-1",
            vec!["Conversation", "Input per request", "cache read"],
        ),
        (
            "/admin/key/macbook",
            vec!["Revoke macbook", "requests in the last 24 hours"],
        ),
    ];
    for (path, wanted) in pages {
        let response = get_as(&router, &cookie, path).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        let body = text(response).await;
        assert!(body.contains("Times in UTC+09:00"), "{path}");
        assert!(!body.contains("secret-value"), "{path}");
        assert!(!body.contains("<b>overloaded"), "{path}");
        for want in wanted {
            assert!(body.contains(want), "{path} lacks {want}\n{body}");
        }
    }
    for path in [
        "/admin/live/overview",
        "/admin/live/usage?provider=anthropic&window=7d&range=14d",
        "/admin/live/traffic?range=24h&by=model",
        "/admin/live/activity",
        "/admin/live/accounts",
        "/admin/live/models",
        "/admin/live/keys",
    ] {
        let mut request = get(path)
            .header(header::COOKIE, &cookie)
            .body(Body::empty())
            .unwrap();
        request
            .headers_mut()
            .insert("hx-request", "true".parse().unwrap());
        let response = send(&router, request).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        let body = text(response).await;
        assert!(body.starts_with("<div id=\"live\""), "{path}");
        assert!(!body.contains("<html"), "{path}");
    }
    let older = text(get_as(&router, &cookie, "/admin/activity").await).await;
    assert!(older.contains("/admin/activity?before="));
    let missing = get_as(&router, &cookie, "/admin/request/9999").await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_sign_in_returns_to_the_page_that_asked_for_it() {
    let home = Home::new("next");
    let token = home.token(false);
    let router = app(&home);
    let response = send(
        &router,
        post_form(
            "/admin/login",
            &format!("token={token}&next=%2Fadmin%2Factivity%3Foutcome%3Derror"),
        ),
    )
    .await;
    assert_eq!(location(&response), "/admin/activity?outcome=error");
    for evil in [
        "https%3A%2F%2Fevil.example",
        "%2F%2Fevil.example",
        "%2Fadmin%2F%2Fx",
    ] {
        let response = send(
            &router,
            post_form("/admin/login", &format!("token={token}&next={evil}")),
        )
        .await;
        assert_eq!(location(&response), "/admin/overview", "{evil}");
    }
    let cookie = sign_in(&router, &token).await;
    let again = get_as(&router, &cookie, "/admin/login?next=%2Fadmin%2Fkeys").await;
    assert_eq!(location(&again), "/admin/keys");
}

#[tokio::test]
async fn cancelling_a_sign_in_forgets_it_and_adds_nothing() {
    let (home, router, cookie) = signed_in("cancel").await;
    let started = post_as(
        &router,
        &cookie,
        "/admin/accounts/new/anthropic/oauth",
        "name=claude",
    )
    .await;
    let page_path = location(&started).to_owned();
    let cancelled = post_as(&router, &cookie, &format!("{page_path}/cancel"), "").await;
    assert_eq!(location(&cancelled), "/admin/accounts/new");
    assert!(
        follow(&router, &cookie, &cancelled)
            .await
            .contains("Cancelled the sign-in.")
    );
    let state = text(get_as(&router, &cookie, &format!("{page_path}/state")).await).await;
    assert!(state.contains("no longer running"), "{state}");
    assert!(!state.contains("hx-trigger"), "{state}");
    let pasted = post_as(&router, &cookie, &page_path, "value=code%23state").await;
    assert_eq!(location(&pasted), page_path);
    let gone = get_as(&router, &cookie, &page_path).await;
    assert_eq!(location(&gone), "/admin/accounts");
    assert_eq!(accounts(&home), Vec::<String>::new());
}

#[tokio::test]
async fn a_search_with_no_match_nearby_keeps_going_further_back() {
    let (home, router, cookie) = signed_in("scan").await;
    {
        let store = Store::open(&crate::home::database(&home.0).unwrap()).unwrap();
        for ts in 0..3_500 {
            store
                .insert_log(&crate::store::LogRow {
                    ts,
                    model: if ts == 10 { "needle" } else { "hay" }.to_owned(),
                    reason: "new".to_owned(),
                    status: 200,
                    ..crate::store::LogRow::default()
                })
                .unwrap();
        }
    }
    let page = text(get_as(&router, &cookie, "/admin/activity?q=needle").await).await;
    assert!(
        page.contains("No match in the requests searched so far."),
        "{page}"
    );
    let further = page
        .split("href=\"")
        .find_map(|part| {
            part.strip_prefix("/admin/activity?q=needle&#38;before=")
                .and_then(|rest| rest.split('"').next())
        })
        .unwrap()
        .to_owned();
    let next = text(
        get_as(
            &router,
            &cookie,
            &format!("/admin/activity?q=needle&before={further}"),
        )
        .await,
    )
    .await;
    assert!(next.contains(">needle<"), "{next}");
}
