//! Automation rules: "if node.channel goes above/below a threshold, set an output".
//!
//! Each rule fires once when its condition becomes true (sets `on_value`) and
//! once when it clears (sets `off_value`, if there is one). It doesn't resend
//! the command on every reading.

use anyhow::Result;
use rusqlite::{Connection, params};
use uplink::PcMsg;

use crate::db;

struct Rule {
    id: i64,
    name: String,
    op: String,
    threshold: i32,
    target_node: u16,
    target_channel: u8,
    on_value: i32,
    off_value: Option<i32>,
    active: bool,
}

pub fn on_reading(
    conn: &Connection,
    node_id: u16,
    channel: u8,
    value: i32,
    ts: i64,
) -> Result<Vec<PcMsg>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, op, threshold, target_node, target_channel, on_value, off_value, active
         FROM rules WHERE enabled = 1 AND node_id = ?1 AND channel = ?2",
    )?;
    let rules: Vec<Rule> = stmt
        .query_map(params![node_id, channel], |r| {
            Ok(Rule {
                id: r.get(0)?,
                name: r.get(1)?,
                op: r.get(2)?,
                threshold: r.get(3)?,
                target_node: r.get(4)?,
                target_channel: r.get(5)?,
                on_value: r.get(6)?,
                off_value: r.get(7)?,
                active: r.get::<_, i64>(8)? != 0,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;

    let mut out = Vec::new();
    for rule in rules {
        let cond = match rule.op.as_str() {
            "gt" => value > rule.threshold,
            "lt" => value < rule.threshold,
            _ => false,
        };
        if cond == rule.active {
            continue;
        }
        conn.execute(
            "UPDATE rules SET active = ?2 WHERE id = ?1",
            params![rule.id, cond as i64],
        )?;
        let source = format!("rule:{}", rule.id);
        let send = if cond {
            conn.execute(
                "INSERT INTO alerts (node_id, channel, code, value, message, ts) VALUES (?1, ?2, 100, ?3, ?4, ?5)",
                params![node_id, channel, value, format!("Rule \"{}\" fired", rule.name), ts],
            )?;
            Some(rule.on_value)
        } else {
            rule.off_value
        };
        if let Some(v) = send {
            db::insert_command(conn, rule.target_node, rule.target_channel, v, &source, ts)?;
            out.push(PcMsg::Command {
                node_id: rule.target_node,
                channel: rule.target_channel,
                value: v,
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fires_once_and_clears() {
        let conn = Connection::open_in_memory().unwrap();
        db::init(&conn).unwrap();
        conn.execute(
            "INSERT INTO rules (name, node_id, channel, op, threshold, target_node, target_channel, on_value, off_value)
             VALUES ('hot', 1, 0, 'gt', 30000, 1, 2, 1, 0)",
            [],
        )
        .unwrap();
        assert!(on_reading(&conn, 1, 0, 25_000, 1).unwrap().is_empty());
        let fired = on_reading(&conn, 1, 0, 31_000, 2).unwrap();
        assert_eq!(
            fired,
            vec![PcMsg::Command {
                node_id: 1,
                channel: 2,
                value: 1
            }]
        );
        assert!(on_reading(&conn, 1, 0, 32_000, 3).unwrap().is_empty()); // still hot, no resend
        let cleared = on_reading(&conn, 1, 0, 29_000, 4).unwrap();
        assert_eq!(
            cleared,
            vec![PcMsg::Command {
                node_id: 1,
                channel: 2,
                value: 0
            }]
        );
    }
}
