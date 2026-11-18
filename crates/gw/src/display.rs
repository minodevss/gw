use std::cmp::Ordering;

use crate::account::{Account, State};
use crate::provider::{Billing, Kind, Method, Provider, Scope, Usage, Window};

pub(crate) const PROVIDERS: [Provider; 5] = [
    Provider::Anthropic,
    Provider::OpenAi,
    Provider::Zai,
    Provider::Moonshot,
    Provider::Custom,
];

pub(crate) fn provider_title(provider: Provider) -> &'static str {
    match provider {
        Provider::Anthropic => "Anthropic",
        Provider::OpenAi => "OpenAI",
        Provider::Zai => "Z.ai",
        Provider::Moonshot => "Moonshot",
        Provider::Custom => "Custom",
    }
}

pub(crate) fn kind_label(kind: Kind) -> &'static str {
    match (kind.provider(), kind.method()) {
        (Provider::Anthropic, Method::Oauth) => "Claude subscription",
        (Provider::Anthropic, Method::SetupToken) => "Claude setup token",
        (Provider::OpenAi, Method::Siwc) => "ChatGPT subscription",
        (Provider::OpenAi, Method::Codex) => "ChatGPT subscription (device code)",
        (Provider::Zai, Method::Plan) => "GLM coding plan",
        (Provider::Moonshot, Method::Plan) => "Kimi coding plan",
        (Provider::Custom, _) => "Custom endpoint",
        (
            Provider::Anthropic | Provider::OpenAi | Provider::Zai | Provider::Moonshot,
            Method::Oauth
            | Method::SetupToken
            | Method::Siwc
            | Method::Codex
            | Method::Plan
            | Method::Key
            | Method::Custom,
        ) => match kind.billing() {
            Billing::Api => "API key",
            Billing::Sub => "Subscription",
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Health<'a> {
    Ready,
    Limited { reset_at: i64, scope: &'a Scope },
    SignedOut(&'a str),
}

pub(crate) fn health(account: &Account, now: i64) -> Health<'_> {
    match &account.state {
        State::Active => Health::Ready,
        State::Blocked { reset_at, .. } if *reset_at <= now => Health::Ready,
        State::Blocked { scope, reset_at } => Health::Limited {
            reset_at: *reset_at,
            scope,
        },
        State::Inactive { reason } => Health::SignedOut(reason),
    }
}

pub(crate) fn window_label(label: &str) -> String {
    let (span, family) = label
        .split_once('_')
        .map_or((label, None), |(span, family)| (span, Some(family)));
    let split = span
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(span.len());
    let (count, unit) = span.split_at(split);
    let unit = match (unit, count == "1") {
        ("s", true) => "second",
        ("s", false) => "seconds",
        ("m", true) => "minute",
        ("m", false) => "minutes",
        ("h", true) => "hour",
        ("h", false) => "hours",
        ("d", true) => "day",
        ("d", false) => "days",
        ("mo", true) => "month",
        ("mo", false) => "months",
        _ => return label.to_owned(),
    };
    let duration = format!("{count} {unit}");
    match family {
        Some(family) => format!("{duration} {family}"),
        None => duration,
    }
}

pub(crate) fn tightest(usage: &Usage) -> Option<&Window> {
    usage
        .windows
        .iter()
        .max_by(|a, b| a.used.partial_cmp(&b.used).unwrap_or(Ordering::Equal))
}

pub(crate) fn rank(account: &Account, now: i64) -> u8 {
    match health(account, now) {
        Health::Ready => 0,
        Health::Limited { .. } => 1,
        Health::SignedOut(_) => 2,
    }
}
