use std::path::Path;

use crate::account::Account;
use crate::catalog::Catalog;
use crate::catalog::view::{self, ModelView};
use crate::clock;
use crate::ops::{self, model};
use crate::select::{Choice, NoCandidate};
use crate::store::{AccountUse, LogRow, LogSummary};

const LOG_ROWS: u32 = 1000;
const HOUR: i64 = 3600;
const FIVE_HOURS: i64 = 5 * HOUR;
const HEARTBEAT_FRESH_SECS: i64 = 10;

#[derive(Debug, Clone)]
pub(crate) struct KeyInfo {
    pub(crate) name: String,
    pub(crate) created_at: i64,
    pub(crate) last_used: Option<i64>,
}

#[derive(Debug, Clone)]
pub(crate) struct ModelInfo {
    pub(crate) view: ModelView,
    pub(crate) next: Result<Choice, NoCandidate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Server {
    Running(String),
    Stopped,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Snapshot {
    pub(crate) now: i64,
    pub(crate) accounts: Vec<Account>,
    pub(crate) catalog: Catalog,
    pub(crate) models: Vec<ModelInfo>,
    pub(crate) keys: Vec<KeyInfo>,
    pub(crate) log: Vec<LogRow>,
    pub(crate) hour: LogSummary,
    pub(crate) five_hours: Vec<AccountUse>,
    pub(crate) heartbeat: Option<(i64, String)>,
}

impl Snapshot {
    pub(crate) fn server(&self) -> Server {
        match &self.heartbeat {
            Some((at, listen)) if self.now - at <= HEARTBEAT_FRESH_SECS => {
                Server::Running(listen.clone())
            }
            Some(_) | None => Server::Stopped,
        }
    }

    pub(crate) fn base_url(&self) -> String {
        match &self.heartbeat {
            Some((_, listen)) => format!("http://{listen}"),
            None => "http://<OpenGateway address>".to_owned(),
        }
    }

    pub(crate) fn account(&self, name: &str) -> Option<&Account> {
        self.accounts.iter().find(|account| account.name == name)
    }

    pub(crate) fn account_names(&self) -> Vec<String> {
        self.accounts.iter().map(|a| a.name.clone()).collect()
    }
}

pub(crate) async fn load(home: &Path) -> anyhow::Result<Snapshot> {
    let now = clock::now();
    let (accounts, catalog, keys, last_used, log, hour, five_hours, heartbeat) =
        ops::with_store(home, move |store| {
            Ok((
                store.accounts()?,
                store.catalog()?,
                store.keys()?,
                store.key_last_used()?,
                store.log_after(0, LOG_ROWS)?,
                store.log_summary(now - HOUR)?,
                store.account_use_since(now - FIVE_HOURS)?,
                store.last_heartbeat()?,
            ))
        })
        .await?;
    let models = view::models(&catalog, &accounts)
        .into_iter()
        .map(|view| ModelInfo {
            next: model::next_in(&accounts, &catalog, &view.id, now),
            view,
        })
        .collect();
    let keys = keys
        .into_iter()
        .map(|key| KeyInfo {
            last_used: last_used
                .iter()
                .find(|(name, _)| *name == key.name)
                .map(|(_, at)| *at),
            name: key.name,
            created_at: key.created_at,
        })
        .collect();
    Ok(Snapshot {
        now,
        accounts,
        catalog,
        models,
        keys,
        log,
        hour,
        five_hours,
        heartbeat,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_is_stopped_when_the_heartbeat_is_stale_or_missing() {
        let mut snapshot = Snapshot {
            now: 100,
            heartbeat: Some((95, "127.0.0.1:8787".to_owned())),
            ..Snapshot::default()
        };
        assert_eq!(
            snapshot.server(),
            Server::Running("127.0.0.1:8787".to_owned())
        );
        snapshot.heartbeat = Some((89, "127.0.0.1:8787".to_owned()));
        assert_eq!(snapshot.server(), Server::Stopped);
        snapshot.heartbeat = None;
        assert_eq!(snapshot.server(), Server::Stopped);
    }

    #[tokio::test]
    async fn loads_an_empty_database() {
        let dir = std::env::temp_dir().join(format!("opengateway-snapshot-{}", std::process::id()));
        let snapshot = load(&dir).await.unwrap();
        assert_eq!(snapshot.accounts, []);
        assert_eq!(snapshot.server(), Server::Stopped);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
