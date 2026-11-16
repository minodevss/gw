use super::oauth::{
    self, ANTHROPIC_CLIENT_ID, ANTHROPIC_TOKEN_URL, Body, CODEX_CLIENT_ID, CODEX_TOKEN_URL,
    SIWC_RESOURCE, SIWC_TOKEN_URL,
};
use super::{Kind, Method, ProviderError};
use crate::account::Secret;

const REFRESH_MARGIN_SECS: i64 = 300;

pub(crate) fn needs_refresh(kind: Kind, secret: &Secret, now: i64) -> bool {
    match kind.method() {
        Method::Oauth | Method::Siwc | Method::Codex => secret
            .expires_at()
            .is_some_and(|expires_at| expires_at <= now.saturating_add(REFRESH_MARGIN_SECS)),
        Method::SetupToken | Method::Plan | Method::Key | Method::Custom => false,
    }
}

pub(crate) async fn refresh(
    http: &reqwest::Client,
    kind: Kind,
    secret: &Secret,
) -> Result<Secret, ProviderError> {
    let Secret::Oauth {
        refresh,
        account_id,
        client_id,
        ..
    } = secret
    else {
        return Err(ProviderError::Invalid(
            "secret has no refresh token".to_owned(),
        ));
    };
    let result = match kind.method() {
        Method::Oauth => refresh_anthropic(http, ANTHROPIC_TOKEN_URL, refresh).await,
        Method::Siwc => {
            let client_id = client_id
                .as_deref()
                .ok_or_else(|| ProviderError::Invalid("secret has no client_id".to_owned()))?;
            refresh_siwc(http, SIWC_TOKEN_URL, refresh, client_id).await
        }
        Method::Codex => refresh_codex(http, CODEX_TOKEN_URL, refresh, account_id.as_deref()).await,
        Method::SetupToken | Method::Plan | Method::Key | Method::Custom => Err(
            ProviderError::Invalid(format!("{} has no refresh flow", kind.method())),
        ),
    };
    result.map_err(unauthorized_is_expired)
}

// The token endpoints answer 401 for a refresh token they no longer recognize.
// https://github.com/openai/codex/blob/main/codex-rs/login/src/auth/manager.rs
fn unauthorized_is_expired(error: ProviderError) -> ProviderError {
    match error {
        ProviderError::Status { status: 401, .. } => ProviderError::Expired,
        ProviderError::Http(_)
        | ProviderError::Status { .. }
        | ProviderError::Io(_)
        | ProviderError::Json(_)
        | ProviderError::Invalid(_)
        | ProviderError::Expired
        | ProviderError::Cancelled => error,
    }
}

async fn refresh_anthropic(
    http: &reqwest::Client,
    url: &str,
    refresh: &str,
) -> Result<Secret, ProviderError> {
    let body = serde_json::json!({
        "grant_type": "refresh_token",
        "refresh_token": refresh,
        "client_id": ANTHROPIC_CLIENT_ID,
    });
    let text = oauth::post(http, url, Body::Json(&body)).await?;
    let token = oauth::parse_token(&text)?;
    Ok(Secret::Oauth {
        expires_at: oauth::expires_at(&token, oauth::unix_now())?,
        refresh: token.refresh_token.unwrap_or_else(|| refresh.to_owned()),
        access: token.access_token,
        account_id: None,
        client_id: None,
    })
}

async fn refresh_siwc(
    http: &reqwest::Client,
    url: &str,
    refresh: &str,
    client_id: &str,
) -> Result<Secret, ProviderError> {
    let text = oauth::post(
        http,
        url,
        Body::Form(&[
            ("grant_type", "refresh_token"),
            ("client_id", client_id),
            ("refresh_token", refresh),
            ("resource", SIWC_RESOURCE),
        ]),
    )
    .await?;
    let token = oauth::parse_token(&text)?;
    Ok(Secret::Oauth {
        expires_at: oauth::expires_at(&token, oauth::unix_now())?,
        refresh: token.refresh_token.unwrap_or_else(|| refresh.to_owned()),
        access: token.access_token,
        account_id: None,
        client_id: Some(client_id.to_owned()),
    })
}

async fn refresh_codex(
    http: &reqwest::Client,
    url: &str,
    refresh: &str,
    account_id: Option<&str>,
) -> Result<Secret, ProviderError> {
    let body = serde_json::json!({
        "client_id": CODEX_CLIENT_ID,
        "grant_type": "refresh_token",
        "refresh_token": refresh,
    });
    let text = oauth::post(http, url, Body::Json(&body)).await?;
    let token = oauth::parse_token(&text)?;
    let account_id = match token.id_token.as_deref() {
        Some(id_token) => oauth::chatgpt_account_id(id_token)?,
        None => None,
    }
    .or_else(|| account_id.map(str::to_owned));
    Ok(Secret::Oauth {
        expires_at: oauth::jwt_expiry(&token.access_token)?,
        refresh: token.refresh_token.unwrap_or_else(|| refresh.to_owned()),
        access: token.access_token,
        account_id,
        client_id: None,
    })
}

#[cfg(test)]
mod tests {
    use base64::Engine as _;

    use super::*;
    use crate::provider::Provider;
    use crate::provider::oauth::fake::{FakeServer, Reply};

    fn kind(provider: Provider, method: Method) -> Kind {
        Kind::new(provider, method).unwrap()
    }

    fn oauth_secret(expires_at: i64) -> Secret {
        Secret::Oauth {
            access: "old-at".to_owned(),
            refresh: "old-rt".to_owned(),
            expires_at,
            account_id: Some("acct-old".to_owned()),
            client_id: Some("oaiapp_1".to_owned()),
        }
    }

    fn jwt(payload: &str) -> String {
        format!(
            "h.{}.s",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload)
        )
    }

    fn reply(status: u16, body: &str) -> Reply {
        Reply {
            status,
            body: body.to_owned(),
        }
    }

    #[test]
    fn refreshes_five_minutes_before_expiry() {
        let oauth = kind(Provider::Anthropic, Method::Oauth);
        assert!(needs_refresh(oauth, &oauth_secret(1300), 1000));
        assert!(!needs_refresh(oauth, &oauth_secret(1301), 1000));
        assert!(needs_refresh(oauth, &oauth_secret(900), 1000));
    }

    #[test]
    fn tokens_and_keys_never_refresh() {
        let token = Secret::Token {
            token: "t".to_owned(),
            expires_at: Some(0),
        };
        let setup = kind(Provider::Anthropic, Method::SetupToken);
        assert!(!needs_refresh(setup, &token, 1000));
        let key = Secret::Key {
            key: "k".to_owned(),
        };
        assert!(!needs_refresh(
            kind(Provider::OpenAi, Method::Key),
            &key,
            1000
        ));
    }

    #[tokio::test]
    async fn anthropic_refresh_rotates_tokens_as_json() {
        let server = FakeServer::start(vec![reply(
            200,
            r#"{"access_token":"new-at","refresh_token":"new-rt","expires_in":28800}"#,
        )])
        .await;
        let secret = refresh_anthropic(&reqwest::Client::new(), &server.base_url, "old-rt")
            .await
            .unwrap();
        let Secret::Oauth {
            access,
            refresh,
            expires_at,
            ..
        } = secret
        else {
            panic!("expected oauth secret");
        };
        assert_eq!((access.as_str(), refresh.as_str()), ("new-at", "new-rt"));
        assert!(expires_at > oauth::unix_now() + 28000);
        let requests = server.requests().await;
        assert!(requests[0].contains(r#""grant_type":"refresh_token""#));
        assert!(requests[0].contains(r#""refresh_token":"old-rt""#));
        assert!(requests[0].contains(ANTHROPIC_CLIENT_ID));
    }

    #[tokio::test]
    async fn siwc_refresh_sends_issued_client_and_resource_without_scope() {
        let server = FakeServer::start(vec![reply(
            200,
            r#"{"access_token":"new-at","refresh_token":"new-rt","expires_in":3600,"scope":"x"}"#,
        )])
        .await;
        let secret = refresh_siwc(
            &reqwest::Client::new(),
            &server.base_url,
            "old-rt",
            "oaiapp_1",
        )
        .await
        .unwrap();
        let Secret::Oauth { client_id, .. } = secret else {
            panic!("expected oauth secret");
        };
        assert_eq!(client_id.as_deref(), Some("oaiapp_1"));
        let requests = server.requests().await;
        let request = &requests[0];
        assert!(request.contains("content-type: application/x-www-form-urlencoded"));
        assert!(request.contains("grant_type=refresh_token"));
        assert!(request.contains("client_id=oaiapp_1"));
        assert!(request.contains("resource=https%3A%2F%2Fapi.openai.com%2Fv1"));
        assert!(!request.contains("scope="));
    }

    #[tokio::test]
    async fn codex_refresh_reads_expiry_and_keeps_account_id_without_id_token() {
        let access = jwt(r#"{"exp":1900000000}"#);
        let body = format!(r#"{{"access_token":"{access}","refresh_token":"new-rt"}}"#);
        let server = FakeServer::start(vec![reply(200, &body)]).await;
        let secret = refresh_codex(
            &reqwest::Client::new(),
            &server.base_url,
            "old-rt",
            Some("acct-old"),
        )
        .await
        .unwrap();
        let Secret::Oauth {
            refresh,
            expires_at,
            account_id,
            ..
        } = secret
        else {
            panic!("expected oauth secret");
        };
        assert_eq!(refresh, "new-rt");
        assert_eq!(expires_at, 1_900_000_000);
        assert_eq!(account_id.as_deref(), Some("acct-old"));
        let requests = server.requests().await;
        assert!(requests[0].contains(r#""client_id":"app_EMoamEEZ73f0CkXaXp7hrann""#));
    }

    #[tokio::test]
    async fn invalid_grant_and_reuse_map_to_expired() {
        for body in [
            r#"{"error":"invalid_grant"}"#,
            r#"{"error":{"code":"refresh_token_reused"}}"#,
        ] {
            let server = FakeServer::start(vec![reply(400, body)]).await;
            let result = refresh_anthropic(&reqwest::Client::new(), &server.base_url, "rt").await;
            assert!(matches!(result, Err(ProviderError::Expired)));
        }
    }

    #[tokio::test]
    async fn server_errors_stay_status_errors() {
        let server =
            FakeServer::start(vec![reply(503, r#"{"error":"temporarily_unavailable"}"#)]).await;
        let result = refresh_anthropic(&reqwest::Client::new(), &server.base_url, "rt").await;
        assert!(matches!(
            result.map_err(unauthorized_is_expired),
            Err(ProviderError::Status { status: 503, .. })
        ));
    }

    #[test]
    fn unauthorized_means_expired() {
        let error = ProviderError::Status {
            status: 401,
            body_excerpt: String::new(),
        };
        assert!(matches!(
            unauthorized_is_expired(error),
            ProviderError::Expired
        ));
    }

    #[tokio::test]
    async fn refresh_rejects_non_oauth_secret() {
        let key = Secret::Key {
            key: "k".to_owned(),
        };
        let result = refresh(
            &reqwest::Client::new(),
            kind(Provider::Anthropic, Method::Oauth),
            &key,
        )
        .await;
        assert!(matches!(result, Err(ProviderError::Invalid(_))));
    }
}
