use rusqlite::{OptionalExtension, params};

use super::{LogRow, Result, Store, read_log};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Point {
    pub(crate) ts: i64,
    pub(crate) key: Option<String>,
    pub(crate) model: String,
    pub(crate) account: Option<String>,
    pub(crate) limit: bool,
    pub(crate) failed: bool,
    pub(crate) input: i64,
    pub(crate) cache_read: i64,
    pub(crate) cache_write: i64,
    pub(crate) output: i64,
    pub(crate) duration_ms: i64,
}

const COLUMNS: &str = "id, ts, key, session, api, model, account, reason, status, error, note,
                       input, cache_read, cache_write, output, duration_ms";

impl Store {
    pub(crate) fn log_points(&self, since: i64) -> Result<Vec<Point>> {
        self.points("ts >= ?1", since)
    }

    pub(crate) fn limit_points(&self, since: i64) -> Result<Vec<Point>> {
        self.points("ts >= ?1 AND reason = 'limit'", since)
    }

    fn points(&self, filter: &str, since: i64) -> Result<Vec<Point>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT ts, key, model, account, reason = 'limit',
                    status >= 400 OR error IS NOT NULL,
                    COALESCE(input, 0), COALESCE(cache_read, 0), COALESCE(cache_write, 0),
                    COALESCE(output, 0), duration_ms
             FROM log WHERE {filter} ORDER BY id"
        ))?;
        let rows = stmt.query_map([since], |row| {
            Ok(Point {
                ts: row.get(0)?,
                key: row.get(1)?,
                model: row.get(2)?,
                account: row.get(3)?,
                limit: row.get(4)?,
                failed: row.get(5)?,
                input: row.get(6)?,
                cache_read: row.get(7)?,
                cache_write: row.get(8)?,
                output: row.get(9)?,
                duration_ms: row.get(10)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub(crate) fn log_before(&self, before_id: i64, limit: u32) -> Result<Vec<LogRow>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM log WHERE id < ?1 ORDER BY id DESC LIMIT ?2"
        ))?;
        let rows = stmt.query_map(params![before_id, limit], read_log)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub(crate) fn log_for_account(&self, account: &str, limit: u32) -> Result<Vec<LogRow>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM log WHERE account = ?1 ORDER BY ts DESC, id DESC LIMIT ?2"
        ))?;
        let rows = stmt.query_map(params![account, limit], read_log)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub(crate) fn log_row(&self, id: i64) -> Result<Option<LogRow>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM log WHERE id = ?1"),
                [id],
                read_log,
            )
            .optional()?)
    }

    pub(crate) fn log_session(&self, session: &str, limit: u32) -> Result<Vec<LogRow>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM (SELECT * FROM log WHERE session = ?1 ORDER BY id DESC LIMIT ?2)
             ORDER BY id"
        ))?;
        let rows = stmt.query_map(params![session, limit], read_log)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}
