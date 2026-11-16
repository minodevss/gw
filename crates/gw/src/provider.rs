mod error;
mod limit;
mod login;
mod models;
mod oauth;
mod quota;
mod refresh;
mod usage;

use std::fmt;
use std::str::FromStr;

use http::HeaderMap;
use http::header::{AUTHORIZATION, HeaderName, HeaderValue};
use proto::Api;

pub(crate) use error::ProviderError;
pub(crate) use limit::{Limit, Scope, limit_signal};
pub(crate) use login::{LoginSession, Prompt, start_login};
pub(crate) use models::list_models;
pub(crate) use quota::{Balance, query_balance, query_usage};
pub(crate) use refresh::{needs_refresh, refresh};
pub(crate) use usage::{Usage, Window, usage_from_headers};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Provider {
    Anthropic,
    OpenAi,
    Zai,
    Moonshot,
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Method {
    Oauth,
    SetupToken,
    Siwc,
    Codex,
    Plan,
    Key,
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Billing {
    Sub,
    Api,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Kind {
    provider: Provider,
    method: Method,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum KindError {
    #[error("unknown provider {0}")]
    Provider(String),
    #[error("unknown method {0}")]
    Method(String),
    #[error("{provider} does not support method {method}")]
    Pair { provider: Provider, method: Method },
}

impl Provider {
    pub(crate) const ALL: [Self; 5] = [
        Self::OpenAi,
        Self::Anthropic,
        Self::Zai,
        Self::Moonshot,
        Self::Custom,
    ];

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::Zai => "zai",
            Self::Moonshot => "moonshot",
            Self::Custom => "custom",
        }
    }

    pub(crate) fn methods(self) -> &'static [Method] {
        match self {
            Self::Anthropic => &[Method::Oauth, Method::SetupToken, Method::Key],
            Self::OpenAi => &[Method::Siwc, Method::Codex, Method::Key],
            Self::Zai | Self::Moonshot => &[Method::Plan, Method::Key],
            Self::Custom => &[Method::Custom],
        }
    }
}

impl Method {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Oauth => "oauth",
            Self::SetupToken => "setup-token",
            Self::Siwc => "siwc",
            Self::Codex => "codex",
            Self::Plan => "plan",
            Self::Key => "key",
            Self::Custom => "custom",
        }
    }
}

impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Provider {
    type Err = KindError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|provider| provider.as_str() == value)
            .ok_or_else(|| KindError::Provider(value.to_owned()))
    }
}

impl FromStr for Method {
    type Err = KindError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        [
            Self::Oauth,
            Self::SetupToken,
            Self::Siwc,
            Self::Codex,
            Self::Plan,
            Self::Key,
            Self::Custom,
        ]
        .into_iter()
        .find(|method| method.as_str() == value)
        .ok_or_else(|| KindError::Method(value.to_owned()))
    }
}

impl Kind {
    pub(crate) fn new(provider: Provider, method: Method) -> Result<Self, KindError> {
        if provider.methods().contains(&method) {
            Ok(Self { provider, method })
        } else {
            Err(KindError::Pair { provider, method })
        }
    }

    pub(crate) fn provider(self) -> Provider {
        self.provider
    }

    pub(crate) fn method(self) -> Method {
        self.method
    }

    pub(crate) fn billing(self) -> Billing {
        match self.method {
            Method::Key => Billing::Api,
            Method::Oauth
            | Method::SetupToken
            | Method::Siwc
            | Method::Codex
            | Method::Plan
            | Method::Custom => Billing::Sub,
        }
    }

    pub(crate) fn refreshes(self) -> bool {
        matches!(self.method, Method::Oauth | Method::Siwc | Method::Codex)
    }

    pub(crate) fn apis(self) -> &'static [Api] {
        match (self.provider, self.method) {
            (Provider::Anthropic, _) => &[Api::Messages],
            (Provider::OpenAi, Method::Key) => &[Api::Responses, Api::Chat],
            (Provider::OpenAi, _) => &[Api::Responses],
            (Provider::Zai, Method::Plan) => &[Api::Messages, Api::Responses, Api::Chat],
            (Provider::Zai, _) => &[Api::Chat],
            (Provider::Moonshot, _) => &[Api::Messages, Api::Chat],
            (Provider::Custom, _) => &[],
        }
    }

    pub(crate) fn base_url(self) -> &'static str {
        match (self.provider, self.method) {
            (Provider::Anthropic, _) => "https://api.anthropic.com",
            (Provider::OpenAi, Method::Codex) => "https://chatgpt.com/backend-api/codex",
            (Provider::OpenAi, _) => "https://api.openai.com/v1",
            (Provider::Zai, _) => "https://api.z.ai/api",
            (Provider::Moonshot, Method::Plan) => "https://api.kimi.ai/coding",
            (Provider::Moonshot, _) => "https://api.moonshot.ai",
            (Provider::Custom, _) => "",
        }
    }

    pub(crate) fn path(self, api: Api) -> &'static str {
        match (self.provider, self.method, api) {
            (Provider::OpenAi, _, Api::Responses) => "/responses",
            (Provider::OpenAi, _, Api::Chat) => "/chat/completions",
            (Provider::Zai, Method::Plan, Api::Messages)
            | (Provider::Moonshot, Method::Key, Api::Messages) => "/anthropic/v1/messages",
            (Provider::Zai, Method::Plan, Api::Chat) => "/coding/paas/v4/chat/completions",
            (Provider::Zai, _, Api::Responses) => "/v1/responses",
            (Provider::Zai, _, Api::Chat) => "/paas/v4/chat/completions",
            (_, _, api) => api.path(),
        }
    }

    pub(crate) fn models_path(self) -> Option<&'static str> {
        match (self.provider, self.method) {
            (Provider::Anthropic, _) => Some("/v1/models?limit=1000"),
            (Provider::OpenAi, _) => Some("/models"),
            (Provider::Moonshot | Provider::Custom, _) => Some("/v1/models"),
            (Provider::Zai, _) => None,
        }
    }

    pub(crate) fn apply_auth(
        self,
        secret: &str,
        account_id: Option<&str>,
        headers: &mut HeaderMap,
    ) {
        headers.remove(AUTHORIZATION);
        headers.remove(X_API_KEY);
        match (self.provider, self.method) {
            (Provider::Anthropic, Method::Key) => insert(headers, X_API_KEY, secret),
            (Provider::Anthropic, _) => {
                insert(headers, AUTHORIZATION, &format!("Bearer {secret}"));
                merge_beta(headers, OAUTH_BETA);
            }
            (Provider::OpenAi, Method::Codex) => {
                insert(headers, AUTHORIZATION, &format!("Bearer {secret}"));
                if let Some(id) = account_id {
                    insert(headers, CHATGPT_ACCOUNT_ID, id);
                }
            }
            (Provider::Custom, _) if secret.is_empty() => {}
            (Provider::OpenAi | Provider::Zai | Provider::Moonshot | Provider::Custom, _) => {
                insert(headers, AUTHORIZATION, &format!("Bearer {secret}"));
            }
        }
    }
}

const OAUTH_BETA: &str = "oauth-2025-04-20";
const X_API_KEY: HeaderName = HeaderName::from_static("x-api-key");
const ANTHROPIC_BETA: HeaderName = HeaderName::from_static("anthropic-beta");
const CHATGPT_ACCOUNT_ID: HeaderName = HeaderName::from_static("chatgpt-account-id");

fn insert(headers: &mut HeaderMap, name: HeaderName, value: &str) {
    if let Ok(value) = HeaderValue::from_str(value) {
        headers.insert(name, value);
    }
}

fn merge_beta(headers: &mut HeaderMap, beta: &str) {
    let existing = headers
        .get(&ANTHROPIC_BETA)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if existing.split(',').any(|value| value.trim() == beta) {
        return;
    }
    let merged = if existing.is_empty() {
        beta.to_owned()
    } else {
        format!("{existing},{beta}")
    };
    insert(headers, ANTHROPIC_BETA, &merged);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(provider: Provider, method: Method) -> Kind {
        Kind::new(provider, method).unwrap()
    }

    #[test]
    fn rejects_method_from_another_provider() {
        assert!(Kind::new(Provider::Anthropic, Method::Siwc).is_err());
    }

    #[test]
    fn billing_follows_method() {
        assert_eq!(kind(Provider::Zai, Method::Plan).billing(), Billing::Sub);
        assert_eq!(kind(Provider::OpenAi, Method::Key).billing(), Billing::Api);
    }

    #[test]
    fn oauth_auth_merges_beta_without_duplicates() {
        let mut headers = HeaderMap::new();
        headers.insert(
            ANTHROPIC_BETA,
            HeaderValue::from_static("claude-code-20250219"),
        );
        headers.insert(X_API_KEY, HeaderValue::from_static("client-key"));
        let oauth = kind(Provider::Anthropic, Method::Oauth);
        oauth.apply_auth("tok", None, &mut headers);
        oauth.apply_auth("tok", None, &mut headers);
        assert_eq!(
            headers[&ANTHROPIC_BETA],
            "claude-code-20250219,oauth-2025-04-20"
        );
        assert_eq!(headers[AUTHORIZATION], "Bearer tok");
        assert!(headers.get(X_API_KEY).is_none());
    }

    #[test]
    fn codex_auth_sets_account_header() {
        let mut headers = HeaderMap::new();
        kind(Provider::OpenAi, Method::Codex).apply_auth("tok", Some("acct"), &mut headers);
        assert_eq!(headers[CHATGPT_ACCOUNT_ID], "acct");
    }

    #[test]
    fn plan_endpoints_differ_from_key_endpoints() {
        assert_eq!(
            kind(Provider::Zai, Method::Plan).path(Api::Chat),
            "/coding/paas/v4/chat/completions"
        );
        assert_eq!(
            kind(Provider::Zai, Method::Key).path(Api::Chat),
            "/paas/v4/chat/completions"
        );
    }
}
