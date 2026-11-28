use std::path::Path;

use anyhow::{Context as _, bail};

use super::{name, with_store};
use crate::account::{Account, Secret, State};
use crate::clock;
use crate::provider::{Kind, LoginSession, Method, Provider, query_usage, start_login};
use crate::select;
use crate::store::{LogRow, Store};

const SETUP_TOKEN_LIFETIME: i64 = 365 * 86_400;

pub(crate) async fn begin_login(
    home: &Path,
    http: &reqwest::Client,
    kind: Kind,
) -> anyhow::Result<LoginSession> {
    let host = with_store(home, |store| host_id(store)).await?;
    Ok(start_login(http, kind, &host).await?)
}

fn host_id(store: &Store) -> crate::store::Result<String> {
    if let Some(id) = store.meta("host_id")? {
        return Ok(id);
    }
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| crate::store::Error::Invalid(error.to_string()))?;
    let id = format!("urn:uuid:{}", uuid_v4(bytes));
    store.set_meta("host_id", &id)?;
    Ok(id)
}

fn uuid_v4(mut bytes: [u8; 16]) -> String {
    if let Some(byte) = bytes.get_mut(6) {
        *byte = (*byte & 0x0f) | 0x40;
    }
    if let Some(byte) = bytes.get_mut(8) {
        *byte = (*byte & 0x3f) | 0x80;
    }
    let hex: String = bytes
        .iter()
        .flat_map(|byte| [byte >> 4, byte & 0x0f])
        .filter_map(|nibble| char::from_digit(u32::from(nibble), 16))
        .collect();
    let part = |range: std::ops::Range<usize>| hex.get(range).unwrap_or_default().to_owned();
    format!(
        "{}-{}-{}-{}-{}",
        part(0..8),
        part(8..12),
        part(12..16),
        part(16..20),
        part(20..32)
    )
}

pub(crate) fn pasted_secret(kind: Kind, value: String, now: i64) -> Secret {
    match kind.method() {
        Method::SetupToken => Secret::Token {
            token: value,
            expires_at: Some(now + SETUP_TOKEN_LIFETIME),
        },
        Method::Oauth
        | Method::Siwc
        | Method::Codex
        | Method::Plan
        | Method::Key
        | Method::Custom => Secret::Key { key: value },
    }
}

pub(crate) fn needs_paste(kind: Kind) -> bool {
    matches!(
        kind.method(),
        Method::SetupToken | Method::Plan | Method::Key
    )
}

pub(crate) fn default_name(accounts: &[Account], kind: Kind) -> String {
    let prefix = match kind.provider() {
        Provider::Custom => "custom".to_owned(),
        Provider::Anthropic | Provider::OpenAi | Provider::Zai | Provider::Moonshot => {
            format!("{}-{}", kind.provider(), kind.method())
        }
    };
    (1..=accounts.len() + 1)
        .map(|n| format!("{prefix}-{n}"))
        .find(|name| accounts.iter().all(|a| &a.name != name))
        .unwrap_or(prefix)
}

pub(crate) async fn add(
    home: &Path,
    name: String,
    kind: Kind,
    secret: Secret,
) -> anyhow::Result<Account> {
    let account = Account {
        name,
        kind,
        secret,
        state: State::Active,
        usage: None,
        usage_at: None,
        created_at: clock::now(),
    };
    let stored = account.clone();
    with_store(home, move |store| {
        if store.account(&stored.name)?.is_some() {
            return Err(crate::store::Error::Invalid(format!(
                "account {} already exists",
                stored.name
            )));
        }
        store.insert_account(&stored)
    })
    .await?;
    Ok(account)
}

pub(crate) async fn relogin(home: &Path, name: &str, secret: Secret) -> anyhow::Result<()> {
    let name = name.to_owned();
    with_store(home, move |store| store.replace_secret(&name, &secret)).await
}

pub(crate) async fn find(home: &Path, name: &str) -> anyhow::Result<Account> {
    let wanted = name.to_owned();
    with_store(home, move |store| store.account(&wanted))
        .await?
        .with_context(|| format!("no account named {name}"))
}

pub(crate) async fn rename(home: &Path, old: &str, new: &str) -> anyhow::Result<()> {
    let (from, to) = (old.to_owned(), new.to_owned());
    let taken: Vec<String> = with_store(home, |store| store.accounts())
        .await?
        .into_iter()
        .map(|a| a.name)
        .collect();
    name::check(new, &taken)?;
    if !with_store(home, move |store| store.rename_account(&from, &to)).await? {
        bail!("no account named {old}");
    }
    Ok(())
}

pub(crate) async fn refresh_usage(
    home: &Path,
    http: &reqwest::Client,
    name: &str,
) -> anyhow::Result<()> {
    let account = find(home, name).await?;
    let usage = query_usage(http, &account)
        .await
        .context("this account has no usage API")??;
    let wanted = name.to_owned();
    let now = clock::now();
    with_store(home, move |store| store.set_usage(&wanted, &usage, now)).await
}

pub(crate) async fn remove(home: &Path, name: &str) -> anyhow::Result<()> {
    let wanted = name.to_owned();
    if !with_store(home, move |store| store.remove_account(&wanted)).await? {
        bail!("no account named {name}");
    }
    Ok(())
}

pub(crate) async fn list(home: &Path) -> anyhow::Result<Vec<Account>> {
    let mut accounts = with_store(home, |store| store.accounts()).await?;
    let now = clock::now();
    accounts.sort_by(|a, b| {
        problem(a, now)
            .cmp(&problem(b, now))
            .then_with(|| select::order(a, b))
    });
    Ok(accounts)
}

fn problem(account: &Account, now: i64) -> u8 {
    match &account.state {
        State::Active => 0,
        State::Blocked { reset_at, .. } if *reset_at <= now => 0,
        State::Blocked { .. } => 1,
        State::Inactive { .. } => 2,
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Detail {
    pub(crate) models: usize,
    pub(crate) requests_24h: usize,
    pub(crate) input_5h: i64,
    pub(crate) cache_5h: i64,
    pub(crate) output_5h: i64,
    pub(crate) sessions: Vec<(String, String, String, i64)>,
    pub(crate) errors: Vec<(i64, String)>,
}

pub(crate) async fn detail(home: &Path, name: &str) -> anyhow::Result<Detail> {
    let wanted = name.to_owned();
    let now = clock::now();
    let (catalog, rows) = with_store(home, move |store| {
        Ok((store.catalog()?, store.log_since(now - 86_400)?))
    })
    .await?;
    let mine: Vec<&LogRow> = rows
        .iter()
        .filter(|r| r.account.as_deref() == Some(wanted.as_str()))
        .collect();
    let mut detail = Detail {
        models: catalog
            .models()
            .filter(|(account, _, _)| *account == wanted)
            .count(),
        requests_24h: mine.len(),
        ..Detail::default()
    };
    for row in mine.iter().filter(|r| r.ts >= now - 5 * 3600) {
        detail.input_5h += row.input.unwrap_or_default()
            + row.cache_read.unwrap_or_default()
            + row.cache_write.unwrap_or_default();
        detail.cache_5h += row.cache_read.unwrap_or_default();
        detail.output_5h += row.output.unwrap_or_default();
    }
    for row in mine.iter().rev() {
        if let Some(session) = &row.session
            && detail.sessions.len() < 3
            && detail.sessions.iter().all(|(s, ..)| s != session)
        {
            detail.sessions.push((
                session.clone(),
                row.key.clone().unwrap_or_default(),
                row.model.clone(),
                row.ts,
            ));
        }
        if let Some(error) = &row.error
            && detail.errors.len() < 3
        {
            detail.errors.push((row.ts, error.clone()));
        }
    }
    Ok(detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_names_count_up_per_kind() {
        let kind = Kind::new(Provider::Anthropic, Method::Oauth).unwrap();
        let existing = Account {
            name: "anthropic-oauth-1".to_owned(),
            kind,
            secret: Secret::Key { key: String::new() },
            state: State::Active,
            usage: None,
            usage_at: None,
            created_at: 0,
        };
        assert_eq!(default_name(&[existing], kind), "anthropic-oauth-2");
    }

    #[test]
    fn uuid_has_version_and_variant_bits() {
        let id = uuid_v4([0xff; 16]);
        assert_eq!(id.len(), 36);
        assert_eq!(id.get(14..15), Some("4"));
        assert!(matches!(id.get(19..20), Some("b" | "8" | "9" | "a")));
    }
}
