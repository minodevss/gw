use rusqlite::{OptionalExtension, Transaction, params};

use super::{Result, Store};
use crate::provider::Usage;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Sample {
    pub(crate) account: String,
    pub(crate) label: String,
    pub(crate) at: i64,
    pub(crate) used: f64,
    pub(crate) reset_at: Option<i64>,
}

const SAME: f64 = 1e-9;

pub(super) fn record(tx: &Transaction<'_>, account: &str, usage: &Usage, at: i64) -> Result<()> {
    for window in &usage.windows {
        let last: Option<(f64, Option<i64>)> = tx
            .query_row(
                "SELECT used, reset_at FROM usage_history
                 WHERE account = ?1 AND label = ?2 ORDER BY at DESC, rowid DESC LIMIT 1",
                params![account, window.label],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let unchanged = last.is_some_and(|(used, reset_at)| {
            (used - window.used).abs() < SAME && reset_at == window.reset_at
        });
        if !unchanged {
            tx.execute(
                "INSERT INTO usage_history (account, label, at, used, reset_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![account, window.label, at, window.used, window.reset_at],
            )?;
        }
    }
    Ok(())
}

impl Store {
    pub(crate) fn usage_history(&self, since: i64) -> Result<Vec<Sample>> {
        let mut stmt = self.conn.prepare(
            "SELECT account, label, at, used, reset_at, rowid FROM usage_history WHERE at >= ?1
             UNION ALL
             SELECT h.account, h.label, h.at, h.used, h.reset_at, h.rowid FROM usage_history h
             JOIN (SELECT account, label, MAX(at) AS at FROM usage_history
                   WHERE at < ?1 GROUP BY account, label) p
               ON h.account = p.account AND h.label = p.label AND h.at = p.at
             ORDER BY 1, 2, 3, 6",
        )?;
        let rows = stmt.query_map([since], |row| {
            Ok(Sample {
                account: row.get(0)?,
                label: row.get(1)?,
                at: row.get(2)?,
                used: row.get(3)?,
                reset_at: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub(crate) fn usage_history_start(&self) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row("SELECT MIN(at) FROM usage_history", [], |row| row.get(0))?)
    }
}
