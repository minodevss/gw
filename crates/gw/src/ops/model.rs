use std::path::Path;

use super::with_store;
use crate::account::Account;
use crate::catalog::{Overrides, view};
use crate::clock;
use crate::provider::{Balance, list_models, query_balance, query_usage};

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
