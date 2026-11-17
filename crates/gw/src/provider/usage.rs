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

const ANTHROPIC_PREFIX: &str = "anthropic-ratelimit-unified-";
const UTILIZATION: &str = "-utilization";
const USED_PERCENT: &str = "-used-percent";

pub(crate) fn usage_from_headers(kind: Kind, headers: &HeaderMap) -> Option<Usage> {
    let mut windows: Vec<Window> = match (kind.provider(), kind.method()) {
        (Provider::Anthropic, Method::Oauth | Method::SetupToken) => anthropic_windows(headers),
        (Provider::OpenAi, Method::Codex) => codex_windows(headers),
        (
            Provider::Anthropic
            | Provider::OpenAi
            | Provider::Zai
            | Provider::Moonshot
            | Provider::Custom,
            _,
        ) => Vec::new(),
    };
    windows.sort_by(|a, b| a.label.cmp(&b.label));
    (!windows.is_empty()).then_some(Usage { windows })
}

// Claude Code reads anthropic-ratelimit-unified-<window>-utilization for every window the plan
// has (5h, 7d, and per-model windows), so every such header is taken as reported. Grace values
// describe a tolerance band, not a usage window.
fn anthropic_windows(headers: &HeaderMap) -> Vec<Window> {
    headers
        .keys()
        .filter_map(|name| {
            name.as_str()
                .strip_prefix(ANTHROPIC_PREFIX)?
                .strip_suffix(UTILIZATION)
        })
        .filter(|window| !window.starts_with("grace"))
        .filter_map(|window| {
            Some(Window {
                label: window.to_owned(),
                used: number(headers, &format!("{ANTHROPIC_PREFIX}{window}{UTILIZATION}"))?,
                reset_at: integer(headers, &format!("{ANTHROPIC_PREFIX}{window}-reset")),
            })
        })
        .collect()
}

// Codex parses x-<limit>-<primary|secondary>-used-percent for the codex limit and for any
// additional metered limit.
// https://github.com/openai/codex/blob/main/codex-rs/codex-api/src/rate_limits.rs
fn codex_windows(headers: &HeaderMap) -> Vec<Window> {
    headers
        .keys()
        .filter_map(|name| name.as_str().strip_prefix("x-")?.strip_suffix(USED_PERCENT))
        .filter_map(|slot| {
            let (limit, _) = slot.rsplit_once('-')?;
            let percent = number(headers, &format!("x-{slot}{USED_PERCENT}"))?;
            let minutes = integer(headers, &format!("x-{slot}-window-minutes"));
            let duration = minutes.map_or_else(|| slot.to_owned(), window_label);
            Some(Window {
                label: if limit == "codex" {
                    duration
                } else {
                    format!("{duration}_{limit}")
                },
                used: percent / 100.0,
                reset_at: integer(headers, &format!("x-{slot}-reset-at")),
            })
        })
        .collect()
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
