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
        let dir = std::env::temp_dir().join(format!("gw-web-{name}-{}", std::process::id()));
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
    assert_eq!(location(&response), "/admin/login");
    let page = text(send(&router, get("/admin/login").body(Body::empty()).unwrap()).await).await;
    assert!(page.contains("gw admin token"));
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
    assert_eq!(location(&response), "/admin/accounts");
    let set_cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
    for part in ["Path=/admin", "HttpOnly", "SameSite=Strict"] {
        assert!(set_cookie.contains(part), "{set_cookie}");
    }
    assert!(!set_cookie.contains("Secure"));
    let cookie = set_cookie.split(';').next().unwrap();

    for tab in ["accounts", "activity", "models", "keys"] {
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
            body.contains(&format!("hx-get=\"/admin/live/{tab}\"")),
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
            .header(header::HOST, "gw.example:8787");
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
        (None, Some("http://gw.example:8787")),
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
    assert_eq!(added.status(), StatusCode::OK);
    assert!(
        text(added)
            .await
            .contains("Added local, but its model list failed")
    );
    assert_eq!(accounts(&home), ["local"]);

    let again = post_as(&router, &cookie, "/admin/accounts/new/custom/custom", form).await;
    assert_eq!(again.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let page = text(get_as(&router, &cookie, "/admin/account/local").await).await;
    assert!(page.contains("Type local to remove it"));
    assert!(!page.contains("Sign in again"));

    let renamed = post_as(
        &router,
        &cookie,
        "/admin/account/local/rename",
        "new_name=ollama",
    )
    .await;
    assert_eq!(location(&renamed), "/admin/account/ollama");
    assert_eq!(accounts(&home), ["ollama"]);

    let refused = post_as(
        &router,
        &cookie,
        "/admin/account/ollama/remove",
        "confirm=nope",
    )
    .await;
    assert_eq!(refused.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(accounts(&home), ["ollama"]);
    let removed = post_as(
        &router,
        &cookie,
        "/admin/account/ollama/remove",
        "confirm=ollama",
    )
    .await;
    assert_eq!(location(&removed), "/admin/accounts");
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
    assert!(page.contains("Waiting for the sign-in"));

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
    assert_eq!(created.status(), StatusCode::OK);
    assert_eq!(created.headers()[header::CACHE_CONTROL], "no-store");
    let page = text(created).await;
    assert!(page.contains("gw_"));
    assert!(page.contains("http://100.64.0.2:8787"));

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
