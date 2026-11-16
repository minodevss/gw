use http::HeaderMap;
use serde_json::Value;

use super::{Kind, Method, Provider};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Scope {
    Account,
    Family(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Limit {
    pub(crate) scope: Scope,
    pub(crate) reset_at: Option<i64>,
}

impl Scope {
    pub(crate) fn covers(&self, model: &str) -> bool {
        match self {
            Self::Account => true,
            Self::Family(family) => model.contains(family.as_str()),
        }
    }
}

const OPENAI_QUOTA_CODES: [&str; 5] = [
    "insufficient_quota",
    "credit_balance_exhausted",
    "organization_spend_limit_exceeded",
    "project_spend_limit_exceeded",
    "organization_usage_limit_exceeded",
];

const ZAI_QUOTA_CODES: [&str; 8] = [
    "1308", "1310", "1316", "1317", "1318", "1319", "1320", "1321",
];

pub(crate) fn limit_signal(
    kind: Kind,
    status: u16,
    headers: &HeaderMap,
    body: Option<&Value>,
) -> Option<Limit> {
    let error = body.and_then(|body| body.get("error"));
    let text = |key: &str| error.and_then(|e| e.get(key)).and_then(Value::as_str);
    let account = |reset_at| {
        Some(Limit {
            scope: Scope::Account,
            reset_at,
        })
    };
    match (kind.provider(), kind.method()) {
        (Provider::Anthropic, Method::Oauth | Method::SetupToken) => {
            let rejected =
                header(headers, "anthropic-ratelimit-unified-status") == Some("rejected");
            (status == 429 && rejected).then(|| Limit {
                scope: claim_scope(header(
                    headers,
                    "anthropic-ratelimit-unified-representative-claim",
                )),
                reset_at: header(headers, "anthropic-ratelimit-unified-reset")
                    .and_then(|value| value.parse().ok()),
            })
        }
        (Provider::Anthropic, _) => {
            let spend_cap = error
                .and_then(|e| e.pointer("/details/error_code"))
                .and_then(Value::as_str)
                == Some("enforced_spend_limit_reached");
            let usage_cap = text("message")
                .is_some_and(|m| m.starts_with("You have reached your specified API usage limits"));
            ((status == 429 && spend_cap) || (status == 400 && usage_cap))
                .then_some(())
                .and_then(|()| account(None))
        }
        (Provider::OpenAi, Method::Codex) => (status == 429
            && text("type") == Some("usage_limit_reached"))
        .then_some(())
        .and_then(|()| {
            account(
                error
                    .and_then(|e| e.get("resets_at"))
                    .and_then(Value::as_i64),
            )
        }),
        (Provider::OpenAi, Method::Siwc) => (status == 429
            && text("code") == Some("subscription_sharing_usage_limit_exceeded"))
        .then_some(())
        .and_then(|()| account(None)),
        (Provider::OpenAi, _) => text("code")
            .or_else(|| text("type"))
            .is_some_and(|code| OPENAI_QUOTA_CODES.contains(&code))
            .then_some(())
            .and_then(|()| account(None)),
        (Provider::Zai, Method::Plan) => {
            let code = error
                .and_then(|e| e.get("code"))
                .and_then(|code| match code {
                    Value::String(code) => Some(code.clone()),
                    Value::Number(code) => Some(code.to_string()),
                    Value::Null | Value::Bool(_) | Value::Array(_) | Value::Object(_) => None,
                });
            (status == 429 && code.is_some_and(|code| ZAI_QUOTA_CODES.contains(&code.as_str())))
                .then_some(())
                .and_then(|()| account(None))
        }
        (Provider::Zai | Provider::Moonshot | Provider::Custom, _) => None,
    }
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

fn claim_scope(claim: Option<&str>) -> Scope {
    match claim.and_then(|claim| claim.strip_prefix("seven_day_")) {
        Some("overage_included") | None => Scope::Account,
        Some(family) => Scope::Family(family.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use http::HeaderValue;
    use serde_json::json;

    use super::*;

    fn kind(provider: Provider, method: Method) -> Kind {
        Kind::new(provider, method).unwrap()
    }

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, HeaderValue::from_static(value));
        }
        map
    }

    #[test]
    fn claude_plan_limit_reads_reset_and_family_scope() {
        let h = headers(&[
            ("anthropic-ratelimit-unified-status", "rejected"),
            ("anthropic-ratelimit-unified-reset", "1700000000"),
            (
                "anthropic-ratelimit-unified-representative-claim",
                "seven_day_opus",
            ),
        ]);
        let limit = limit_signal(kind(Provider::Anthropic, Method::Oauth), 429, &h, None).unwrap();
        assert_eq!(limit.reset_at, Some(1_700_000_000));
        assert!(limit.scope.covers("claude-opus-5-5"));
        assert!(!limit.scope.covers("claude-sonnet-5-5"));
    }

    #[test]
    fn plain_429_without_unified_headers_is_not_a_limit() {
        let none = limit_signal(
            kind(Provider::Anthropic, Method::Oauth),
            429,
            &HeaderMap::new(),
            None,
        );
        assert_eq!(none, None);
    }

    #[test]
    fn codex_usage_limit_carries_resets_at() {
        let body = json!({"error": {"type": "usage_limit_reached", "resets_at": 1_700_000_100}});
        let limit = limit_signal(
            kind(Provider::OpenAi, Method::Codex),
            429,
            &HeaderMap::new(),
            Some(&body),
        )
        .unwrap();
        assert_eq!(limit.reset_at, Some(1_700_000_100));
    }

    #[test]
    fn siwc_limit_has_no_reset() {
        let body = json!({"error": {"code": "subscription_sharing_usage_limit_exceeded"}});
        let limit = limit_signal(
            kind(Provider::OpenAi, Method::Siwc),
            429,
            &HeaderMap::new(),
            Some(&body),
        )
        .unwrap();
        assert_eq!(limit.reset_at, None);
    }

    #[test]
    fn openai_rate_limit_is_not_quota() {
        let body = json!({"error": {"code": "rate_limit_exceeded", "type": "requests"}});
        assert_eq!(
            limit_signal(
                kind(Provider::OpenAi, Method::Key),
                429,
                &HeaderMap::new(),
                Some(&body)
            ),
            None
        );
    }

    #[test]
    fn zai_quota_codes_accept_string_or_number() {
        let zai = kind(Provider::Zai, Method::Plan);
        let string = json!({"error": {"code": "1310"}});
        let number = json!({"error": {"code": 1308}});
        let transient = json!({"error": {"code": "1302"}});
        assert!(limit_signal(zai, 429, &HeaderMap::new(), Some(&string)).is_some());
        assert!(limit_signal(zai, 429, &HeaderMap::new(), Some(&number)).is_some());
        assert!(limit_signal(zai, 429, &HeaderMap::new(), Some(&transient)).is_none());
    }
}
