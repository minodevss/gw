use std::path::Path;

use super::with_store;
use crate::account::Account;
use crate::catalog::{Catalog, Overrides, view};
use crate::clock;
use crate::provider::{Balance, list_models, query_balance, query_usage};
use crate::select::{self, Choice, NoCandidate, Query, Suffix};
use proto::features::Features;

#[derive(Debug)]
pub(crate) struct Refreshed {
    pub(crate) account: String,
    pub(crate) result: Result<usize, String>,
    pub(crate) balance: Option<Balance>,
}

pub(crate) async fn refresh(
    home: &Path,
    http: &reqwest::Client,
    only: Option<&str>,
) -> anyhow::Result<Vec<Refreshed>> {
    let overrides = load_overrides(home)?;
    let accounts = with_store(home, |store| store.accounts()).await?;
    let mut report = Vec::new();
    for account in accounts
        .into_iter()
        .filter(|a| only.is_none_or(|name| a.name == name))
    {
        report.push(refresh_one(home, http, &overrides, account).await?);
    }
    Ok(report)
}

async fn refresh_one(
    home: &Path,
    http: &reqwest::Client,
    overrides: &Overrides,
    account: Account,
) -> anyhow::Result<Refreshed> {
    let now = clock::now();
    let listed = list_models(http, &account).await;
    let result = match listed {
        Ok(mut models) => {
            overrides.apply(account.kind.provider(), &account.name, &mut models);
            let count = models.len();
            let name = account.name.clone();
            with_store(home, move |store| {
                store.replace_catalog(&name, &models, now)
            })
            .await?;
            Ok(count)
        }
        Err(error) => Err(error.to_string()),
    };
    if let Some(Ok(usage)) = query_usage(http, &account).await {
        let name = account.name.clone();
        with_store(home, move |store| store.set_usage(&name, &usage, now)).await?;
    }
    let balance = query_balance(http, &account).await.and_then(Result::ok);
    Ok(Refreshed {
        account: account.name,
        result,
        balance,
    })
}

fn load_overrides(home: &Path) -> anyhow::Result<Overrides> {
    let path = crate::home::overrides(home);
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(Overrides::parse(&text)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Overrides::default()),
        Err(error) => Err(error.into()),
    }
}

pub(crate) async fn views(home: &Path) -> anyhow::Result<Vec<view::ModelView>> {
    let (accounts, catalog) =
        with_store(home, |store| Ok((store.accounts()?, store.catalog()?))).await?;
    Ok(view::models(&catalog, &accounts))
}

pub(crate) fn next_in(
    accounts: &[Account],
    catalog: &Catalog,
    model: &str,
    now: i64,
) -> Result<Choice, NoCandidate> {
    let features = Features::default();
    let query = Query {
        model,
        suffix: Suffix::Any,
        features: &features,
        pinned: None,
        skip: &[],
        now,
    };
    select::select(query, accounts, catalog)
}

pub(crate) async fn capabilities(
    home: &Path,
    model: &str,
) -> anyhow::Result<Vec<(String, crate::catalog::Capabilities)>> {
    let wanted = model.to_owned();
    let catalog = with_store(home, |store| store.catalog()).await?;
    Ok(catalog
        .models()
        .filter(|(_, id, _)| *id == wanted)
        .map(|(account, _, caps)| (account.to_owned(), caps.clone()))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::{Secret, State};
    use crate::catalog::Capabilities;
    use crate::provider::{Kind, Method, Provider};

    fn account(name: &str, method: Method, created_at: i64) -> Account {
        Account {
            name: name.to_owned(),
            kind: Kind::new(Provider::Anthropic, method).unwrap(),
            secret: Secret::Key { key: String::new() },
            state: State::Active,
            usage: None,
            usage_at: None,
            created_at,
        }
    }

    #[test]
    fn next_prefers_subscriptions_over_api_keys() {
        let accounts = vec![
            account("api", Method::Key, 1),
            account("sub", Method::Oauth, 2),
        ];
        let mut catalog = Catalog::default();
        for a in &accounts {
            catalog.insert(&a.name, "m", Capabilities::default());
        }
        let choice = next_in(&accounts, &catalog, "m", 0).unwrap();
        assert_eq!(choice.account, "sub");
        assert_eq!(
            next_in(&accounts, &catalog, "x", 0),
            Err(NoCandidate::UnknownModel)
        );
    }
}
