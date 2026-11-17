use http::HeaderMap;
use http::header::{AUTHORIZATION, HeaderValue};
use serde::{Deserialize, Deserializer};

use super::oauth;
use super::{Method, Provider, ProviderError, Usage, Window};
use crate::account::Account;

const PLAN_USAGE_URL: &str = "https://api.kimi.ai/coding/v1/usages";
const BALANCE_URL: &str = "https://api.moonshot.ai/v1/users/me/balance";
const SUMMARY_LABEL: &str = "7d";

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Balance {
    pub(crate) available: f64,
    pub(crate) voucher: f64,
    pub(crate) cash: f64,
}

pub(crate) async fn query_usage(
    http: &reqwest::Client,
    account: &Account,
) -> Option<Result<Usage, ProviderError>> {
    match (account.kind.provider(), account.kind.method()) {
        (Provider::Moonshot, Method::Plan) => {
            Some(fetch_usage(http, PLAN_USAGE_URL, account.secret.bearer()).await)
        }
        (
            Provider::Anthropic
            | Provider::OpenAi
            | Provider::Zai
            | Provider::Moonshot
            | Provider::Custom,
            _,
        ) => None,
    }
}

pub(crate) async fn query_balance(
    http: &reqwest::Client,
    account: &Account,
) -> Option<Result<Balance, ProviderError>> {
    match (account.kind.provider(), account.kind.method()) {
        (Provider::Moonshot, Method::Key) => {
            Some(fetch_balance(http, BALANCE_URL, account.secret.bearer()).await)
        }
        (
            Provider::Anthropic
            | Provider::OpenAi
            | Provider::Zai
            | Provider::Moonshot
            | Provider::Custom,
            _,
        ) => None,
    }
}

fn bearer_headers(key: &str) -> Result<HeaderMap, ProviderError> {
    let value = HeaderValue::from_str(&format!("Bearer {key}"))
        .map_err(|_| ProviderError::Invalid("key is not a valid header value".to_owned()))?;
    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, value);
    Ok(headers)
}

async fn fetch_usage(http: &reqwest::Client, url: &str, key: &str) -> Result<Usage, ProviderError> {
    let text = oauth::get_text(http, url, bearer_headers(key)?).await?;
    parse_usage(&text, oauth::unix_now())
}

async fn fetch_balance(
    http: &reqwest::Client,
    url: &str,
    key: &str,
) -> Result<Balance, ProviderError> {
    let text = oauth::get_text(http, url, bearer_headers(key)?).await?;
    parse_balance(&text)
}

fn lenient_f64<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<f64>, D::Error> {
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(match value {
        Some(serde_json::Value::Number(number)) => number.as_f64(),
        Some(serde_json::Value::String(text)) => text.trim().parse().ok(),
        Some(
            serde_json::Value::Null
            | serde_json::Value::Bool(_)
            | serde_json::Value::Array(_)
            | serde_json::Value::Object(_),
        )
        | None => None,
    })
}

#[derive(Deserialize)]
struct UsagePayload {
    #[serde(default)]
    usage: Option<Quota>,
    #[serde(default)]
    limits: Vec<LimitItem>,
}

#[derive(Deserialize)]
struct LimitItem {
    #[serde(default)]
    detail: Option<Quota>,
    #[serde(default)]
    window: Option<Duration>,
    #[serde(flatten)]
    inline: Quota,
}

#[derive(Deserialize, Default)]
struct Duration {
    #[serde(default, deserialize_with = "lenient_f64")]
    duration: Option<f64>,
    #[serde(default, rename = "timeUnit")]
    time_unit: Option<String>,
}

#[derive(Deserialize, Default)]
struct Quota {
    #[serde(default, deserialize_with = "lenient_f64")]
    limit: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    used: Option<f64>,
    #[serde(default, deserialize_with = "lenient_f64")]
    remaining: Option<f64>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default, alias = "resetAt", alias = "reset_time", alias = "resetTime")]
    reset_at: Option<String>,
    #[serde(
        default,
        alias = "resetIn",
        alias = "ttl",
        deserialize_with = "lenient_f64"
    )]
    reset_in: Option<f64>,
}

impl Quota {
    fn fraction_used(&self) -> Option<f64> {
        let limit = self.limit.filter(|limit| *limit > 0.0)?;
        let used = self
            .used
            .or_else(|| self.remaining.map(|remaining| limit - remaining))?;
        Some((used / limit).clamp(0.0, 1.0))
    }

    fn reset_at(&self, now: i64) -> Option<i64> {
        if let Some(text) = &self.reset_at {
            return parse_rfc3339(text);
        }
        let seconds = self.reset_in.filter(|seconds| *seconds > 0.0)?;
        #[expect(
            clippy::cast_possible_truncation,
            reason = "reset_in is a small positive number of seconds"
        )]
        Some(now.saturating_add(seconds as i64))
    }

    fn explicit_label(&self) -> Option<&str> {
        [&self.name, &self.title, &self.scope]
            .into_iter()
            .find_map(|value| value.as_deref().filter(|text| !text.is_empty()))
    }
}

// Field names and fallbacks follow the official client's /usage command.
// https://github.com/MoonshotAI/kimi-cli/blob/main/src/kimi_cli/ui/shell/usage.py
fn parse_usage(body: &str, now: i64) -> Result<Usage, ProviderError> {
    let payload: UsagePayload = serde_json::from_str(body)?;
    let mut windows = Vec::new();
    if let Some(summary) = &payload.usage
        && let Some(used) = summary.fraction_used()
    {
        windows.push(Window {
            label: summary.explicit_label().unwrap_or(SUMMARY_LABEL).to_owned(),
            used,
            reset_at: summary.reset_at(now),
        });
    }
    for item in &payload.limits {
        let quota = item.detail.as_ref().unwrap_or(&item.inline);
        let Some(used) = quota.fraction_used() else {
            continue;
        };
        let label = quota
            .explicit_label()
            .or_else(|| item.inline.explicit_label())
            .map(str::to_owned)
            .or_else(|| item.window.as_ref().and_then(window_label))
            .unwrap_or_else(|| format!("limit-{}", windows.len() + 1));
        windows.push(Window {
            label,
            used,
            reset_at: quota.reset_at(now),
        });
    }
    Ok(Usage { windows })
}

fn window_label(window: &Duration) -> Option<String> {
    let duration = window.duration.filter(|duration| *duration > 0.0)?;
    let unit = window.time_unit.as_deref().unwrap_or_default();
    #[expect(
        clippy::cast_possible_truncation,
        reason = "window durations are small positive numbers"
    )]
    let count = duration as i64;
    let minutes = if unit.contains("SECOND") {
        if count % 60 != 0 {
            return Some(format!("{count}s"));
        }
        count / 60
    } else if unit.contains("MINUTE") {
        count
    } else if unit.contains("HOUR") {
        count.saturating_mul(60)
    } else if unit.contains("DAY") {
        count.saturating_mul(1440)
    } else if unit.contains("WEEK") {
        count.saturating_mul(7 * 1440)
    } else if unit.contains("MONTH") {
        return Some(format!("{count}mo"));
    } else {
        return None;
    };
    Some(match minutes {
        m if m % 1440 == 0 => format!("{}d", m / 1440),
        m if m % 60 == 0 => format!("{}h", m / 60),
        m => format!("{m}m"),
    })
}

fn parse_rfc3339(text: &str) -> Option<i64> {
    let (date, time) = text.split_once(['T', 't', ' '])?;
    let mut date = date.split('-');
    let year: i64 = date.next()?.parse().ok()?;
    let month: i64 = date.next()?.parse().ok()?;
    let day: i64 = date.next()?.parse().ok()?;
    let (clock, offset_secs) = split_offset(time)?;
    let mut clock = clock.split(':');
    let hour: i64 = clock.next()?.parse().ok()?;
    let minute: i64 = clock.next()?.parse().ok()?;
    let second: i64 = clock.next()?.split('.').next()?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let days = days_from_civil(year, month, day);
    Some(days * 86_400 + hour * 3600 + minute * 60 + second - offset_secs)
}

fn split_offset(time: &str) -> Option<(&str, i64)> {
    if let Some(clock) = time.strip_suffix(['Z', 'z']) {
        return Some((clock, 0));
    }
    let position = time.rfind(['+', '-'])?;
    let (clock, offset) = time.split_at(position);
    let sign = if offset.starts_with('-') { -1 } else { 1 };
    let (hours, minutes) = offset.get(1..)?.split_once(':')?;
    let seconds = hours.parse::<i64>().ok()? * 3600 + minutes.parse::<i64>().ok()? * 60;
    Some((clock, sign * seconds))
}

// https://howardhinnant.github.io/date_algorithms.html#days_from_civil
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let shifted_month = (month + 9) % 12;
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[derive(Deserialize)]
struct BalancePayload {
    data: BalanceData,
}

#[derive(Deserialize)]
struct BalanceData {
    #[serde(rename = "available_balance")]
    available: f64,
    #[serde(rename = "voucher_balance")]
    voucher: f64,
    #[serde(rename = "cash_balance")]
    cash: f64,
}

fn parse_balance(body: &str) -> Result<Balance, ProviderError> {
    let payload: BalancePayload = serde_json::from_str(body)?;
    Ok(Balance {
        available: payload.data.available,
        voucher: payload.data.voucher,
        cash: payload.data.cash,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::{Secret, State};
    use crate::provider::Kind;
    use crate::provider::oauth::fake::{FakeServer, Reply};

    const NOW: i64 = 1_766_000_000;

    #[test]
    fn parses_usage_summary_and_window_limits() {
        let body = r#"{
          "usage": {"limit": "100", "used": "25", "remaining": "75", "resetTime": "2025-12-23T05:24:18.443553353Z"},
          "limits": [
            {"window": {"duration": 300, "timeUnit": "TIME_UNIT_MINUTE"},
             "detail": {"limit": "100", "remaining": "40", "resetTime": "2025-12-18T10:00:00Z"}},
            {"window": {"duration": 1, "timeUnit": "TIME_UNIT_DAY"},
             "detail": {"limit": 10, "used": 10, "reset_in": 3600}}
          ]
        }"#;
        let usage = parse_usage(body, NOW).unwrap();
        let labels: Vec<_> = usage.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, ["7d", "5h", "1d"]);
        assert!((usage.windows[0].used - 0.25).abs() < 1e-9);
        assert!((usage.windows[1].used - 0.6).abs() < 1e-9);
        assert!((usage.windows[2].used - 1.0).abs() < 1e-9);
        assert_eq!(usage.windows[0].reset_at, Some(1_766_467_458));
        assert_eq!(usage.windows[1].reset_at, Some(1_766_052_000));
        assert_eq!(usage.windows[2].reset_at, Some(NOW + 3600));
    }

    #[test]
    fn named_limits_use_their_name_and_inline_fields_work() {
        let body = r#"{"limits":[{"name":"Weekly","limit":50,"used":5}]}"#;
        let usage = parse_usage(body, NOW).unwrap();
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].label, "Weekly");
    }

    #[test]
    fn limits_without_a_positive_limit_are_skipped() {
        let body = r#"{"limits":[{"detail":{"limit":0,"used":0}},{"detail":{"name":"x"}}]}"#;
        assert!(parse_usage(body, NOW).unwrap().windows.is_empty());
    }

    #[test]
    fn parses_rfc3339_timestamps() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_rfc3339("2025-12-23T05:24:18.443553353Z"),
            Some(1_766_467_458)
        );
        assert_eq!(
            parse_rfc3339("2025-12-23T14:24:18+09:00"),
            Some(1_766_467_458)
        );
        assert_eq!(
            parse_rfc3339("2000-02-29T00:00:00-01:30"),
            Some(951_782_400 + 5400)
        );
        assert_eq!(parse_rfc3339("garbage"), None);
        assert_eq!(parse_rfc3339("2025-13-01T00:00:00Z"), None);
    }

    #[test]
    fn labels_windows_like_the_other_providers() {
        let label = |duration, unit: &str| {
            window_label(&Duration {
                duration: Some(duration),
                time_unit: Some(unit.to_owned()),
            })
        };
        assert_eq!(label(300.0, "TIME_UNIT_MINUTE").as_deref(), Some("5h"));
        assert_eq!(label(45.0, "TIME_UNIT_MINUTE").as_deref(), Some("45m"));
        assert_eq!(label(2.0, "TIME_UNIT_HOUR").as_deref(), Some("2h"));
        assert_eq!(label(7.0, "TIME_UNIT_DAY").as_deref(), Some("7d"));
        assert_eq!(label(30.0, "TIME_UNIT_SECOND").as_deref(), Some("30s"));
        assert_eq!(label(1.0, "TIME_UNIT_WEEK").as_deref(), Some("7d"));
        assert_eq!(label(1.0, "TIME_UNIT_MONTH").as_deref(), Some("1mo"));
        assert_eq!(label(1.0, "TIME_UNIT_FORTNIGHT"), None);
    }

    #[test]
    fn parses_balance_response() {
        let body = r#"{"code":0,"data":{"available_balance":49.58,"voucher_balance":46.58,"cash_balance":3.0},"scode":"0x0","status":true}"#;
        assert_eq!(
            parse_balance(body).unwrap(),
            Balance {
                available: 49.58,
                voucher: 46.58,
                cash: 3.0
            }
        );
        assert!(parse_balance(r#"{"error":{"message":"x"}}"#).is_err());
    }

    fn account(kind: Kind) -> Account {
        Account {
            name: "a".to_owned(),
            kind,
            secret: Secret::Key {
                key: "sk-test".to_owned(),
            },
            state: State::Active,
            usage: None,
            usage_at: None,
            created_at: 0,
        }
    }

    #[tokio::test]
    async fn only_moonshot_accounts_have_quota_queries() {
        let http = reqwest::Client::new();
        let openai = account(Kind::new(Provider::OpenAi, Method::Key).unwrap());
        assert!(query_usage(&http, &openai).await.is_none());
        assert!(query_balance(&http, &openai).await.is_none());
        let key = account(Kind::new(Provider::Moonshot, Method::Key).unwrap());
        assert!(query_usage(&http, &key).await.is_none());
        let plan = account(Kind::new(Provider::Moonshot, Method::Plan).unwrap());
        assert!(query_balance(&http, &plan).await.is_none());
    }

    #[tokio::test]
    async fn usage_request_sends_bearer_key() {
        let server = FakeServer::start(vec![Reply {
            status: 200,
            body: r#"{"usage":{"limit":10,"used":1}}"#.to_owned(),
        }])
        .await;
        let usage = fetch_usage(
            &reqwest::Client::new(),
            &format!("{}/v1/usages", server.base_url),
            "sk-test",
        )
        .await
        .unwrap();
        assert_eq!(usage.windows.len(), 1);
        let requests = server.requests().await;
        assert!(
            requests[0]
                .to_lowercase()
                .contains("authorization: bearer sk-test")
        );
    }

    #[tokio::test]
    async fn balance_http_error_keeps_status() {
        let server = FakeServer::start(vec![Reply {
            status: 401,
            body: r#"{"error":{"message":"invalid key"}}"#.to_owned(),
        }])
        .await;
        let error = fetch_balance(&reqwest::Client::new(), &server.base_url, "k")
            .await
            .unwrap_err();
        assert!(matches!(error, ProviderError::Status { status: 401, .. }));
    }
}
