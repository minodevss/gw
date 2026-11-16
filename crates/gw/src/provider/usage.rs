use http::HeaderMap;
use serde::{Deserialize, Serialize};

use super::{Kind, Method, Provider};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Window {
    pub(crate) label: String,
    pub(crate) used: f64,
    pub(crate) reset_at: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Usage {
    pub(crate) windows: Vec<Window>,
}

impl Usage {
    pub(crate) fn earliest_reset(&self) -> Option<i64> {
        self.windows.iter().filter_map(|w| w.reset_at).min()
    }
}

pub(crate) fn usage_from_headers(kind: Kind, headers: &HeaderMap) -> Option<Usage> {
    let windows: Vec<Window> = match (kind.provider(), kind.method()) {
        (Provider::Anthropic, Method::Oauth | Method::SetupToken) => ["5h", "7d"]
            .into_iter()
            .filter_map(|label| {
                let used = number(
                    headers,
                    &format!("anthropic-ratelimit-unified-{label}-utilization"),
                )?;
                Some(Window {
                    label: label.to_owned(),
                    used,
                    reset_at: integer(
                        headers,
                        &format!("anthropic-ratelimit-unified-{label}-reset"),
                    ),
                })
            })
            .collect(),
        (Provider::OpenAi, Method::Codex) => ["primary", "secondary"]
            .into_iter()
            .filter_map(|slot| {
                let percent = number(headers, &format!("x-codex-{slot}-used-percent"))?;
                let minutes = integer(headers, &format!("x-codex-{slot}-window-minutes"));
                Some(Window {
                    label: minutes.map_or_else(|| slot.to_owned(), window_label),
                    used: percent / 100.0,
                    reset_at: integer(headers, &format!("x-codex-{slot}-reset-at")),
                })
            })
            .collect(),
        (
            Provider::Anthropic
            | Provider::OpenAi
            | Provider::Zai
            | Provider::Moonshot
            | Provider::Custom,
            _,
        ) => Vec::new(),
    };
    (!windows.is_empty()).then_some(Usage { windows })
}

fn number(headers: &HeaderMap, name: &str) -> Option<f64> {
    headers.get(name)?.to_str().ok()?.parse().ok()
}

fn integer(headers: &HeaderMap, name: &str) -> Option<i64> {
    headers.get(name)?.to_str().ok()?.parse().ok()
}

fn window_label(minutes: i64) -> String {
    match minutes {
        m if m > 0 && m % 1440 == 0 => format!("{}d", m / 1440),
        m if m > 0 && m % 60 == 0 => format!("{}h", m / 60),
        m => format!("{m}m"),
    }
}

#[cfg(test)]
mod tests {
    use http::HeaderValue;

    use super::*;

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, HeaderValue::from_static(value));
        }
        map
    }

    #[test]
    fn reads_claude_plan_windows() {
        let h = headers(&[
            ("anthropic-ratelimit-unified-5h-utilization", "0.72"),
            ("anthropic-ratelimit-unified-5h-reset", "1700000000"),
            ("anthropic-ratelimit-unified-7d-utilization", "0.31"),
        ]);
        let usage =
            usage_from_headers(Kind::new(Provider::Anthropic, Method::Oauth).unwrap(), &h).unwrap();
        assert_eq!(usage.windows.len(), 2);
        assert_eq!(usage.earliest_reset(), Some(1_700_000_000));
    }

    #[test]
    fn labels_codex_windows_from_minutes() {
        let h = headers(&[
            ("x-codex-primary-used-percent", "18"),
            ("x-codex-primary-window-minutes", "300"),
            ("x-codex-secondary-used-percent", "9"),
            ("x-codex-secondary-window-minutes", "10080"),
        ]);
        let usage =
            usage_from_headers(Kind::new(Provider::OpenAi, Method::Codex).unwrap(), &h).unwrap();
        let labels: Vec<_> = usage.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, ["5h", "7d"]);
    }

    #[test]
    fn key_accounts_have_no_usage() {
        let usage = usage_from_headers(
            Kind::new(Provider::OpenAi, Method::Key).unwrap(),
            &HeaderMap::new(),
        );
        assert_eq!(usage, None);
    }
}
