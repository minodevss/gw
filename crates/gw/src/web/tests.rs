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
    axum::Router::new().nest("/admin", super::router(home.0.clone()))
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
            body.contains(&format!("hx-get=\"/admin/{tab}/live\"")),
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
        get("/admin/accounts/live")
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
    assert_ne!(asset.headers()[header::CACHE_CONTROL], "no-store");
}
