use std::path::Path;

use crate::account::Account;
use crate::catalog::Capabilities;
use crate::catalog::view::ModelView;
use crate::clock;
use crate::ops::{self, account::Detail};
use crate::store::{KeyRow, LogRow};

pub(crate) const LOG_PAGE: u32 = 1000;
const DAY: i64 = 86_400;

#[derive(Debug, Clone, Default)]
pub(crate) struct LoadRequest {
    pub(crate) account: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) log_after: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KeyStat {
    pub(crate) name: String,
    pub(crate) created_at: i64,
    pub(crate) requests_24h: usize,
    pub(crate) last_24h: Option<i64>,
}

pub(crate) struct Snapshot {
    pub(crate) now: i64,
    pub(crate) accounts: Vec<Account>,
    pub(crate) detail: Option<(String, Detail)>,
    pub(crate) keys: Vec<KeyStat>,
    pub(crate) models: Vec<ModelView>,
    pub(crate) capabilities: Option<(String, Vec<(String, Capabilities)>)>,
    pub(crate) heartbeat: Option<(i64, String)>,
    pub(crate) log: Vec<LogRow>,
}

pub(crate) async fn load(home: &Path, request: LoadRequest) -> anyhow::Result<Snapshot> {
    let accounts = ops::account::list(home).await?;
    let detail = match request.account {
        Some(name) if accounts.iter().any(|a| a.name == name) => {
            let detail = ops::account::detail(home, &name).await?;
            Some((name, detail))
        }
        Some(_) | None => None,
    };
    let models = ops::model::views(home).await?;
    let capabilities = match request.model {
        Some(id) if models.iter().any(|m| m.id == id) => {
            let caps = ops::model::capabilities(home, &id).await?;
            Some((id, caps))
        }
        Some(_) | None => None,
    };
    let after = request.log_after;
    let now = clock::now();
    let (keys, recent, heartbeat, log) = ops::with_store(home, move |store| {
        Ok((
            store.keys()?,
            store.log_since(now - DAY)?,
            store.last_heartbeat()?,
            store.log_after(after, LOG_PAGE)?,
        ))
    })
    .await?;
    Ok(Snapshot {
        now,
        accounts,
        detail,
        keys: key_stats(keys, &recent),
        models,
        capabilities,
        heartbeat,
        log,
    })
}

pub(crate) fn key_stats(keys: Vec<KeyRow>, recent: &[LogRow]) -> Vec<KeyStat> {
    keys.into_iter()
        .map(|key| {
            let mine: Vec<&LogRow> = recent
                .iter()
                .filter(|row| row.key.as_deref() == Some(key.name.as_str()))
                .collect();
            KeyStat {
                requests_24h: mine.len(),
                last_24h: mine.iter().map(|row| row.ts).max(),
                name: key.name,
                created_at: key.created_at,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(key: &str, ts: i64) -> LogRow {
        LogRow {
            key: Some(key.to_owned()),
            ts,
            ..LogRow::default()
        }
    }

    #[test]
    fn counts_requests_and_last_use_per_key() {
        let keys = vec![
            KeyRow {
                name: "macbook".to_owned(),
                created_at: 1,
            },
            KeyRow {
                name: "desktop".to_owned(),
                created_at: 2,
            },
        ];
        let stats = key_stats(
            keys,
            &[row("macbook", 10), row("macbook", 30), row("other", 5)],
        );
        assert_eq!(
            stats.first().map(|s| (s.requests_24h, s.last_24h)),
            Some((2, Some(30)))
        );
        assert_eq!(
            stats.get(1).map(|s| (s.requests_24h, s.last_24h)),
            Some((0, None))
        );
    }
}
