//! SQLite storage. The schema lives in database/schema.sql.

use std::path::Path;

use anyhow::Result;
use crypto_sharecrate::packet::ChannelKind;
use rusqlite::{Connection, OptionalExtension, params};
use uplink::HubMsg;

use crate::rules;
use uplink::PcMsg;

const SCHEMA: &str = include_str!("../../database/schema.sql");

pub fn open(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    init(&conn)?;
    Ok(conn)
}

pub fn init(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.execute_batch(SCHEMA)?;
    // Older databases had a `delivered` flag instead of `status`.
    let has_status: bool = conn
        .prepare("SELECT 1 FROM pragma_table_info('commands') WHERE name = 'status'")?
        .exists([])?;
    if !has_status {
        conn.execute(
            "ALTER TABLE commands ADD COLUMN status TEXT NOT NULL DEFAULT 'queued'",
            [],
        )?;
    }
    Ok(())
}

pub fn kind_name(kind: i64) -> &'static str {
    match kind {
        0 => "digital_in",
        1 => "digital_out",
        2 => "analog_in",
        3 => "temperature",
        _ => "unknown",
    }
}

/// CRYPTO-REVIEW: the pinned hub key, if a hub is paired.
pub fn paired_hub(conn: &Connection) -> Result<Option<(i64, [u8; 32])>> {
    let row: Option<(i64, String)> = conn
        .query_row(
            "SELECT id, public_key FROM hubs ORDER BY id LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(match row {
        Some((id, k)) => Some((id, uplink::key_from_hex(&k)?)),
        None => None,
    })
}

/// Pins a newly approved hub (replacing any old one: one hub per PC).
pub fn pair_hub(conn: &Connection, key: &[u8; 32], now: i64) -> Result<i64> {
    conn.execute("DELETE FROM hubs", [])?;
    conn.execute(
        "INSERT INTO hubs (id, public_key, paired_at) VALUES (1, ?1, ?2)",
        params![uplink::key_to_hex(key), now],
    )?;
    Ok(1)
}

pub fn hub_last_seq(conn: &Connection) -> Result<i64> {
    Ok(conn
        .query_row("SELECT last_seq FROM hubs LIMIT 1", [], |r| r.get(0))
        .optional()?
        .unwrap_or(0))
}

/// Next free node id (1..=65534).
/// Next node id. Ids are never reused, even after a node is deleted
/// (findings 16/22/31): the counter only goes up, and it also skips past any
/// id that still has readings or rules.
pub fn next_node_id(conn: &Connection) -> Result<u16> {
    let counter: i64 = conn
        .query_row(
            "SELECT value FROM counters WHERE name = 'node_id'",
            [],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(0);
    let used: i64 = conn.query_row(
        "SELECT MAX(m) FROM (
            SELECT MAX(id) AS m FROM nodes
            UNION ALL SELECT MAX(node_id) FROM readings
            UNION ALL SELECT MAX(node_id) FROM rules
            UNION ALL SELECT MAX(target_node) FROM rules)",
        [],
        |r| Ok(r.get::<_, Option<i64>>(0)?.unwrap_or(0)),
    )?;
    let id = counter.max(used) + 1;
    anyhow::ensure!(id < 0xFFFF, "no node ids left");
    conn.execute(
        "INSERT INTO counters (name, value) VALUES ('node_id', ?1)
         ON CONFLICT(name) DO UPDATE SET value = excluded.value",
        [id],
    )?;
    Ok(id as u16)
}

/// Next phone id. Like node ids, never reused.
pub fn next_phone_id(conn: &Connection) -> Result<u16> {
    let counter: i64 = conn
        .query_row(
            "SELECT value FROM counters WHERE name = 'phone_id'",
            [],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(0);
    let used: i64 = conn.query_row("SELECT COALESCE(MAX(id), 0) FROM phones", [], |r| r.get(0))?;
    let id = counter.max(used) + 1;
    anyhow::ensure!(id < 0xFFFF, "no phone ids left");
    conn.execute(
        "INSERT INTO counters (name, value) VALUES ('phone_id', ?1)
         ON CONFLICT(name) DO UPDATE SET value = excluded.value",
        [id],
    )?;
    Ok(id as u16)
}

/// Node and channel names for the hub (the phone app shows them).
pub fn labels(conn: &Connection) -> Result<PcMsg> {
    let nodes = conn
        .prepare("SELECT id, name FROM nodes ORDER BY id")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let channels = conn
        .prepare("SELECT node_id, channel, name FROM channels ORDER BY node_id, channel")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(PcMsg::Labels { nodes, channels })
}

/// Marks the newest matching command with `status`, moving only forward.
fn set_command_status(
    conn: &Connection,
    node_id: u16,
    channel: u8,
    value: i32,
    from: &[&str],
    to: &str,
) -> Result<usize> {
    let from = from
        .iter()
        .map(|s| format!("'{s}'"))
        .collect::<Vec<_>>()
        .join(",");
    Ok(conn.execute(
        &format!(
            "UPDATE commands SET status = ?4 WHERE id = (
                SELECT id FROM commands WHERE node_id = ?1 AND channel = ?2 AND value = ?3
                AND status IN ({from}) ORDER BY id DESC LIMIT 1)"
        ),
        params![node_id, channel, value, to],
    )?)
}

/// Stores one message from the hub. Returns commands that rules want sent.
pub fn store_hub_msg(conn: &Connection, seq: i64, msg: &HubMsg) -> Result<Vec<PcMsg>> {
    let mut out = Vec::new();
    match msg {
        HubMsg::Reading {
            node_id,
            channel,
            value,
            ts,
        } => {
            conn.execute(
                "INSERT INTO readings (node_id, channel, value, ts) VALUES (?1, ?2, ?3, ?4)",
                params![node_id, channel, value, ts],
            )?;
            conn.execute(
                "UPDATE channels SET last_value = ?3, last_ts = ?4 WHERE node_id = ?1 AND channel = ?2",
                params![node_id, channel, value, ts],
            )?;
            conn.execute(
                "UPDATE nodes SET last_seen = ?2 WHERE id = ?1",
                params![node_id, ts],
            )?;
            out.extend(rules::on_reading(conn, *node_id, *channel, *value, *ts)?);
        }
        HubMsg::Announce {
            node_id,
            channels,
            ts,
        } => {
            for (ch, kind) in channels {
                let kind_s = ChannelKind::from_byte(*kind)
                    .map(|k| kind_name(k as i64))
                    .unwrap_or("unknown");
                conn.execute(
                    "INSERT INTO channels (node_id, channel, kind, name) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(node_id, channel) DO UPDATE SET kind = excluded.kind",
                    params![node_id, ch, kind, format!("Ch {ch} ({kind_s})")],
                )?;
            }
            conn.execute(
                "UPDATE nodes SET last_seen = ?2 WHERE id = ?1",
                params![node_id, ts],
            )?;
        }
        HubMsg::Alert {
            node_id,
            channel,
            code,
            value,
            ts,
        } => {
            let text = match *code {
                crypto_sharecrate::packet::alert_code::SENSOR_FAULT => "Sensor fault",
                crypto_sharecrate::packet::alert_code::THRESHOLD => "Threshold crossed on node",
                crypto_sharecrate::packet::alert_code::REBOOTED => "Node rebooted",
                _ => "Node alert",
            };
            conn.execute(
                "INSERT INTO alerts (node_id, channel, code, value, message, ts) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![node_id, channel, code, value, text, ts],
            )?;
        }
        HubMsg::Message { node_id, text, ts } => {
            conn.execute(
                "INSERT INTO messages (node_id, text, ts) VALUES (?1, ?2, ?3)",
                params![node_id, text, ts],
            )?;
        }
        HubMsg::NodeOnline {
            node_id,
            transport,
            ts,
            ..
        } => {
            let t = match transport {
                uplink::Transport::Wifi => "wifi",
                uplink::Transport::Lora => "lora",
            };
            conn.execute(
                "UPDATE nodes SET last_seen = ?2, transport = ?3, sessions = sessions + 1 WHERE id = ?1",
                params![node_id, ts, t],
            )?;
        }
        HubMsg::CommandSent {
            node_id,
            channel,
            value,
            ..
        } => {
            set_command_status(conn, *node_id, *channel, *value, &["queued"], "sent")?;
        }
        // Finding 10: "acked" only when the node itself confirms.
        HubMsg::CommandAcked {
            node_id,
            channel,
            value,
            ts,
        } => {
            set_command_status(
                conn,
                *node_id,
                *channel,
                *value,
                &["queued", "sent"],
                "acked",
            )?;
            conn.execute(
                "UPDATE channels SET last_value = ?3, last_ts = ?4 WHERE node_id = ?1 AND channel = ?2",
                params![node_id, channel, value, ts],
            )?;
        }
        HubMsg::CommandDropped {
            node_id,
            channel,
            value,
            ts,
        } => {
            set_command_status(conn, *node_id, *channel, *value, &["queued"], "dropped")?;
            conn.execute(
                "INSERT INTO alerts (node_id, channel, code, value, message, ts) VALUES (?1, ?2, 101, ?3, ?4, ?5)",
                params![node_id, channel, value, "Command dropped: the hub's queue for this node was full", ts],
            )?;
        }
        HubMsg::ApproveFailed {
            public_key,
            node_id,
            reason,
            ts,
        } => {
            // Undo the node row approve() made, if that node never showed up.
            conn.execute(
                "DELETE FROM nodes WHERE id = ?1 AND public_key = ?2 AND last_seen IS NULL",
                params![node_id, public_key],
            )?;
            conn.execute(
                "INSERT INTO alerts (node_id, code, message, ts) VALUES (NULL, 102, ?1, ?2)",
                params![
                    format!("Pairing refused by the hub: {reason}. Try approving again."),
                    ts
                ],
            )?;
        }
        // Started on the hub by a phone or a recipe; the hub's CommandSent /
        // CommandAcked for it follow and move it along like any other.
        HubMsg::CommandRequested {
            node_id,
            channel,
            value,
            source,
            ts,
        } => insert_command(conn, *node_id, *channel, *value, source, *ts)?,
        HubMsg::RecipeAlert { name, text, ts, .. } => {
            conn.execute(
                "INSERT INTO alerts (node_id, code, message, ts) VALUES (NULL, 103, ?1, ?2)",
                params![format!("Recipe \"{name}\": {text}"), ts],
            )?;
        }
        // Handled by hub_link (they update live state, not the DB).
        HubMsg::PairRequest { .. } | HubMsg::Heartbeat { .. } | HubMsg::ApproveRequested { .. } => {
        }
    }
    conn.execute("UPDATE hubs SET last_seq = MAX(last_seq, ?1)", params![seq])?;
    Ok(out)
}

pub fn insert_command(
    conn: &Connection,
    node_id: u16,
    channel: u8,
    value: i32,
    source: &str,
    ts: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO commands (node_id, channel, value, source, ts) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![node_id, channel, value, source, ts],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        init(&c).unwrap();
        c
    }

    #[test]
    fn node_ids_are_never_reused() {
        let c = conn();
        let a = next_node_id(&c).unwrap();
        c.execute(
            "INSERT INTO nodes (id, public_key, name, paired_at) VALUES (?1, 'k1', 'A', 0)",
            [a],
        )
        .unwrap();
        c.execute("DELETE FROM nodes WHERE id = ?1", [a]).unwrap();
        assert_ne!(next_node_id(&c).unwrap(), a);
    }

    #[test]
    fn command_status_follows_hub_and_node() {
        let c = conn();
        insert_command(&c, 1, 2, 1, "app", 0).unwrap();
        let status = || -> String {
            c.query_row("SELECT status FROM commands", [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(status(), "queued");
        store_hub_msg(
            &c,
            0,
            &HubMsg::CommandSent {
                node_id: 1,
                channel: 2,
                value: 1,
                ts: 0,
            },
        )
        .unwrap();
        assert_eq!(status(), "sent");
        store_hub_msg(
            &c,
            0,
            &HubMsg::CommandAcked {
                node_id: 1,
                channel: 2,
                value: 1,
                ts: 0,
            },
        )
        .unwrap();
        assert_eq!(status(), "acked");
    }

    #[test]
    fn dropped_command_raises_alert() {
        let c = conn();
        insert_command(&c, 1, 2, 0, "app", 0).unwrap();
        store_hub_msg(
            &c,
            0,
            &HubMsg::CommandDropped {
                node_id: 1,
                channel: 2,
                value: 0,
                ts: 0,
            },
        )
        .unwrap();
        let (st, alerts): (String, i64) = c
            .query_row(
                "SELECT (SELECT status FROM commands), (SELECT COUNT(*) FROM alerts WHERE code = 101)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((st.as_str(), alerts), ("dropped", 1));
    }

    #[test]
    fn old_database_gets_status_column() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE commands (id INTEGER PRIMARY KEY, node_id INTEGER NOT NULL, channel INTEGER NOT NULL,
             value INTEGER NOT NULL, source TEXT NOT NULL, ts INTEGER NOT NULL, delivered INTEGER NOT NULL DEFAULT 0);",
        )
        .unwrap();
        init(&c).unwrap();
        insert_command(&c, 1, 1, 1, "app", 0).unwrap();
    }
}
