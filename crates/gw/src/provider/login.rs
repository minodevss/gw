use std::time::Duration;

use serde::{Deserialize, Deserializer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use url::Url;

use super::oauth::{
    self, ANTHROPIC_AUTHORIZE_URL, ANTHROPIC_CLIENT_ID, ANTHROPIC_REDIRECT_URI, ANTHROPIC_SCOPE,
    ANTHROPIC_TOKEN_URL, Body, CODEX_CLIENT_ID, CODEX_DEVICE_REDIRECT_URI, CODEX_DEVICE_TOKEN_URL,
    CODEX_TOKEN_URL, CODEX_USER_CODE_URL, CODEX_VERIFICATION_URI, Pkce, SIWC_AUTHORIZE_URL,
    SIWC_REGISTRATION_CLIENT_ID, SIWC_RESOURCE, SIWC_SCOPE, SIWC_TOKEN_URL, TokenResponse,
};
use super::{Kind, Method, ProviderError};
use crate::account::Secret;

const STATE_BYTES: usize = 32;
const AGENT_NAME_HINT: &str = "gw";
const CALLBACK_PATH: &str = "/auth/callback";
const LOOPBACK_TIMEOUT: Duration = Duration::from_secs(600);
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_REQUEST_LINE: usize = 8192;
const DEVICE_CODE_LIFETIME_SECS: i64 = 900;
const DEFAULT_DEVICE_INTERVAL_SECS: u64 = 5;
const SLOW_DOWN_STEP_SECS: u64 = 5;

const SUCCESS_PAGE: &str = "<!doctype html><meta charset=\"utf-8\"><title>gw</title><p>Signed in. You can close this tab.</p>";
const FAILURE_PAGE: &str = "<!doctype html><meta charset=\"utf-8\"><title>gw</title><p>Sign-in failed. Return to the terminal.</p>";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Prompt {
    PasteCode {
        url: String,
    },
    Loopback {
        url: String,
        port: u16,
        ssh_hint: String,
    },
    Device {
        verification_uri: String,
        user_code: String,
        expires_at: i64,
        interval: u64,
    },
}

impl Prompt {
    pub(crate) fn url(&self) -> &str {
        match self {
            Self::PasteCode { url } | Self::Loopback { url, .. } => url,
            Self::Device {
                verification_uri, ..
            } => verification_uri,
        }
    }
}

pub(crate) struct LoginSession {
    http: reqwest::Client,
    prompt: Prompt,
    flow: Flow,
}

enum Flow {
    PasteCode(PasteCode),
    Loopback(Loopback),
    Device(Device),
}

struct PasteCode {
    verifier: String,
    state: String,
    token_url: String,
}

struct Loopback {
    listener: TcpListener,
    verifier: String,
    state: String,
    redirect_uri: String,
    token_url: String,
}

struct Device {
    auth_id: String,
    user_code: String,
    interval: u64,
    expires_at: i64,
    poll_url: String,
    token_url: String,
}

impl std::fmt::Debug for LoginSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoginSession")
            .field("prompt", &self.prompt)
            .finish_non_exhaustive()
    }
}

pub(crate) async fn start_login(
    http: &reqwest::Client,
    kind: Kind,
    host_id: &str,
) -> Result<LoginSession, ProviderError> {
    match kind.method() {
        Method::Oauth => start_paste_code(http, ANTHROPIC_AUTHORIZE_URL, ANTHROPIC_TOKEN_URL),
        Method::Siwc => start_loopback(http, host_id, SIWC_AUTHORIZE_URL, SIWC_TOKEN_URL).await,
        Method::Codex => {
            start_device(
                http,
                CODEX_USER_CODE_URL,
                CODEX_DEVICE_TOKEN_URL,
                CODEX_TOKEN_URL,
            )
            .await
        }
        Method::SetupToken | Method::Plan | Method::Key | Method::Custom => Err(
            ProviderError::Invalid(format!("{} has no login flow", kind.method())),
        ),
    }
}

impl LoginSession {
    pub(crate) fn prompt(&self) -> &Prompt {
        &self.prompt
    }

    pub(crate) async fn complete(self, pasted: Option<String>) -> Result<Secret, ProviderError> {
        let Self { http, flow, .. } = self;
        match flow {
            Flow::PasteCode(flow) => complete_paste_code(&http, &flow, pasted.as_deref()).await,
            Flow::Loopback(flow) => complete_loopback(&http, flow).await,
            Flow::Device(flow) => complete_device(&http, &flow).await,
        }
    }
}

fn start_paste_code(
    http: &reqwest::Client,
    authorize_url: &str,
    token_url: &str,
) -> Result<LoginSession, ProviderError> {
    let pkce = Pkce::generate()?;
    let state = oauth::random_token(STATE_BYTES)?;
    let mut url = parse_url(authorize_url)?;
    url.query_pairs_mut()
        .append_pair("code", "true")
        .append_pair("client_id", ANTHROPIC_CLIENT_ID)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", ANTHROPIC_REDIRECT_URI)
        .append_pair("scope", ANTHROPIC_SCOPE)
        .append_pair("code_challenge", &pkce.challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", &state);
    Ok(LoginSession {
        http: http.clone(),
        prompt: Prompt::PasteCode { url: url.into() },
        flow: Flow::PasteCode(PasteCode {
            verifier: pkce.verifier,
            state,
            token_url: token_url.to_owned(),
        }),
    })
}

async fn start_loopback(
    http: &reqwest::Client,
    host_id: &str,
    authorize_url: &str,
    token_url: &str,
) -> Result<LoginSession, ProviderError> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}{CALLBACK_PATH}");
    let pkce = Pkce::generate()?;
    let state = oauth::random_token(STATE_BYTES)?;
    let nonce = oauth::random_token(STATE_BYTES)?;
    let mut url = parse_url(authorize_url)?;
    url.query_pairs_mut()
        .append_pair("client_id", SIWC_REGISTRATION_CLIENT_ID)
        .append_pair("agent_name_hint", AGENT_NAME_HINT)
        .append_pair("ext_agent_host_id", host_id)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("scope", SIWC_SCOPE)
        .append_pair("resource", SIWC_RESOURCE)
        .append_pair("state", &state)
        .append_pair("nonce", &nonce)
        .append_pair("code_challenge_method", "S256")
        .append_pair("code_challenge", &pkce.challenge);
    Ok(LoginSession {
        http: http.clone(),
        prompt: Prompt::Loopback {
            url: url.into(),
            port,
            ssh_hint: format!("ssh -L {port}:127.0.0.1:{port} <server>"),
        },
        flow: Flow::Loopback(Loopback {
            listener,
            verifier: pkce.verifier,
            state,
            redirect_uri,
            token_url: token_url.to_owned(),
        }),
    })
}

#[derive(Deserialize)]
struct UserCodeResponse {
    device_auth_id: String,
    #[serde(alias = "usercode")]
    user_code: String,
    #[serde(default = "default_interval", deserialize_with = "lenient_u64")]
    interval: u64,
}

fn default_interval() -> u64 {
    DEFAULT_DEVICE_INTERVAL_SECS
}

// The usercode endpoint returns `interval` as a string, e.g. "5".
// https://github.com/openai/codex/blob/main/codex-rs/login/src/device_code_auth.rs
fn lenient_u64<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    let parsed = match &value {
        serde_json::Value::Number(number) => number.as_u64(),
        serde_json::Value::String(text) => text.trim().parse().ok(),
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Array(_)
        | serde_json::Value::Object(_) => None,
    };
    parsed.ok_or_else(|| serde::de::Error::custom("interval is not an unsigned integer"))
}

async fn start_device(
    http: &reqwest::Client,
    user_code_url: &str,
    poll_url: &str,
    token_url: &str,
) -> Result<LoginSession, ProviderError> {
    let body = serde_json::json!({ "client_id": CODEX_CLIENT_ID });
    let response = http.post(user_code_url).json(&body).send().await?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(ProviderError::Invalid(
            "device code login is not enabled for this ChatGPT account".to_owned(),
        ));
    }
    let text = oauth::read_success(response).await?;
    let issued: UserCodeResponse = serde_json::from_str(&text)?;
    let interval = issued.interval.max(1);
    let expires_at = oauth::unix_now().saturating_add(DEVICE_CODE_LIFETIME_SECS);
    Ok(LoginSession {
        http: http.clone(),
        prompt: Prompt::Device {
            verification_uri: CODEX_VERIFICATION_URI.to_owned(),
            user_code: issued.user_code.clone(),
            expires_at,
            interval,
        },
        flow: Flow::Device(Device {
            auth_id: issued.device_auth_id,
            user_code: issued.user_code,
            interval,
            expires_at,
            poll_url: poll_url.to_owned(),
            token_url: token_url.to_owned(),
        }),
    })
}

fn parse_url(raw: &str) -> Result<Url, ProviderError> {
    Url::parse(raw).map_err(|error| ProviderError::Invalid(format!("bad url: {error}")))
}

fn split_pasted(pasted: &str) -> Option<(&str, &str)> {
    let (code, state) = pasted.trim().split_once('#')?;
    (!code.is_empty() && !state.is_empty()).then_some((code, state))
}

async fn complete_paste_code(
    http: &reqwest::Client,
    flow: &PasteCode,
    pasted: Option<&str>,
) -> Result<Secret, ProviderError> {
    let pasted = pasted.ok_or_else(|| ProviderError::Invalid("no code pasted".to_owned()))?;
    let (code, state) = split_pasted(pasted)
        .ok_or_else(|| ProviderError::Invalid("pasted value is not code#state".to_owned()))?;
    if state != flow.state {
        return Err(ProviderError::Invalid("state mismatch".to_owned()));
    }
    let body = serde_json::json!({
        "grant_type": "authorization_code",
        "code": code,
        "state": state,
        "redirect_uri": ANTHROPIC_REDIRECT_URI,
        "client_id": ANTHROPIC_CLIENT_ID,
        "code_verifier": flow.verifier,
    });
    let text = oauth::post(http, &flow.token_url, Body::Json(&body)).await?;
    let token = oauth::parse_token(&text)?;
    let refresh = token
        .refresh_token
        .clone()
        .ok_or_else(|| ProviderError::Invalid("token response has no refresh_token".to_owned()))?;
    Ok(Secret::Oauth {
        expires_at: oauth::expires_at(&token, oauth::unix_now())?,
        access: token.access_token,
        refresh,
        account_id: None,
        client_id: None,
    })
}

#[derive(Debug, PartialEq, Eq)]
enum Callback {
    Authorized {
        code: String,
        client_id: Option<String>,
    },
    Denied,
    Failed(String),
    Ignored,
}

fn parse_callback(request_line: &str, expected_state: &str) -> Callback {
    let mut parts = request_line.split_whitespace();
    let (Some("GET"), Some(target)) = (parts.next(), parts.next()) else {
        return Callback::Ignored;
    };
    let Ok(url) = Url::parse(&format!("http://127.0.0.1{target}")) else {
        return Callback::Ignored;
    };
    if url.path() != CALLBACK_PATH {
        return Callback::Ignored;
    }
    let param = |name: &str| {
        url.query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    };
    if param("state").as_deref() != Some(expected_state) {
        return Callback::Ignored;
    }
    if let Some(error) = param("error") {
        return if error == "access_denied" {
            Callback::Denied
        } else {
            Callback::Failed(format!("authorization failed: {error}"))
        };
    }
    match param("code") {
        Some(code) if !code.is_empty() => Callback::Authorized {
            code,
            client_id: param("client_id").filter(|id| !id.is_empty()),
        },
        Some(_) | None => Callback::Failed("callback has no code".to_owned()),
    }
}

async fn read_request_line(stream: &mut TcpStream) -> Option<String> {
    let mut data = Vec::new();
    let mut chunk = [0_u8; 1024];
    while !data.windows(2).any(|pair| pair == b"\r\n") && data.len() < MAX_REQUEST_LINE {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            break;
        }
        data.extend_from_slice(chunk.get(..read)?);
    }
    let text = String::from_utf8_lossy(&data);
    text.lines().next().map(str::to_owned)
}

async fn respond(stream: &mut TcpStream, status: &str, page: &str) -> std::io::Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{page}",
        page.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await
}

struct Authorized {
    code: String,
    client_id: Option<String>,
}

async fn handle_connection(
    mut stream: TcpStream,
    state: &str,
) -> Option<Result<Authorized, ProviderError>> {
    let line = tokio::time::timeout(CONNECTION_TIMEOUT, read_request_line(&mut stream))
        .await
        .ok()??;
    let callback = parse_callback(&line, state);
    let (status, page) = match callback {
        Callback::Authorized { .. } => ("200 OK", SUCCESS_PAGE),
        Callback::Denied | Callback::Failed(_) => ("400 Bad Request", FAILURE_PAGE),
        Callback::Ignored => ("404 Not Found", FAILURE_PAGE),
    };
    respond(&mut stream, status, page).await.unwrap_or_default();
    match callback {
        Callback::Authorized { code, client_id } => Some(Ok(Authorized { code, client_id })),
        Callback::Denied => Some(Err(ProviderError::Cancelled)),
        Callback::Failed(message) => Some(Err(ProviderError::Invalid(message))),
        Callback::Ignored => None,
    }
}

async fn await_callback(listener: &TcpListener, state: &str) -> Result<Authorized, ProviderError> {
    let serve = async {
        loop {
            let (stream, _) = listener.accept().await?;
            if let Some(outcome) = handle_connection(stream, state).await {
                return outcome;
            }
        }
    };
    tokio::time::timeout(LOOPBACK_TIMEOUT, serve)
        .await
        .map_err(|_| ProviderError::Expired)?
}

async fn complete_loopback(
    http: &reqwest::Client,
    flow: Loopback,
) -> Result<Secret, ProviderError> {
    let authorized = await_callback(&flow.listener, &flow.state).await?;
    drop(flow.listener);
    let client_id = authorized.client_id.ok_or_else(|| {
        ProviderError::Invalid("callback has no client_id; registration incomplete".to_owned())
    })?;
    let text = oauth::post(
        http,
        &flow.token_url,
        Body::Form(&[
            ("grant_type", "authorization_code"),
            ("client_id", &client_id),
            ("code", &authorized.code),
            ("redirect_uri", &flow.redirect_uri),
            ("code_verifier", &flow.verifier),
            ("resource", SIWC_RESOURCE),
        ]),
    )
    .await?;
    siwc_secret(&oauth::parse_token(&text)?, client_id)
}

fn siwc_secret(token: &TokenResponse, client_id: String) -> Result<Secret, ProviderError> {
    let refresh = token
        .refresh_token
        .clone()
        .ok_or_else(|| ProviderError::Invalid("token response has no refresh_token".to_owned()))?;
    Ok(Secret::Oauth {
        access: token.access_token.clone(),
        refresh,
        expires_at: oauth::expires_at(token, oauth::unix_now())?,
        account_id: None,
        client_id: Some(client_id),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Retry {
    Pending,
    SlowDown,
}

#[derive(Deserialize)]
struct DeviceGrant {
    authorization_code: String,
    code_verifier: String,
}

enum Poll {
    Authorized(DeviceGrant),
    Retry(Retry),
}

// The device token endpoint answers 403 or 404 while the user has not approved yet.
// https://github.com/openai/codex/blob/main/codex-rs/login/src/device_code_auth.rs
fn classify_poll(status: u16, body: &str) -> Result<Poll, ProviderError> {
    if (200..300).contains(&status) {
        return Ok(Poll::Authorized(serde_json::from_str(body)?));
    }
    if status == 403 || status == 404 {
        return Ok(Poll::Retry(Retry::Pending));
    }
    match oauth::error_code(body).as_deref() {
        Some("authorization_pending") => Ok(Poll::Retry(Retry::Pending)),
        Some("slow_down") => Ok(Poll::Retry(Retry::SlowDown)),
        Some("expired_token") => Err(ProviderError::Expired),
        Some("access_denied") => Err(ProviderError::Cancelled),
        Some(_) | None => Err(ProviderError::Status {
            status,
            body_excerpt: oauth::excerpt(body),
        }),
    }
}

struct Poller {
    interval: u64,
    expires_at: i64,
}

impl Poller {
    fn after(&mut self, retry: Retry, now: i64) -> Result<Duration, ProviderError> {
        if now >= self.expires_at {
            return Err(ProviderError::Expired);
        }
        match retry {
            Retry::Pending => {}
            Retry::SlowDown => self.interval = self.interval.saturating_add(SLOW_DOWN_STEP_SECS),
        }
        let remaining = u64::try_from(self.expires_at.saturating_sub(now)).unwrap_or(0);
        Ok(Duration::from_secs(self.interval.min(remaining)))
    }
}

async fn complete_device(http: &reqwest::Client, flow: &Device) -> Result<Secret, ProviderError> {
    let grant = poll_device(http, flow).await?;
    let text = oauth::post(
        http,
        &flow.token_url,
        Body::Form(&[
            ("grant_type", "authorization_code"),
            ("client_id", CODEX_CLIENT_ID),
            ("code", &grant.authorization_code),
            ("redirect_uri", CODEX_DEVICE_REDIRECT_URI),
            ("code_verifier", &grant.code_verifier),
        ]),
    )
    .await?;
    codex_secret(&oauth::parse_token(&text)?)
}

async fn poll_device(http: &reqwest::Client, flow: &Device) -> Result<DeviceGrant, ProviderError> {
    let body = serde_json::json!({
        "device_auth_id": flow.auth_id,
        "user_code": flow.user_code,
    });
    let mut poller = Poller {
        interval: flow.interval,
        expires_at: flow.expires_at,
    };
    loop {
        let response = http.post(&flow.poll_url).json(&body).send().await?;
        let status = response.status().as_u16();
        let text = response.text().await?;
        match classify_poll(status, &text)? {
            Poll::Authorized(grant) => return Ok(grant),
            Poll::Retry(retry) => {
                tokio::time::sleep(poller.after(retry, oauth::unix_now())?).await;
            }
        }
    }
}

fn codex_secret(token: &TokenResponse) -> Result<Secret, ProviderError> {
    let refresh = token
        .refresh_token
        .clone()
        .ok_or_else(|| ProviderError::Invalid("token response has no refresh_token".to_owned()))?;
    let id_token = token
        .id_token
        .as_deref()
        .ok_or_else(|| ProviderError::Invalid("token response has no id_token".to_owned()))?;
    Ok(Secret::Oauth {
        access: token.access_token.clone(),
        refresh,
        expires_at: oauth::jwt_expiry(&token.access_token)?,
        account_id: oauth::chatgpt_account_id(id_token)?,
        client_id: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::Provider;
    use crate::provider::oauth::fake::{FakeServer, Reply};

    fn kind(provider: Provider, method: Method) -> Kind {
        Kind::new(provider, method).unwrap()
    }

    fn query(url: &str) -> std::collections::BTreeMap<String, String> {
        Url::parse(url)
            .unwrap()
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect()
    }

    fn jwt(payload: &str) -> String {
        use base64::Engine as _;
        format!(
            "h.{}.s",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload)
        )
    }

    #[test]
    fn parses_callback_with_valid_state() {
        let line = "GET /auth/callback?code=abc&state=s1&client_id=oaiapp_1 HTTP/1.1";
        assert_eq!(
            parse_callback(line, "s1"),
            Callback::Authorized {
                code: "abc".to_owned(),
                client_id: Some("oaiapp_1".to_owned())
            }
        );
    }

    #[test]
    fn ignores_callback_with_wrong_state_or_path() {
        assert_eq!(
            parse_callback("GET /auth/callback?code=abc&state=bad HTTP/1.1", "s1"),
            Callback::Ignored
        );
        assert_eq!(
            parse_callback("GET /favicon.ico HTTP/1.1", "s1"),
            Callback::Ignored
        );
        assert_eq!(
            parse_callback("POST /auth/callback?state=s1 HTTP/1.1", "s1"),
            Callback::Ignored
        );
    }

    #[test]
    fn access_denied_cancels_only_with_valid_state() {
        let line = "GET /auth/callback?error=access_denied&state=s1 HTTP/1.1";
        assert_eq!(parse_callback(line, "s1"), Callback::Denied);
        assert_eq!(parse_callback(line, "other"), Callback::Ignored);
    }

    #[test]
    fn callback_without_code_fails() {
        let line = "GET /auth/callback?state=s1 HTTP/1.1";
        assert!(matches!(parse_callback(line, "s1"), Callback::Failed(_)));
    }

    #[test]
    fn splits_pasted_code_and_state() {
        assert_eq!(split_pasted(" abc#def \n"), Some(("abc", "def")));
        assert_eq!(split_pasted("abc"), None);
        assert_eq!(split_pasted("#def"), None);
    }

    #[test]
    fn device_interval_accepts_string_and_number() {
        let from_string: UserCodeResponse =
            serde_json::from_str(r#"{"device_auth_id":"d","usercode":"ABCD-1234","interval":"7"}"#)
                .unwrap();
        assert_eq!(
            (from_string.user_code.as_str(), from_string.interval),
            ("ABCD-1234", 7)
        );
        let from_number: UserCodeResponse =
            serde_json::from_str(r#"{"device_auth_id":"d","user_code":"X","interval":3}"#).unwrap();
        assert_eq!(from_number.interval, 3);
        let missing: UserCodeResponse =
            serde_json::from_str(r#"{"device_auth_id":"d","user_code":"X"}"#).unwrap();
        assert_eq!(missing.interval, 5);
    }

    #[test]
    fn poll_classification_follows_device_rules() {
        let grant = r#"{"authorization_code":"c","code_challenge":"x","code_verifier":"v"}"#;
        assert!(matches!(classify_poll(200, grant), Ok(Poll::Authorized(_))));
        assert!(matches!(
            classify_poll(403, ""),
            Ok(Poll::Retry(Retry::Pending))
        ));
        assert!(matches!(
            classify_poll(404, ""),
            Ok(Poll::Retry(Retry::Pending))
        ));
        assert!(matches!(
            classify_poll(400, r#"{"error":"slow_down"}"#),
            Ok(Poll::Retry(Retry::SlowDown))
        ));
        assert!(matches!(
            classify_poll(400, r#"{"error":"authorization_pending"}"#),
            Ok(Poll::Retry(Retry::Pending))
        ));
        assert!(matches!(
            classify_poll(400, r#"{"error":"expired_token"}"#),
            Err(ProviderError::Expired)
        ));
        assert!(matches!(
            classify_poll(400, r#"{"error":"access_denied"}"#),
            Err(ProviderError::Cancelled)
        ));
        assert!(matches!(
            classify_poll(500, "boom"),
            Err(ProviderError::Status { status: 500, .. })
        ));
    }

    #[test]
    fn poller_waits_interval_and_backs_off_on_slow_down() {
        let mut poller = Poller {
            interval: 5,
            expires_at: 1000,
        };
        assert_eq!(
            poller.after(Retry::Pending, 100).unwrap(),
            Duration::from_secs(5)
        );
        assert_eq!(
            poller.after(Retry::SlowDown, 105).unwrap(),
            Duration::from_secs(10)
        );
        assert_eq!(
            poller.after(Retry::Pending, 115).unwrap(),
            Duration::from_secs(10)
        );
    }

    #[test]
    fn poller_caps_wait_at_deadline_then_expires() {
        let mut poller = Poller {
            interval: 10,
            expires_at: 1000,
        };
        assert_eq!(
            poller.after(Retry::Pending, 996).unwrap(),
            Duration::from_secs(4)
        );
        assert!(matches!(
            poller.after(Retry::Pending, 1000),
            Err(ProviderError::Expired)
        ));
    }

    #[tokio::test]
    async fn anthropic_prompt_carries_pkce_and_state() {
        let session = start_login(
            &reqwest::Client::new(),
            kind(Provider::Anthropic, Method::Oauth),
            "urn:uuid:x",
        )
        .await
        .unwrap();
        let Prompt::PasteCode { url } = session.prompt() else {
            panic!("expected paste code prompt");
        };
        let params = query(url);
        assert!(url.starts_with("https://claude.com/cai/oauth/authorize?"));
        assert_eq!(params["client_id"], ANTHROPIC_CLIENT_ID);
        assert_eq!(params["scope"], "user:inference user:profile");
        assert_eq!(params["redirect_uri"], ANTHROPIC_REDIRECT_URI);
        assert_eq!(params["code_challenge_method"], "S256");
        let Flow::PasteCode(flow) = &session.flow else {
            panic!("expected paste flow");
        };
        assert_eq!(params["state"], flow.state);
        assert_eq!(
            params["code_challenge"],
            oauth::challenge_for(&flow.verifier)
        );
    }

    #[tokio::test]
    async fn rejects_methods_without_login_flow() {
        let result = start_login(
            &reqwest::Client::new(),
            kind(Provider::OpenAi, Method::Key),
            "h",
        )
        .await;
        assert!(matches!(result, Err(ProviderError::Invalid(_))));
    }

    #[tokio::test]
    async fn paste_code_exchanges_and_rejects_foreign_state() {
        let server = FakeServer::start(vec![Reply {
            status: 200,
            body: r#"{"access_token":"at","refresh_token":"rt","expires_in":3600,"token_type":"Bearer"}"#.to_owned(),
        }])
        .await;
        let http = reqwest::Client::new();
        let token_url = format!("{}/v1/oauth/token", server.base_url);
        let session = start_paste_code(&http, ANTHROPIC_AUTHORIZE_URL, &token_url).unwrap();
        let Flow::PasteCode(flow) = &session.flow else {
            panic!("expected paste flow");
        };
        let state = flow.state.clone();
        let verifier = flow.verifier.clone();

        let bad = start_paste_code(&http, ANTHROPIC_AUTHORIZE_URL, &token_url).unwrap();
        let rejected = bad.complete(Some("code#wrong".to_owned())).await;
        assert!(matches!(rejected, Err(ProviderError::Invalid(_))));

        let secret = session
            .complete(Some(format!("thecode#{state}")))
            .await
            .unwrap();
        let Secret::Oauth {
            access,
            refresh,
            expires_at,
            account_id,
            client_id,
        } = secret
        else {
            panic!("expected oauth secret");
        };
        assert_eq!((access.as_str(), refresh.as_str()), ("at", "rt"));
        assert!(expires_at > oauth::unix_now());
        assert_eq!((account_id, client_id), (None, None));

        let requests = server.requests().await;
        let request = requests.first().unwrap();
        assert!(request.starts_with("POST /v1/oauth/token"));
        assert!(request.contains(r#""grant_type":"authorization_code""#));
        assert!(request.contains(&format!(r#""code_verifier":"{verifier}""#)));
    }

    async fn browser_get(port: u16, target: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        stream
            .write_all(format!("GET {target} HTTP/1.1\r\nhost: 127.0.0.1\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        response
    }

    #[tokio::test]
    async fn loopback_waits_for_valid_state_then_exchanges_with_issued_client() {
        let server = FakeServer::start(vec![Reply {
            status: 200,
            body: r#"{"access_token":"at","refresh_token":"rt","expires_in":3600}"#.to_owned(),
        }])
        .await;
        let token_url = format!("{}/api/accounts/oauth/token", server.base_url);
        let session = start_loopback(
            &reqwest::Client::new(),
            "urn:uuid:host",
            SIWC_AUTHORIZE_URL,
            &token_url,
        )
        .await
        .unwrap();
        let Prompt::Loopback {
            url,
            port,
            ssh_hint,
        } = session.prompt().clone()
        else {
            panic!("expected loopback prompt");
        };
        let params = query(&url);
        assert_eq!(params["client_id"], "dynamic_agent_client");
        assert_eq!(params["ext_agent_host_id"], "urn:uuid:host");
        assert_eq!(params["resource"], SIWC_RESOURCE);
        assert_eq!(
            params["redirect_uri"],
            format!("http://127.0.0.1:{port}/auth/callback")
        );
        assert!(ssh_hint.contains(&format!("{port}:127.0.0.1:{port}")));
        let state = params["state"].clone();

        let browser = tokio::spawn(async move {
            let stray = browser_get(port, "/auth/callback?code=evil&state=nope").await;
            let ok = browser_get(
                port,
                &format!("/auth/callback?code=thecode&state={state}&client_id=oaiapp_9"),
            )
            .await;
            (stray, ok)
        });
        let secret = session.complete(None).await.unwrap();
        let (stray, ok) = browser.await.unwrap();
        assert!(stray.starts_with("HTTP/1.1 404"));
        assert!(ok.contains("You can close this tab"));
        let Secret::Oauth { client_id, .. } = secret else {
            panic!("expected oauth secret");
        };
        assert_eq!(client_id.as_deref(), Some("oaiapp_9"));

        let requests = server.requests().await;
        let request = requests.first().unwrap();
        assert!(request.contains("client_id=oaiapp_9"));
        assert!(request.contains("code=thecode"));
        assert!(request.contains("resource=https%3A%2F%2Fapi.openai.com%2Fv1"));
        assert!(!request.contains("scope="));
    }

    #[tokio::test]
    async fn loopback_access_denied_cancels() {
        let session = start_loopback(
            &reqwest::Client::new(),
            "h",
            SIWC_AUTHORIZE_URL,
            "http://127.0.0.1:1/unused",
        )
        .await
        .unwrap();
        let Prompt::Loopback { url, port, .. } = session.prompt().clone() else {
            panic!("expected loopback prompt");
        };
        let state = query(&url)["state"].clone();
        let browser = tokio::spawn(async move {
            browser_get(
                port,
                &format!("/auth/callback?error=access_denied&state={state}"),
            )
            .await
        });
        let result = session.complete(None).await;
        browser.await.unwrap();
        assert!(matches!(result, Err(ProviderError::Cancelled)));
    }

    #[tokio::test(start_paused = true)]
    async fn device_flow_polls_until_approved_then_extracts_account_id() {
        let access = jwt(r#"{"exp":1900000000}"#);
        let id = jwt(r#"{"https://api.openai.com/auth":{"chatgpt_account_id":"acct-7"}}"#);
        let token_body =
            format!(r#"{{"access_token":"{access}","refresh_token":"rt","id_token":"{id}"}}"#);
        let server = FakeServer::start(vec![
            Reply {
                status: 200,
                body: r#"{"device_auth_id":"dev-1","user_code":"ABCD-1234","interval":"0"}"#
                    .to_owned(),
            },
            Reply {
                status: 403,
                body: String::new(),
            },
            Reply {
                status: 200,
                body: r#"{"authorization_code":"ac","code_challenge":"cc","code_verifier":"cv"}"#
                    .to_owned(),
            },
            Reply {
                status: 200,
                body: token_body,
            },
        ])
        .await;
        let session = start_device(
            &reqwest::Client::new(),
            &format!("{}/usercode", server.base_url),
            &format!("{}/token", server.base_url),
            &format!("{}/oauth/token", server.base_url),
        )
        .await
        .unwrap();
        let Prompt::Device {
            verification_uri,
            user_code,
            ..
        } = session.prompt().clone()
        else {
            panic!("expected device prompt");
        };
        assert_eq!(verification_uri, "https://auth.openai.com/codex/device");
        assert_eq!(user_code, "ABCD-1234");

        let secret = session.complete(None).await.unwrap();
        let Secret::Oauth {
            expires_at,
            account_id,
            client_id,
            ..
        } = secret
        else {
            panic!("expected oauth secret");
        };
        assert_eq!(expires_at, 1_900_000_000);
        assert_eq!(account_id.as_deref(), Some("acct-7"));
        assert_eq!(client_id, None);

        let requests = server.requests().await;
        assert!(requests[0].contains(r#""client_id":"app_EMoamEEZ73f0CkXaXp7hrann""#));
        assert!(requests[1].contains(r#""device_auth_id":"dev-1""#));
        assert!(requests[3].contains("code_verifier=cv"));
        assert!(
            requests[3]
                .contains("redirect_uri=https%3A%2F%2Fauth.openai.com%2Fdeviceauth%2Fcallback")
        );
    }

    #[tokio::test]
    async fn device_flow_reports_disabled_account() {
        let server = FakeServer::start(vec![Reply {
            status: 404,
            body: String::new(),
        }])
        .await;
        let result = start_device(
            &reqwest::Client::new(),
            &format!("{}/usercode", server.base_url),
            "unused",
            "unused",
        )
        .await;
        assert!(matches!(result, Err(ProviderError::Invalid(_))));
    }
}
