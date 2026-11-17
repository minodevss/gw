use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

use crate::account::{Account, Secret, State};
use crate::catalog::{Capabilities, Catalog};
use crate::provider::{Kind, Method, Provider, Scope, Usage};

pub(crate) type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("invalid stored value {0}")]
    Invalid(String),
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS accounts (
    name TEXT PRIMARY KEY,
    provider TEXT NOT NULL,
    method TEXT NOT NULL,
    secret TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'active',
    block_scope TEXT,
    block_reset INTEGER,
    inactive_reason TEXT,
    usage TEXT,
    usage_at INTEGER,
    created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS keys (
    name TEXT PRIMARY KEY,
    hash TEXT NOT NULL UNIQUE,
    created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS catalog (
    account TEXT NOT NULL REFERENCES accounts(name) ON DELETE CASCADE,
    model TEXT NOT NULL,
    caps TEXT NOT NULL,
    fetched_at INTEGER NOT NULL,
    PRIMARY KEY (account, model)
);
CREATE TABLE IF NOT EXISTS log (
    id INTEGER PRIMARY KEY,
    ts INTEGER NOT NULL,
    key TEXT,
    session TEXT,
    api TEXT NOT NULL,
    model TEXT NOT NULL,
    account TEXT,
    reason TEXT NOT NULL,
    status INTEGER NOT NULL,
    error TEXT,
    note TEXT,
    input INTEGER,
    cache_read INTEGER,
    cache_write INTEGER,
    output INTEGER,
    duration_ms INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS log_ts ON log (ts);
CREATE INDEX IF NOT EXISTS log_key ON log (key, ts);
CREATE INDEX IF NOT EXISTS log_account ON log (account, ts);
CREATE TABLE IF NOT EXISTS meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
";

#[derive(Debug)]
pub(crate) struct Store {
    conn: Connection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KeyRow {
    pub(crate) name: String,
    pub(crate) created_at: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LogSummary {
    pub(crate) requests: i64,
    pub(crate) limits: i64,
    pub(crate) errors: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AccountUse {
    pub(crate) account: String,
    pub(crate) requests: i64,
    pub(crate) input: i64,
    pub(crate) cached: i64,
    pub(crate) output: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LogRow {
    pub(crate) id: i64,
    pub(crate) ts: i64,
    pub(crate) key: Option<String>,
    pub(crate) session: Option<String>,
    pub(crate) api: String,
    pub(crate) model: String,
    pub(crate) account: Option<String>,
    pub(crate) reason: String,
    pub(crate) status: u16,
    pub(crate) error: Option<String>,
    pub(crate) note: Option<String>,
    pub(crate) input: Option<i64>,
    pub(crate) cache_read: Option<i64>,
    pub(crate) cache_write: Option<i64>,
    pub(crate) output: Option<i64>,
    pub(crate) duration_ms: i64,
}

impl Store {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    pub(crate) fn accounts(&self) -> Result<Vec<Account>> {
        let mut stmt = self.conn.prepare(
            "SELECT name, provider, method, secret, state, block_scope, block_reset,
                    inactive_reason, usage, usage_at, created_at
             FROM accounts ORDER BY created_at",
        )?;
        let rows = stmt.query_map([], read_account)?;
        rows.map(|row| row.map_err(Error::from).and_then(|r| r))
            .collect()
    }

    pub(crate) fn account(&self, name: &str) -> Result<Option<Account>> {
        self.conn
            .query_row(
                "SELECT name, provider, method, secret, state, block_scope, block_reset,
                        inactive_reason, usage, usage_at, created_at
                 FROM accounts WHERE name = ?1",
                [name],
                read_account,
            )
            .optional()?
            .transpose()
    }

    pub(crate) fn insert_account(&self, account: &Account) -> Result<()> {
        self.conn.execute(
            "INSERT INTO accounts (name, provider, method, secret, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                account.name,
                account.kind.provider().as_str(),
                account.kind.method().as_str(),
                encode_secret(&account.secret)?,
                account.created_at,
            ],
        )?;
        Ok(())
    }

    pub(crate) fn replace_secret(&self, name: &str, secret: &Secret) -> Result<()> {
        self.conn.execute(
            "UPDATE accounts SET secret = ?2, state = 'active', inactive_reason = NULL,
                    block_scope = NULL, block_reset = NULL
             WHERE name = ?1",
            params![name, encode_secret(secret)?],
        )?;
        Ok(())
    }

    pub(crate) fn refresh_secret<E>(
        &mut self,
        name: &str,
        stale: &Secret,
        refresh: impl FnOnce(&Secret) -> std::result::Result<Secret, E>,
    ) -> Result<std::result::Result<Secret, E>> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stored: String = tx.query_row(
            "SELECT secret FROM accounts WHERE name = ?1",
            [name],
            |row| row.get(0),
        )?;
        let current = decode_secret(&stored)?;
        if &current != stale {
            return Ok(Ok(current));
        }
        match refresh(&current) {
            Ok(fresh) => {
                tx.execute(
                    "UPDATE accounts SET secret = ?2 WHERE name = ?1",
                    params![name, encode_secret(&fresh)?],
                )?;
                tx.commit()?;
                Ok(Ok(fresh))
            }
            Err(error) => Ok(Err(error)),
        }
    }

    pub(crate) fn set_state(&self, name: &str, state: &State) -> Result<()> {
        let (label, scope, reset, reason) = match state {
            State::Active => ("active", None, None, None),
            State::Blocked { scope, reset_at } => {
                ("blocked", Some(scope_text(scope)), Some(*reset_at), None)
            }
            State::Inactive { reason } => ("inactive", None, None, Some(reason.as_str())),
        };
        self.conn.execute(
            "UPDATE accounts SET state = ?2, block_scope = ?3, block_reset = ?4,
                    inactive_reason = ?5
             WHERE name = ?1",
            params![name, label, scope, reset, reason],
        )?;
        Ok(())
    }

    pub(crate) fn set_usage(&self, name: &str, usage: &Usage, at: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE accounts SET usage = ?2, usage_at = ?3 WHERE name = ?1",
            params![name, serde_json::to_string(usage)?, at],
        )?;
        Ok(())
    }

    pub(crate) fn rename_account(&mut self, old: &str, new: &str) -> Result<bool> {
        let tx = self.conn.transaction()?;
        tx.pragma_update(None, "defer_foreign_keys", "ON")?;
        let taken: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM accounts WHERE name = ?1)",
            [new],
            |row| row.get(0),
        )?;
        if taken {
            return Err(Error::Invalid(format!("account {new} already exists")));
        }
        let renamed = tx.execute("UPDATE accounts SET name = ?2 WHERE name = ?1", [old, new])?;
        tx.execute(
            "UPDATE catalog SET account = ?2 WHERE account = ?1",
            [old, new],
        )?;
        tx.commit()?;
        Ok(renamed > 0)
    }

    pub(crate) fn remove_account(&self, name: &str) -> Result<bool> {
        Ok(self
            .conn
            .execute("DELETE FROM accounts WHERE name = ?1", [name])?
            > 0)
    }

    pub(crate) fn catalog(&self) -> Result<Catalog> {
        let mut stmt = self
            .conn
            .prepare("SELECT account, model, caps FROM catalog ORDER BY account, model")?;
        let mut rows = stmt.query([])?;
        let mut catalog = Catalog::default();
        while let Some(row) = rows.next()? {
            let account: String = row.get(0)?;
            let model: String = row.get(1)?;
            let caps: String = row.get(2)?;
            catalog.insert(&account, &model, serde_json::from_str(&caps)?);
        }
        Ok(catalog)
    }

    pub(crate) fn replace_catalog(
        &mut self,
        account: &str,
        models: &[(String, Capabilities)],
        at: i64,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM catalog WHERE account = ?1", [account])?;
        for (model, caps) in models {
            tx.execute(
                "INSERT INTO catalog (account, model, caps, fetched_at) VALUES (?1, ?2, ?3, ?4)",
                params![account, model, serde_json::to_string(caps)?, at],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn insert_key(&self, name: &str, hash: &str, at: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO keys (name, hash, created_at) VALUES (?1, ?2, ?3)",
            params![name, hash, at],
        )?;
        Ok(())
    }

    pub(crate) fn key_name(&self, hash: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT name FROM keys WHERE hash = ?1", [hash], |row| {
                row.get(0)
            })
            .optional()?)
    }

    pub(crate) fn keys(&self) -> Result<Vec<KeyRow>> {
        let mut stmt = self
            .conn
            .prepare("SELECT name, created_at FROM keys ORDER BY created_at")?;
        let rows = stmt.query_map([], |row| {
            Ok(KeyRow {
                name: row.get(0)?,
                created_at: row.get(1)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub(crate) fn revoke_key(&self, name: &str) -> Result<bool> {
        Ok(self
            .conn
            .execute("DELETE FROM keys WHERE name = ?1", [name])?
            > 0)
    }

    pub(crate) fn insert_log(&self, row: &LogRow) -> Result<()> {
        self.conn.execute(
            "INSERT INTO log (ts, key, session, api, model, account, reason, status, error, note,
                              input, cache_read, cache_write, output, duration_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                row.ts,
                row.key,
                row.session,
                row.api,
                row.model,
                row.account,
                row.reason,
                row.status,
                row.error,
                row.note,
                row.input,
                row.cache_read,
                row.cache_write,
                row.output,
                row.duration_ms,
            ],
        )?;
        Ok(())
    }

    pub(crate) fn log_after(&self, after_id: i64, limit: u32) -> Result<Vec<LogRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, ts, key, session, api, model, account, reason, status, error, note,
                    input, cache_read, cache_write, output, duration_ms
             FROM (SELECT * FROM log WHERE id > ?1 ORDER BY id DESC LIMIT ?2) ORDER BY id",
        )?;
        let rows = stmt.query_map(params![after_id, limit], read_log)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub(crate) fn meta(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()?)
    }

    pub(crate) fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            [key, value],
        )?;
        Ok(())
    }

    pub(crate) fn log_since(&self, since: i64) -> Result<Vec<LogRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, ts, key, session, api, model, account, reason, status, error, note,
                    input, cache_read, cache_write, output, duration_ms
             FROM log WHERE ts >= ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map([since], read_log)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub(crate) fn log_summary(&self, since: i64) -> Result<LogSummary> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*),
                    COALESCE(SUM(reason = 'limit'), 0),
                    COALESCE(SUM(status >= 400 AND reason != 'limit'), 0)
             FROM log WHERE ts >= ?1",
            [since],
            |row| {
                Ok(LogSummary {
                    requests: row.get(0)?,
                    limits: row.get(1)?,
                    errors: row.get(2)?,
                })
            },
        )?)
    }

    pub(crate) fn account_use_since(&self, since: i64) -> Result<Vec<AccountUse>> {
        let mut stmt = self.conn.prepare(
            "SELECT account, COUNT(*),
                    COALESCE(SUM(COALESCE(input, 0) + COALESCE(cache_read, 0) + COALESCE(cache_write, 0)), 0),
                    COALESCE(SUM(COALESCE(cache_read, 0)), 0),
                    COALESCE(SUM(COALESCE(output, 0)), 0)
             FROM log
             WHERE ts >= ?1 AND account IS NOT NULL AND reason != 'limit'
             GROUP BY account",
        )?;
        let rows = stmt.query_map([since], |row| {
            Ok(AccountUse {
                account: row.get(0)?,
                requests: row.get(1)?,
                input: row.get(2)?,
                cached: row.get(3)?,
                output: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub(crate) fn key_last_used(&self) -> Result<Vec<(String, i64)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT key, MAX(ts) FROM log WHERE key IS NOT NULL GROUP BY key")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub(crate) fn heartbeat(&self, listen: &str, at: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta (key, value) VALUES ('heartbeat', ?1)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            [format!("{at} {listen}")],
        )?;
        Ok(())
    }

    pub(crate) fn last_heartbeat(&self) -> Result<Option<(i64, String)>> {
        let value: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'heartbeat'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        Ok(value.and_then(|value| {
            let (at, listen) = value.split_once(' ')?;
            Some((at.parse().ok()?, listen.to_owned()))
        }))
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum StoredSecret {
    Oauth {
        access: String,
        refresh: String,
        expires_at: i64,
        account_id: Option<String>,
        client_id: Option<String>,
    },
    Token {
        token: String,
        expires_at: Option<i64>,
    },
    Key {
        key: String,
    },
    Custom {
        url: String,
        api: String,
        key: Option<String>,
    },
}

fn encode_secret(secret: &Secret) -> Result<String> {
    let stored = match secret.clone() {
        Secret::Oauth {
            access,
            refresh,
            expires_at,
            account_id,
            client_id,
        } => StoredSecret::Oauth {
            access,
            refresh,
            expires_at,
            account_id,
            client_id,
        },
        Secret::Token { token, expires_at } => StoredSecret::Token { token, expires_at },
        Secret::Key { key } => StoredSecret::Key { key },
        Secret::Custom { url, api, key } => StoredSecret::Custom { url, api, key },
    };
    Ok(serde_json::to_string(&stored)?)
}

fn decode_secret(text: &str) -> Result<Secret> {
    Ok(match serde_json::from_str(text)? {
        StoredSecret::Oauth {
            access,
            refresh,
            expires_at,
            account_id,
            client_id,
        } => Secret::Oauth {
            access,
            refresh,
            expires_at,
            account_id,
            client_id,
        },
        StoredSecret::Token { token, expires_at } => Secret::Token { token, expires_at },
        StoredSecret::Key { key } => Secret::Key { key },
        StoredSecret::Custom { url, api, key } => Secret::Custom { url, api, key },
    })
}

fn scope_text(scope: &Scope) -> String {
    match scope {
        Scope::Account => "account".to_owned(),
        Scope::Family(family) => format!("family:{family}"),
    }
}

fn parse_scope(text: &str) -> Scope {
    text.strip_prefix("family:")
        .map_or(Scope::Account, |family| Scope::Family(family.to_owned()))
}

fn read_account(row: &Row<'_>) -> rusqlite::Result<Result<Account>> {
    let provider: String = row.get(1)?;
    let method: String = row.get(2)?;
    let secret: String = row.get(3)?;
    let state: String = row.get(4)?;
    let block_scope: Option<String> = row.get(5)?;
    let block_reset: Option<i64> = row.get(6)?;
    let inactive_reason: Option<String> = row.get(7)?;
    let usage: Option<String> = row.get(8)?;
    let name: String = row.get(0)?;
    let usage_at: Option<i64> = row.get(9)?;
    let created_at: i64 = row.get(10)?;
    Ok((|| {
        let kind = provider
            .parse::<Provider>()
            .and_then(|p| Kind::new(p, method.parse::<Method>()?))
            .map_err(|e| Error::Invalid(e.to_string()))?;
        let state = match (state.as_str(), block_scope, block_reset) {
            ("blocked", Some(scope), Some(reset_at)) => State::Blocked {
                scope: parse_scope(&scope),
                reset_at,
            },
            ("inactive", _, _) => State::Inactive {
                reason: inactive_reason.unwrap_or_default(),
            },
            ("active", _, _) => State::Active,
            (other, _, _) => return Err(Error::Invalid(format!("state {other}"))),
        };
        Ok(Account {
            name,
            kind,
            secret: decode_secret(&secret)?,
            state,
            usage: usage.as_deref().map(serde_json::from_str).transpose()?,
            usage_at,
            created_at,
        })
    })())
}

fn read_log(row: &Row<'_>) -> rusqlite::Result<LogRow> {
    Ok(LogRow {
        id: row.get(0)?,
        ts: row.get(1)?,
        key: row.get(2)?,
        session: row.get(3)?,
        api: row.get(4)?,
        model: row.get(5)?,
        account: row.get(6)?,
        reason: row.get(7)?,
        status: row.get(8)?,
        error: row.get(9)?,
        note: row.get(10)?,
        input: row.get(11)?,
        cache_read: row.get(12)?,
        cache_write: row.get(13)?,
        output: row.get(14)?,
        duration_ms: row.get(15)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("gw-store-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{n}.db"));
        std::fs::remove_file(&path).ok();
        Store::open(&path).unwrap()
    }

    fn account(name: &str) -> Account {
        Account {
            name: name.to_owned(),
            kind: Kind::new(Provider::OpenAi, Method::Codex).unwrap(),
            secret: Secret::Oauth {
                access: "a1".to_owned(),
                refresh: "r1".to_owned(),
                expires_at: 10,
                account_id: Some("acct".to_owned()),
                client_id: None,
            },
            state: State::Active,
            usage: None,
            usage_at: None,
            created_at: 1,
        }
    }

    #[test]
    fn account_round_trips_with_state() {
        let store = store();
        store.insert_account(&account("a")).unwrap();
        let blocked = State::Blocked {
            scope: Scope::Family("opus".to_owned()),
            reset_at: 99,
        };
        store.set_state("a", &blocked).unwrap();
        let loaded = store.account("a").unwrap().unwrap();
        assert_eq!(loaded.state, blocked);
        assert_eq!(loaded.secret, account("a").secret);
    }

    #[test]
    fn refresh_skips_when_another_writer_already_refreshed() {
        let mut store = store();
        store.insert_account(&account("a")).unwrap();
        let stale = account("a").secret;
        let fresh = Secret::Key {
            key: "new".to_owned(),
        };
        store.replace_secret("a", &fresh).unwrap();
        let mut called = false;
        let result = store
            .refresh_secret::<()>("a", &stale, |_| {
                called = true;
                Ok(stale.clone())
            })
            .unwrap();
        assert!(!called);
        assert_eq!(result, Ok(fresh));
    }

    #[test]
    fn catalog_is_replaced_per_account() {
        let mut store = store();
        store.insert_account(&account("a")).unwrap();
        store
            .replace_catalog("a", &[("m1".to_owned(), Capabilities::default())], 1)
            .unwrap();
        store
            .replace_catalog("a", &[("m2".to_owned(), Capabilities::default())], 2)
            .unwrap();
        let catalog = store.catalog().unwrap();
        assert!(catalog.get("a", "m1").is_none());
        assert!(catalog.get("a", "m2").is_some());
    }

    #[test]
    fn log_after_returns_newest_rows_in_order() {
        let store = store();
        for ts in 1..=5 {
            store
                .insert_log(&LogRow {
                    ts,
                    api: "messages".to_owned(),
                    model: "m".to_owned(),
                    reason: "new".to_owned(),
                    status: 200,
                    ..LogRow::default()
                })
                .unwrap();
        }
        let rows = store.log_after(0, 2).unwrap();
        let ts: Vec<i64> = rows.iter().map(|r| r.ts).collect();
        assert_eq!(ts, [4, 5]);
    }

    #[test]
    fn rename_moves_catalog_and_keeps_log_names() {
        let mut store = store();
        store.insert_account(&account("old")).unwrap();
        store
            .replace_catalog("old", &[("m".to_owned(), Capabilities::default())], 1)
            .unwrap();
        store
            .insert_log(&LogRow {
                account: Some("old".to_owned()),
                ..LogRow::default()
            })
            .unwrap();
        assert!(store.rename_account("old", "new").unwrap());
        assert!(store.account("new").unwrap().is_some());
        assert!(store.catalog().unwrap().get("new", "m").is_some());
        assert_eq!(
            store
                .log_after(0, 10)
                .unwrap()
                .first()
                .and_then(|r| r.account.clone()),
            Some("old".to_owned())
        );
    }

    #[test]
    fn rename_refuses_a_taken_name() {
        let mut store = store();
        store.insert_account(&account("a")).unwrap();
        store.insert_account(&account("b")).unwrap();
        assert!(store.rename_account("a", "b").is_err());
        assert!(store.account("a").unwrap().is_some());
    }

    #[test]
    fn summaries_count_limits_errors_and_tokens() {
        let store = store();
        let row = |reason: &str, status: u16, input: i64| LogRow {
            ts: 100,
            key: Some("k".to_owned()),
            account: Some("a".to_owned()),
            reason: reason.to_owned(),
            status,
            input: Some(input),
            cache_read: Some(5),
            output: Some(2),
            ..LogRow::default()
        };
        store.insert_log(&row("new", 200, 10)).unwrap();
        store.insert_log(&row("limit", 429, 0)).unwrap();
        store.insert_log(&row("new", 502, 10)).unwrap();
        let summary = store.log_summary(0).unwrap();
        assert_eq!(
            (summary.requests, summary.limits, summary.errors),
            (3, 1, 1)
        );
        let used = store.account_use_since(0).unwrap();
        assert_eq!(
            used.first().map(|u| (u.requests, u.input, u.cached)),
            Some((2, 30, 10))
        );
        assert_eq!(store.key_last_used().unwrap(), vec![("k".to_owned(), 100)]);
    }
}
