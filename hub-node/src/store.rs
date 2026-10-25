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

    /// Averages of one channel since `since`, in at most `points` buckets.
    pub fn history(
        &self,
        node_id: u16,
        channel: u8,
        since: i64,
        points: usize,
    ) -> Result<Vec<(i64, i32)>> {
        let now = uplink::unix_now();
        let bucket = ((now - since).max(1) / points.max(1) as i64).max(1);
        let mut stmt = self.conn.prepare_cached(
            "SELECT (ts / ?4) * ?4 AS t, CAST(AVG(json_extract(json, '$.value')) AS INTEGER)
             FROM outbox
             WHERE json_extract(json, '$.type') = 'reading'
               AND json_extract(json, '$.node_id') = ?1 AND json_extract(json, '$.channel') = ?2
               AND ts >= ?3
             GROUP BY t ORDER BY t",
        )?;
        let rows = stmt.query_map(params![node_id, channel, since, bucket], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
        let mut v: Vec<(i64, i32)> = rows.collect::<rusqlite::Result<_>>()?;
        // Rounding can make one extra bucket; drop the oldest.
        let extra = v.len().saturating_sub(points);
        v.drain(..extra);
        Ok(v)
    }

    /// Newest alerts, recipe alerts and dropped commands, older than `before`.
    pub fn events(&self, before: Option<i64>, limit: usize) -> Result<Vec<(i64, HubMsg)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, json FROM outbox
             WHERE json_extract(json, '$.type') IN ('alert', 'recipe_alert', 'command_dropped')
               AND id < ?1
             ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![before.unwrap_or(i64::MAX), limit as i64], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, json) = row?;
            if let Ok(m) = serde_json::from_str(&json) {
                out.push((id, m));
            }
        }
        Ok(out)
    }

    /// Each node's newest Announce and each channel's newest reading, so the
    /// phone app has something to show right after the hub restarts.
    pub fn latest(&self) -> Result<Vec<HubMsg>> {
        let mut stmt = self.conn.prepare(
            "SELECT json FROM outbox WHERE id IN (
                SELECT MAX(id) FROM outbox WHERE json_extract(json, '$.type') = 'announce'
                GROUP BY json_extract(json, '$.node_id')
                UNION ALL
                SELECT MAX(id) FROM outbox WHERE json_extract(json, '$.type') = 'reading'
                GROUP BY json_extract(json, '$.node_id'), json_extract(json, '$.channel'))
             ORDER BY id",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            if let Ok(m) = serde_json::from_str(&row?) {
                out.push(m);
            }
        }
        Ok(out)
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
    fn history_events_and_latest() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Store::open(&dir.path().join("hub.db")).unwrap();
        let now = uplink::unix_now();
        for v in [10, 20, 30] {
            s.push(&msg(v)).unwrap();
        }
        s.push(&HubMsg::Alert {
            node_id: 1,
            channel: 0,
            code: 2,
            value: 5,
            ts: now,
        })
        .unwrap();
        let h = s.history(1, 0, now - 3600, 60).unwrap();
        assert_eq!(h.len(), 1, "all in one bucket");
        assert_eq!(h[0].1, 20, "averaged");
        assert!(s.history(1, 1, now - 3600, 60).unwrap().is_empty());
        let ev = s.events(None, 10).unwrap();
        assert_eq!(ev.len(), 1);
        assert!(s.events(Some(ev[0].0), 10).unwrap().is_empty());
        let latest = s.latest().unwrap();
        assert_eq!(latest.len(), 1);
        assert!(matches!(latest[0], HubMsg::Reading { value: 30, .. }));
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
