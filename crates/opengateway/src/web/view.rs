use std::cmp::Ordering;

use jiff::Timestamp;

use super::chart::{self, Svg, Tone, Zone};
use super::segment;
use crate::account::Account;
use crate::catalog::Catalog;
use crate::display::{Health, health, kind_label, rank, window_label};
use crate::format;
use crate::provider::{Billing, Method, Provider, Scope, Window};
use crate::select::{self, Choice, NoCandidate};

pub(super) const STALE_SECS: i64 = 3600;

pub(super) struct Stamp {
    pub(super) local: String,
    pub(super) ago: String,
    pub(super) iso: String,
}

impl Stamp {
    pub(super) fn new(at: i64, now: i64, zone: &Zone) -> Self {
        Self {
            local: zone.clock(now).when(at),
            ago: format::ago(now - at),
            iso: Timestamp::from_second(at).map_or_else(|_| String::new(), |at| at.to_string()),
        }
    }

    pub(super) fn until(at: i64, now: i64, zone: &Zone) -> Self {
        Self {
            local: zone.clock(now).when(at),
            ago: format!("in {}", format::remaining(at - now)),
            iso: Timestamp::from_second(at).map_or_else(|_| String::new(), |at| at.to_string()),
        }
    }
}

pub(super) struct StateView {
    pub(super) text: String,
    pub(super) tone: &'static str,
    pub(super) fix: Option<Fix>,
}

pub(super) struct Fix {
    pub(super) label: &'static str,
    pub(super) action: String,
}

pub(super) fn state(account: &Account, serves: bool, now: i64, zone: &Zone) -> StateView {
    match health(account, now) {
        Health::Ready if !serves => StateView {
            text: "Serves no models".to_owned(),
            tone: "warn",
            fix: None,
        },
        Health::Ready => StateView {
            text: "Ready".to_owned(),
            tone: "ok",
            fix: None,
        },
        Health::Limited { reset_at, scope } => {
            let until = Stamp::until(reset_at, now, zone);
            let what = match scope {
                Scope::Account => "The provider reported a limit".to_owned(),
                Scope::Family(family) => {
                    format!("The provider reported a limit for {family} models")
                }
            };
            StateView {
                text: format!("{what}, usable again {} ({})", until.local, until.ago),
                tone: "warn",
                fix: None,
            }
        }
        Health::SignedOut(reason) => StateView {
            text: if reason.is_empty() {
                "Signed out".to_owned()
            } else {
                format!("Signed out: {reason}")
            },
            tone: "bad",
            fix: match account.kind.method() {
                Method::Custom => None,
                Method::Oauth
                | Method::Siwc
                | Method::Codex
                | Method::SetupToken
                | Method::Plan
                | Method::Key => Some(Fix {
                    label: "Sign in again",
                    action: format!("/admin/account/{}#sign-in", segment(&account.name)),
                }),
            },
        },
    }
}

pub(super) fn span_secs(label: &str) -> Option<i64> {
    let span = label.split_once('_').map_or(label, |(span, _)| span);
    let split = span.find(|c: char| !c.is_ascii_digit())?;
    let (count, unit) = span.split_at(split);
    let count: i64 = count.parse().ok()?;
    let unit = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86_400,
        "mo" => 30 * 86_400,
        _ => return None,
    };
    Some(count * unit)
}

pub(super) fn span_of(label: &str) -> &str {
    label.split_once('_').map_or(label, |(span, _)| span)
}

pub(super) fn window_order(a: &str, b: &str) -> Ordering {
    let key = |label: &str| (span_secs(label).unwrap_or(i64::MAX), label.contains('_'));
    key(a).cmp(&key(b)).then_with(|| a.cmp(b))
}

pub(super) struct WindowRow {
    pub(super) used: Option<f64>,
    pub(super) label: String,
    pub(super) percent: String,
    pub(super) meter: Svg,
    pub(super) when: String,
    pub(super) passed: bool,
}

pub(super) fn windows(account: &Account, now: i64, zone: &Zone) -> Vec<WindowRow> {
    let mut windows: Vec<&Window> = account
        .usage
        .iter()
        .flat_map(|usage| &usage.windows)
        .collect();
    windows.sort_by(|a, b| window_order(&a.label, &b.label));
    let limited = matches!(health(account, now), Health::Limited { .. });
    windows
        .into_iter()
        .map(|window| {
            let passed = window.reset_at.is_some_and(|at| at <= now);
            let label = window_label(&window.label);
            let tone = if passed {
                Tone::Stale
            } else if window.used >= 1.0 || (limited && window.used >= 0.95) {
                Tone::Full
            } else if window.used >= 0.8 {
                Tone::High
            } else {
                Tone::Normal
            };
            let percent = format::percent(window.used);
            WindowRow {
                used: (!passed).then_some(window.used),
                meter: if passed {
                    chart::meter(0.0, tone, &format!("{label}: no reading since its reset"))
                } else {
                    chart::meter(window.used, tone, &format!("{label}: {percent}% used"))
                },
                percent: if passed {
                    "—".to_owned()
                } else {
                    format!("{percent}%")
                },
                when: match window.reset_at {
                    Some(_) if passed => "reset passed, no reading since".to_owned(),
                    Some(at) => {
                        let until = Stamp::until(at, now, zone);
                        format!("resets {} · {}", until.local, until.ago)
                    }
                    None => String::new(),
                },
                passed,
                label,
            }
        })
        .collect()
}

pub(super) struct AccountRow {
    pub(super) name: String,
    pub(super) href: String,
    pub(super) kind: &'static str,
    pub(super) billing: &'static str,
    pub(super) state: StateView,
    pub(super) windows: Vec<WindowRow>,
    pub(super) read: Option<Stamp>,
    pub(super) stale: bool,
    pub(super) tight: Option<usize>,
}

pub(super) fn serves(catalog: &Catalog, account: &Account) -> bool {
    catalog.models().any(|(name, _, _)| name == account.name)
}

impl AccountRow {
    pub(super) fn tight_window(&self) -> Option<&WindowRow> {
        self.tight.and_then(|index| self.windows.get(index))
    }
}

fn tightest(rows: &[WindowRow]) -> Option<usize> {
    rows.iter()
        .enumerate()
        .filter_map(|(index, row)| row.used.map(|used| (index, used)))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(index, _)| index)
}

pub(super) fn account_row(account: &Account, serves: bool, now: i64, zone: &Zone) -> AccountRow {
    let rows = windows(account, now, zone);
    AccountRow {
        name: account.name.clone(),
        href: format!("/admin/account/{}", segment(&account.name)),
        kind: kind_label(account.kind),
        billing: match account.kind.billing() {
            Billing::Sub => "subscription",
            Billing::Api => "pay as you go",
        },
        state: state(account, serves, now, zone),
        tight: tightest(&rows),
        windows: rows,
        read: account.usage_at.map(|at| Stamp::new(at, now, zone)),
        stale: account.usage_at.is_some_and(|at| now - at > STALE_SECS),
    }
}

pub(super) fn sorted(accounts: &[Account], provider: Provider, now: i64) -> Vec<&Account> {
    let mut mine: Vec<&Account> = accounts
        .iter()
        .filter(|account| account.kind.provider() == provider)
        .collect();
    mine.sort_by(|a, b| {
        rank(a, now)
            .cmp(&rank(b, now))
            .then_with(|| select::order(a, b))
    });
    mine
}

pub(super) struct Pool {
    pub(super) label: String,
    pub(super) text: String,
    pub(super) count: usize,
}

pub(super) fn pools(accounts: &[Account], provider: Provider, now: i64, zone: &Zone) -> Vec<Pool> {
    let mut labels: Vec<String> = accounts
        .iter()
        .filter(|account| account.kind.provider() == provider)
        .flat_map(|account| account.usage.iter().flat_map(|usage| &usage.windows))
        .filter(|window| !window.label.contains('_'))
        .map(|window| window.label.clone())
        .collect();
    labels.sort_by(|a, b| window_order(a, b));
    labels.dedup();
    labels
        .into_iter()
        .map(|label| Pool {
            text: pool_text(accounts, provider, &label, now, zone),
            count: accounts
                .iter()
                .filter(|account| account.kind.provider() == provider)
                .filter(|account| {
                    account
                        .usage
                        .iter()
                        .flat_map(|usage| &usage.windows)
                        .any(|window| window.label == label)
                })
                .count(),
            label: window_label(&label),
        })
        .collect()
}

pub(super) fn pool_text(
    accounts: &[Account],
    provider: Provider,
    label: &str,
    now: i64,
    zone: &Zone,
) -> String {
    let mut left = 0.0;
    let mut known = 0;
    let mut unknown = 0;
    let mut out = 0;
    let mut next_reset: Option<i64> = None;
    for account in accounts.iter().filter(|a| a.kind.provider() == provider) {
        let Some(window) = account
            .usage
            .iter()
            .flat_map(|usage| &usage.windows)
            .find(|window| window.label == label)
        else {
            continue;
        };
        if matches!(health(account, now), Health::SignedOut(_)) {
            out += 1;
            continue;
        }
        match window.reset_at {
            Some(at) if at <= now => unknown += 1,
            Some(at) => {
                known += 1;
                left += (1.0 - window.used).clamp(0.0, 1.0);
                next_reset = Some(next_reset.map_or(at, |next| next.min(at)));
            }
            None => {
                known += 1;
                left += (1.0 - window.used).clamp(0.0, 1.0);
            }
        }
    }
    let mut parts = vec![if known == 0 {
        "no current reading".to_owned()
    } else {
        format!(
            "{left:.1} of {} left",
            format::noun(i64::from(known), "account", "accounts")
        )
    }];
    if unknown > 0 {
        parts.push(format!("{unknown} not read since its reset"));
    }
    if out > 0 {
        parts.push(format!("{out} signed out"));
    }
    if let Some(at) = next_reset.filter(|_| known > 1) {
        parts.push(format!("next reset {}", zone.clock(now).when(at)));
    }
    parts.join(", ")
}

pub(super) fn route(result: Result<Choice, NoCandidate>, now: i64, zone: &Zone) -> (String, bool) {
    match result {
        Ok(choice) => (choice.account, false),
        Err(NoCandidate::Limited { reset_at: Some(at) }) => {
            let until = Stamp::until(at, now, zone);
            (
                format!(
                    "Every account is limited until {} ({})",
                    until.local, until.ago
                ),
                true,
            )
        }
        Err(NoCandidate::Limited { reset_at: None }) => {
            ("Every account is limited".to_owned(), true)
        }
        Err(NoCandidate::UnknownModel | NoCandidate::Unsupported(_)) => {
            ("No account serves it".to_owned(), true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_order_short_windows_first_and_families_after() {
        assert_eq!(span_secs("5h"), Some(18_000));
        assert_eq!(span_secs("7d_opus"), Some(604_800));
        assert_eq!(span_secs("limit-1"), None);
        let mut labels = vec!["7d_opus", "7d", "5h", "7d_fable", "weird"];
        labels.sort_by(|a, b| window_order(a, b));
        assert_eq!(labels, ["5h", "7d", "7d_fable", "7d_opus", "weird"]);
        assert_eq!(span_of("7d_fable"), "7d");
    }
}
