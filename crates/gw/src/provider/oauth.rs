use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};

use super::ProviderError;

#[cfg(test)]
pub(super) mod fake;

pub(super) const ANTHROPIC_AUTHORIZE_URL: &str = "https://claude.com/cai/oauth/authorize";
pub(super) const ANTHROPIC_TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
pub(super) const ANTHROPIC_REDIRECT_URI: &str = "https://platform.claude.com/oauth/code/callback";
pub(super) const ANTHROPIC_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
pub(super) const ANTHROPIC_SCOPE: &str = "user:inference user:profile";

pub(super) const SIWC_AUTHORIZE_URL: &str = "https://auth.openai.com/api/accounts/authorize";
pub(super) const SIWC_TOKEN_URL: &str = "https://auth.openai.com/api/accounts/oauth/token";
pub(super) const SIWC_REGISTRATION_CLIENT_ID: &str = "dynamic_agent_client";
pub(super) const SIWC_SCOPE: &str =
    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
pub(super) const SIWC_RESOURCE: &str = "https://api.openai.com/v1";

pub(super) const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub(super) const CODEX_USER_CODE_URL: &str =
    "https://auth.openai.com/api/accounts/deviceauth/usercode";
pub(super) const CODEX_DEVICE_TOKEN_URL: &str =
    "https://auth.openai.com/api/accounts/deviceauth/token";
pub(super) const CODEX_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
pub(super) const CODEX_DEVICE_REDIRECT_URI: &str = "https://auth.openai.com/deviceauth/callback";
pub(super) const CODEX_VERIFICATION_URI: &str = "https://auth.openai.com/codex/device";

const EXCERPT_CHARS: usize = 200;

const EXPIRED_CODES: [&str; 4] = [
    "invalid_grant",
    "refresh_token_expired",
    "refresh_token_reused",
    "refresh_token_invalidated",
];

pub(super) struct Pkce {
    pub(super) verifier: String,
    pub(super) challenge: String,
}

#[derive(Deserialize)]
pub(super) struct TokenResponse {
    pub(super) access_token: String,
    #[serde(default)]
    pub(super) refresh_token: Option<String>,
    #[serde(default)]
    pub(super) id_token: Option<String>,
    #[serde(default)]
    pub(super) expires_in: Option<i64>,
}

pub(super) enum Body<'a> {
    Form(&'a [(&'a str, &'a str)]),
    Json(&'a serde_json::Value),
}

impl Pkce {
    pub(super) fn generate() -> Result<Self, ProviderError> {
        let verifier = random_token(64)?;
        let challenge = challenge_for(&verifier);
        Ok(Self {
            verifier,
            challenge,
        })
    }
}

pub(super) fn challenge_for(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

pub(super) fn random_token(bytes: usize) -> Result<String, ProviderError> {
    let mut buffer = vec![0_u8; bytes];
    getrandom::fill(&mut buffer)
        .map_err(|error| ProviderError::Invalid(format!("random source failed: {error}")))?;
    Ok(URL_SAFE_NO_PAD.encode(buffer))
}

pub(super) fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
}

pub(super) fn form_body(pairs: &[(&str, &str)]) -> String {
    url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs)
        .finish()
}

pub(super) fn excerpt(body: &str) -> String {
    body.chars().take(EXCERPT_CHARS).collect()
}

pub(super) fn error_code(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let text = |value: &serde_json::Value| value.as_str().map(str::to_owned);
    value
        .get("error")
        .and_then(|error| text(error).or_else(|| error.get("code").and_then(text)))
        .or_else(|| value.get("code").and_then(text))
}

pub(super) fn is_expired_code(code: &str) -> bool {
    EXPIRED_CODES
        .iter()
        .any(|expired| expired.eq_ignore_ascii_case(code))
}

pub(super) async fn post(
    http: &reqwest::Client,
    url: &str,
    body: Body<'_>,
) -> Result<String, ProviderError> {
    let request = http.post(url);
    let request = match body {
        Body::Form(pairs) => request
            .header("content-type", "application/x-www-form-urlencoded")
            .body(form_body(pairs)),
        Body::Json(value) => request.json(value),
    };
    read_success(request.send().await?).await
}

pub(super) async fn read_success(response: reqwest::Response) -> Result<String, ProviderError> {
    let status = response.status();
    let body = response.text().await?;
    if status.is_success() {
        return Ok(body);
    }
    Err(status_error(status.as_u16(), &body))
}

pub(super) fn status_error(status: u16, body: &str) -> ProviderError {
    if let Some(code) = error_code(body)
        && is_expired_code(&code)
    {
        return ProviderError::Expired;
    }
    ProviderError::Status {
        status,
        body_excerpt: token_error_excerpt(body),
    }
}

fn token_error_excerpt(body: &str) -> String {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return String::new();
    };
    let field = |name: &str| value.get(name).and_then(serde_json::Value::as_str);
    let text = match (field("error"), field("error_description")) {
        (Some(code), Some(description)) => format!("{code}: {description}"),
        (Some(text), None) | (None, Some(text)) => text.to_owned(),
        (None, None) => value
            .get("error")
            .map(ToString::to_string)
            .unwrap_or_default(),
    };
    excerpt(&text)
}

pub(super) fn parse_token(body: &str) -> Result<TokenResponse, ProviderError> {
    let token: TokenResponse = serde_json::from_str(body)?;
    if token.access_token.is_empty() {
        return Err(ProviderError::Invalid("empty access token".to_owned()));
    }
    Ok(token)
}

pub(super) fn expires_at(token: &TokenResponse, now: i64) -> Result<i64, ProviderError> {
    let seconds = token
        .expires_in
        .filter(|seconds| *seconds > 0)
        .ok_or_else(|| ProviderError::Invalid("token response has no expires_in".to_owned()))?;
    Ok(now.saturating_add(seconds))
}

pub(super) fn jwt_claims<T: DeserializeOwned>(jwt: &str) -> Result<T, ProviderError> {
    let payload = jwt
        .split('.')
        .nth(1)
        .ok_or_else(|| ProviderError::Invalid("malformed jwt".to_owned()))?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .map_err(|_| ProviderError::Invalid("malformed jwt payload".to_owned()))?;
    Ok(serde_json::from_slice(&bytes)?)
}

#[derive(Deserialize)]
struct ExpiryClaims {
    exp: Option<i64>,
}

#[derive(Deserialize)]
struct ChatgptClaims {
    #[serde(rename = "https://api.openai.com/auth", default)]
    auth: Option<ChatgptAuth>,
}

#[derive(Deserialize)]
struct ChatgptAuth {
    #[serde(default)]
    chatgpt_account_id: Option<String>,
}

pub(super) fn jwt_expiry(jwt: &str) -> Result<i64, ProviderError> {
    jwt_claims::<ExpiryClaims>(jwt)?
        .exp
        .ok_or_else(|| ProviderError::Invalid("access token has no exp claim".to_owned()))
}

pub(super) fn chatgpt_account_id(id_token: &str) -> Result<Option<String>, ProviderError> {
    Ok(jwt_claims::<ChatgptClaims>(id_token)?
        .auth
        .and_then(|auth| auth.chatgpt_account_id))
}

pub(super) async fn get_text(
    http: &reqwest::Client,
    url: &str,
    headers: http::HeaderMap,
) -> Result<String, ProviderError> {
    let response = http.get(url).headers(headers).send().await?;
    let status = response.status();
    let body = response.text().await?;
    if status.is_success() {
        return Ok(body);
    }
    Err(ProviderError::Status {
        status: status.as_u16(),
        body_excerpt: excerpt(&body),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_matches_rfc7636_example() {
        assert_eq!(
            challenge_for("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn generated_pkce_is_unique_and_consistent() {
        let a = Pkce::generate().unwrap();
        let b = Pkce::generate().unwrap();
        assert_ne!(a.verifier, b.verifier);
        assert_eq!(a.verifier.len(), 86);
        assert_eq!(a.challenge, challenge_for(&a.verifier));
    }

    #[test]
    fn reads_nested_and_flat_error_codes() {
        assert_eq!(
            error_code(r#"{"error":"invalid_grant"}"#).as_deref(),
            Some("invalid_grant")
        );
        assert_eq!(
            error_code(r#"{"error":{"code":"refresh_token_reused","message":"x"}}"#).as_deref(),
            Some("refresh_token_reused")
        );
        assert_eq!(
            error_code(r#"{"code":"refresh_token_expired"}"#).as_deref(),
            Some("refresh_token_expired")
        );
        assert_eq!(error_code("not json"), None);
    }

    #[test]
    fn expired_codes_map_to_expired() {
        let error = status_error(400, r#"{"error":"invalid_grant"}"#);
        assert!(matches!(error, ProviderError::Expired));
        let error = status_error(500, r#"{"error":"server_error"}"#);
        assert!(matches!(error, ProviderError::Status { status: 500, .. }));
    }

    #[test]
    fn token_error_excerpt_never_echoes_unknown_bodies() {
        let error = status_error(400, "refresh_token=secret-value");
        let ProviderError::Status { body_excerpt, .. } = error else {
            panic!("expected status error");
        };
        assert!(body_excerpt.is_empty());
    }

    #[test]
    fn decodes_jwt_payload_without_verifying() {
        #[derive(Deserialize)]
        struct Claims {
            exp: i64,
        }

        let payload = URL_SAFE_NO_PAD.encode(r#"{"exp":1700000000}"#);
        let jwt = format!("h.{payload}.s");
        assert_eq!(jwt_claims::<Claims>(&jwt).unwrap().exp, 1_700_000_000);
        assert!(jwt_claims::<Claims>("nodots").is_err());
    }

    fn jwt(payload: &str) -> String {
        format!("h.{}.s", URL_SAFE_NO_PAD.encode(payload))
    }

    #[test]
    fn extracts_chatgpt_account_id_claim() {
        let token = jwt(
            r#"{"email":"a@b.c","https://api.openai.com/auth":{"chatgpt_account_id":"acct-1","chatgpt_plan_type":"pro"}}"#,
        );
        assert_eq!(
            chatgpt_account_id(&token).unwrap().as_deref(),
            Some("acct-1")
        );
        assert_eq!(chatgpt_account_id(&jwt("{}")).unwrap(), None);
    }

    #[test]
    fn reads_access_token_expiry() {
        assert_eq!(
            jwt_expiry(&jwt(r#"{"exp":1700000000}"#)).unwrap(),
            1_700_000_000
        );
        assert!(jwt_expiry(&jwt("{}")).is_err());
    }

    #[test]
    fn expiry_requires_positive_lifetime() {
        let token = |expires_in| TokenResponse {
            access_token: "a".to_owned(),
            refresh_token: None,
            id_token: None,
            expires_in,
        };
        assert_eq!(expires_at(&token(Some(3600)), 100).unwrap(), 3700);
        assert!(expires_at(&token(None), 100).is_err());
        assert!(expires_at(&token(Some(0)), 100).is_err());
    }

    #[test]
    fn form_body_percent_encodes() {
        assert_eq!(
            form_body(&[("redirect_uri", "http://127.0.0.1:1/x"), ("a", "b c")]),
            "redirect_uri=http%3A%2F%2F127.0.0.1%3A1%2Fx&a=b+c"
        );
    }
}
