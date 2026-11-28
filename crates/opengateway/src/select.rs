use std::cmp::Ordering;

use proto::features::{Feature, Features};

use crate::account::Account;
use crate::catalog::{Catalog, Support};
use crate::provider::Billing;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Suffix {
    Any,
    Sub,
    Api,
}

pub(crate) fn parse_model(name: &str) -> (&str, Suffix) {
    if let Some(model) = name.strip_suffix(":sub") {
        (model, Suffix::Sub)
    } else if let Some(model) = name.strip_suffix(":api") {
        (model, Suffix::Api)
    } else {
        (name, Suffix::Any)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Reason {
    Reused,
    New,
    Moved { from: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Choice {
    pub(crate) account: String,
    pub(crate) reason: Reason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NoCandidate {
    UnknownModel,
    Limited { reset_at: Option<i64> },
    Unsupported(Feature),
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Query<'a> {
    pub(crate) model: &'a str,
    pub(crate) suffix: Suffix,
    pub(crate) features: &'a Features,
    pub(crate) pinned: Option<&'a str>,
    pub(crate) skip: &'a [String],
    pub(crate) now: i64,
}

pub(crate) fn select(
    query: Query<'_>,
    accounts: &[Account],
    catalog: &Catalog,
) -> Result<Choice, NoCandidate> {
    let serving: Vec<&Account> = accounts
        .iter()
        .filter(|a| catalog.get(&a.name, query.model).is_some())
        .filter(|a| suffix_allows(query.suffix, a.kind.billing()))
        .collect();
    if serving.is_empty() {
        return Err(NoCandidate::UnknownModel);
    }
    let usable: Vec<&Account> = serving
        .iter()
        .copied()
        .filter(|a| a.is_usable_for(query.model, query.now))
        .filter(|a| !query.skip.contains(&a.name))
        .collect();
    if usable.is_empty() {
        return Err(NoCandidate::Limited {
            reset_at: earliest_block_reset(&serving, query.now),
        });
    }
    let mut capable: Vec<&Account> = usable
        .iter()
        .copied()
        .filter(|a| unsupported(query.features, catalog, a, query.model).is_none())
        .collect();
    if capable.is_empty() {
        let feature = usable
            .iter()
            .find_map(|a| unsupported(query.features, catalog, a, query.model));
        return Err(feature.map_or(NoCandidate::UnknownModel, NoCandidate::Unsupported));
    }
    if let Some(pinned) = query.pinned
        && capable.iter().any(|a| a.name == pinned)
    {
        return Ok(Choice {
            account: pinned.to_owned(),
            reason: Reason::Reused,
        });
    }
    capable.sort_by(|a, b| order(a, b));
    let account = capable
        .first()
        .map(|a| a.name.clone())
        .ok_or(NoCandidate::UnknownModel)?;
    let reason = match query.pinned {
        Some(from) => Reason::Moved {
            from: from.to_owned(),
        },
        None => Reason::New,
    };
    Ok(Choice { account, reason })
}

pub(crate) fn order(a: &Account, b: &Account) -> Ordering {
    a.kind
        .billing()
        .cmp(&b.kind.billing())
        .then_with(|| match (a.last_reset(), b.last_reset()) {
            (Some(x), Some(y)) => x.cmp(&y),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        })
        .then_with(|| a.created_at.cmp(&b.created_at))
}

fn suffix_allows(suffix: Suffix, billing: Billing) -> bool {
    match suffix {
        Suffix::Any => true,
        Suffix::Sub => billing == Billing::Sub,
        Suffix::Api => billing == Billing::Api,
    }
}

fn unsupported(
    features: &Features,
    catalog: &Catalog,
    account: &Account,
    model: &str,
) -> Option<Feature> {
    let caps = catalog.get(&account.name, model)?;
    features
        .iter()
        .find(|feature| caps.support(*feature) == Support::No)
}

fn earliest_block_reset(accounts: &[&Account], now: i64) -> Option<i64> {
    accounts
        .iter()
        .filter_map(|a| match &a.state {
            crate::account::State::Blocked { reset_at, .. } if *reset_at > now => Some(*reset_at),
            crate::account::State::Blocked { .. }
            | crate::account::State::Active
            | crate::account::State::Inactive { .. } => None,
        })
        .min()
}

#[cfg(test)]
mod tests {
    use proto::ir::Effort;

    use super::*;
    use crate::account::{Secret, State};
    use crate::catalog::Capabilities;
    use crate::provider::{Kind, Method, Provider, Scope, Usage, Window};

    fn account(name: &str, method: Method, reset: Option<i64>, created_at: i64) -> Account {
        let provider = match method {
            Method::Siwc | Method::Codex => Provider::OpenAi,
            Method::Oauth | Method::SetupToken | Method::Plan | Method::Key | Method::Custom => {
                Provider::Anthropic
            }
        };
        Account {
            name: name.to_owned(),
            kind: Kind::new(provider, method).unwrap(),
            secret: Secret::Key { key: String::new() },
            state: State::Active,
            usage: reset.map(|reset_at| Usage {
                windows: vec![Window {
                    label: "5h".to_owned(),
                    used: 0.5,
                    reset_at: Some(reset_at),
                }],
            }),
            usage_at: None,
            created_at,
        }
    }

    fn catalog(accounts: &[Account], model: &str) -> Catalog {
        let mut catalog = Catalog::default();
        for a in accounts {
            catalog.insert(&a.name, model, Capabilities::default());
        }
        catalog
    }

    fn query<'a>(features: &'a Features, pinned: Option<&'a str>, skip: &'a [String]) -> Query<'a> {
        Query {
            model: "m",
            suffix: Suffix::Any,
            features,
            pinned,
            skip,
            now: 0,
        }
    }

    #[test]
    fn parses_suffix_only_at_the_end() {
        assert_eq!(parse_model("gpt:api"), ("gpt", Suffix::Api));
        assert_eq!(parse_model("qwen3:32b"), ("qwen3:32b", Suffix::Any));
        assert_eq!(parse_model("qwen3:32b:sub"), ("qwen3:32b", Suffix::Sub));
    }

    #[test]
    fn prefers_sub_then_earliest_reset_then_registration() {
        let accounts = vec![
            account("api", Method::Key, Some(1), 0),
            account("late", Method::Oauth, Some(900), 1),
            account("unknown", Method::Oauth, None, 2),
            account("soon", Method::Oauth, Some(100), 3),
        ];
        let features = Features::default();
        let choice = select(
            query(&features, None, &[]),
            &accounts,
            &catalog(&accounts, "m"),
        )
        .unwrap();
        assert_eq!(choice.account, "soon");
        assert_eq!(choice.reason, Reason::New);
    }

    #[test]
    fn keeps_pinned_account_while_usable() {
        let accounts = vec![
            account("soon", Method::Oauth, Some(100), 0),
            account("pinned", Method::Oauth, Some(900), 1),
        ];
        let features = Features::default();
        let choice = select(
            query(&features, Some("pinned"), &[]),
            &accounts,
            &catalog(&accounts, "m"),
        )
        .unwrap();
        assert_eq!(choice.reason, Reason::Reused);
    }

    #[test]
    fn moves_when_pinned_account_is_skipped() {
        let accounts = vec![
            account("pinned", Method::Oauth, Some(100), 0),
            account("other", Method::Oauth, Some(900), 1),
        ];
        let features = Features::default();
        let skip = vec!["pinned".to_owned()];
        let choice = select(
            query(&features, Some("pinned"), &skip),
            &accounts,
            &catalog(&accounts, "m"),
        )
        .unwrap();
        assert_eq!(choice.account, "other");
        assert_eq!(
            choice.reason,
            Reason::Moved {
                from: "pinned".to_owned()
            }
        );
    }

    #[test]
    fn reports_earliest_reset_when_all_blocked() {
        let mut a = account("a", Method::Oauth, None, 0);
        a.state = State::Blocked {
            scope: Scope::Account,
            reset_at: 500,
        };
        let mut b = account("b", Method::Oauth, None, 1);
        b.state = State::Blocked {
            scope: Scope::Account,
            reset_at: 300,
        };
        let accounts = vec![a, b];
        let features = Features::default();
        let err = select(
            query(&features, None, &[]),
            &accounts,
            &catalog(&accounts, "m"),
        )
        .unwrap_err();
        assert_eq!(
            err,
            NoCandidate::Limited {
                reset_at: Some(300)
            }
        );
    }

    #[test]
    fn filters_accounts_known_not_to_support_a_feature() {
        let accounts = vec![
            account("siwc", Method::Siwc, Some(1), 0),
            account("codex", Method::Codex, Some(2), 1),
        ];
        let mut catalog = catalog(&accounts, "m");
        catalog.insert(
            "siwc",
            "m",
            Capabilities {
                unsupported: vec!["max_output_tokens".to_owned()],
                ..Capabilities::default()
            },
        );
        let mut features = Features::default();
        features.insert(Feature::MaxOutputTokens);
        let choice = select(query(&features, None, &[]), &accounts, &catalog).unwrap();
        assert_eq!(choice.account, "codex");
    }

    #[test]
    fn reports_the_unsupported_feature_when_nobody_supports_it() {
        let accounts = vec![account("a", Method::Oauth, None, 0)];
        let mut catalog = Catalog::default();
        catalog.insert(
            "a",
            "m",
            Capabilities {
                efforts: Some(vec!["low".to_owned()]),
                ..Capabilities::default()
            },
        );
        let mut features = Features::default();
        features.insert(Feature::Effort(Effort::Xhigh));
        let err = select(query(&features, None, &[]), &accounts, &catalog).unwrap_err();
        assert_eq!(
            err,
            NoCandidate::Unsupported(Feature::Effort(Effort::Xhigh))
        );
    }

    #[test]
    fn suffix_restricts_billing() {
        let accounts = vec![
            account("sub", Method::Oauth, Some(1), 0),
            account("api", Method::Key, Some(1), 1),
        ];
        let features = Features::default();
        let q = Query {
            suffix: Suffix::Api,
            ..query(&features, None, &[])
        };
        assert_eq!(
            select(q, &accounts, &catalog(&accounts, "m"))
                .unwrap()
                .account,
            "api"
        );
    }
}
