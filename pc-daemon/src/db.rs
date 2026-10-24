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
pub fn next_node_id(conn: &Connection) -> Result<u16> {
    let max: Option<i64> = conn.query_row("SELECT MAX(id) FROM nodes", [], |r| r.get(0))?;
    let id = max.unwrap_or(0) + 1;
    anyhow::ensure!(id < 0xFFFF, "no node ids left");
    Ok(id as u16)
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
            conn.execute(
                "UPDATE commands SET delivered = 1 WHERE id = (
                    SELECT id FROM commands WHERE node_id = ?1 AND channel = ?2 AND value = ?3 AND delivered = 0
                    ORDER BY id DESC LIMIT 1)",
                params![node_id, channel, value],
            )?;
        }
        // Handled by hub_link (they update live state, not the DB).
        HubMsg::PairRequest { .. } | HubMsg::Heartbeat { .. } => {}
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
