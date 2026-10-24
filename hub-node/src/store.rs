//! The hub's database: SQLite in RAM, copied to the SD card once a day.
//!
//! SD cards wear out from lots of small writes, so readings only go to RAM.
//! Once per `flush_interval_secs` (default 24 h), and on clean shutdown, the
//! whole DB is written to `hub.db` in one go (`VACUUM INTO` a temp file, then
//! an atomic rename). Rows older than 30 days are deleted first.
//!
//! Trade-off: if power is cut, readings since the last flush that hadn't
//! reached the PC yet are lost. Anything already forwarded is safe on the PC.
//! Pairings are NOT kept here; they go to `paired.json` right away (hub.rs).

use std::path::{Path, PathBuf};

use anyhow::Result;
use rusqlite::{Connection, DatabaseName, params};
use uplink::HubMsg;

const SCHEMA: &str = include_str!("../../database/hub_schema.sql");
pub const RETENTION_SECS: i64 = 30 * 86_400;

pub struct Store {
    conn: Connection,
    path: PathBuf,
    last_id: i64,
}

fn unix_micros() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as i64)
        .unwrap_or(0)
}

impl Store {
    /// Loads the last flushed copy from `path` (if any) into RAM.
    pub fn open(path: &Path) -> Result<Self> {
        let mut conn = Connection::open_in_memory()?;
        if path.exists() {
            conn.restore(
                DatabaseName::Main,
                path,
                None::<fn(rusqlite::backup::Progress)>,
            )?;
        }
        conn.execute_batch(SCHEMA)?;
        let last_id: i64 =
            conn.query_row("SELECT COALESCE(MAX(id), 0) FROM outbox", [], |r| r.get(0))?;
        let mut s = Self {
            conn,
            path: path.to_path_buf(),
            last_id,
        };
        s.prune(uplink::unix_now())?;
        Ok(s)
    }

    /// Stores a message for the PC and returns its id.
    ///
    /// Ids are unix microseconds (bumped if needed so they always go up). The
    /// PC skips ids it already has, so ids must keep increasing across reboots
    /// too. A per-boot counter would start over after a power cut.
    pub fn push(&mut self, msg: &HubMsg) -> Result<i64> {
        let id = (self.last_id + 1).max(unix_micros());
        self.conn.execute(
            "INSERT INTO outbox (id, ts, json) VALUES (?1, ?2, ?3)",
            params![id, uplink::unix_now(), serde_json::to_string(msg)?],
        )?;
        self.last_id = id;
        Ok(id)
    }

    /// Up to `limit` messages not yet acked by the PC, with id > `after`.
    pub fn unsent(&self, after: i64, limit: usize) -> Result<Vec<(i64, HubMsg)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, json FROM outbox WHERE forwarded = 0 AND id > ?1 ORDER BY id LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![after, limit as i64], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, json) = row?;
            match serde_json::from_str(&json) {
                Ok(m) => out.push((id, m)),
                Err(e) => tracing::warn!("dropping bad outbox row {id}: {e}"),
            }
        }
        Ok(out)
    }

    pub fn mark_forwarded(&mut self, upto: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE outbox SET forwarded = 1 WHERE forwarded = 0 AND id <= ?1",
            [upto],
        )?;
        Ok(())
    }

    pub fn backlog(&self) -> Result<u64> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM outbox WHERE forwarded = 0", [], |r| {
                r.get::<_, i64>(0)
            })? as u64)
    }

    pub fn prune(&mut self, now: i64) -> Result<usize> {
        Ok(self
            .conn
            .execute("DELETE FROM outbox WHERE ts < ?1", [now - RETENTION_SECS])?)
    }

    /// Writes the whole DB to flash in one go. The only time the hub writes
    /// readings to the SD card.
    pub fn flush(&mut self) -> Result<()> {
        self.prune(uplink::unix_now())?;
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("db.tmp");
        let _ = std::fs::remove_file(&tmp);
        self.conn
            .execute("VACUUM INTO ?1", [tmp.to_string_lossy().as_ref()])?;
        std::fs::File::open(&tmp)?.sync_all()?;
        std::fs::rename(&tmp, &self.path)?;
        tracing::info!("flushed hub DB to {}", self.path.display());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(v: i32) -> HubMsg {
        HubMsg::Reading {
            node_id: 1,
            channel: 0,
            value: v,
            ts: uplink::unix_now(),
        }
    }

    #[test]
    fn flush_restore_and_ack() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hub.db");
        let mut s = Store::open(&path).unwrap();
        let a = s.push(&msg(1)).unwrap();
        let b = s.push(&msg(2)).unwrap();
        assert!(b > a);
        s.mark_forwarded(a).unwrap();
        assert_eq!(s.backlog().unwrap(), 1);
        s.flush().unwrap();
        drop(s);

        let mut s = Store::open(&path).unwrap();
        let unsent = s.unsent(0, 10).unwrap();
        assert_eq!(unsent, vec![(b, msg(2))]);
        assert!(
            s.push(&msg(3)).unwrap() > b,
            "ids keep increasing after restore"
        );
    }

    #[test]
    fn prunes_old_rows() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Store::open(&dir.path().join("hub.db")).unwrap();
        s.push(&msg(1)).unwrap();
        s.conn
            .execute("UPDATE outbox SET ts = ts - ?1", [RETENTION_SECS + 10])
            .unwrap();
        s.push(&msg(2)).unwrap();
        assert_eq!(s.prune(uplink::unix_now()).unwrap(), 1);
        assert_eq!(s.backlog().unwrap(), 1);
    }
}
