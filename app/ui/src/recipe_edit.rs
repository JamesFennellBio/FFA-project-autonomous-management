//! Recipe <-> editor conversions, in Rust so they can be tested:
//! - Blockly: recipe JSON <-> Blockly's serialized workspace JSON. The JS side
//!   (vendor/blockly-shim.js) only defines the blocks and draws them; it never
//!   decides what a recipe means. Same block types and JSON as the old
//!   desktop and phone editors, so recipes made there still open here.
//! - The simple "When ___ is above ___, turn ___ on" builder (Boomer Mode).
//!
//! The hub checks every recipe again before saving it (hub-node/src/recipes.rs).

use serde_json::{Value, json};

use crate::model::{Action, Cond, EditorChannel, Op, RecipeBody};

fn split_key(k: &str) -> Option<(u16, u8)> {
    let (n, c) = k.split_once(':')?;
    Some((n.parse().ok()?, c.parse().ok()?))
}

fn find(chans: &[EditorChannel], node: u16, channel: u8) -> Option<&EditorChannel> {
    chans
        .iter()
        .find(|c| c.node == node && c.channel == channel)
}

/// A saved recipe may use a channel no node reports any more; it still needs
/// a dropdown entry, or Blockly would drop the value.
fn remember(chans: &mut Vec<EditorChannel>, node: u16, channel: u8, output: bool) {
    if find(chans, node, channel).is_none() {
        chans.push(EditorChannel {
            node,
            channel,
            label: format!("Node {node} ch {channel} (not reporting)"),
            scale: 1.0,
            unit: String::new(),
            output,
        });
    }
}

// ---- recipe -> Blockly ----

/// The workspace to load: one "when / do / otherwise" block holding `body`
/// (or empty). Adds dropdown entries for channels that aren't reporting.
pub fn workspace_state(body: Option<&RecipeBody>, chans: &mut Vec<EditorChannel>) -> Value {
    let mut top = json!({ "type": "farmlab_recipe", "x": 20, "y": 20, "inputs": {} });
    if let Some(b) = body {
        top["inputs"]["WHEN"] = json!({ "block": cond_state(&b.when, chans) });
        if let Some(t) = actions_state(&b.then, chans) {
            top["inputs"]["THEN"] = json!({ "block": t });
        }
        if let Some(o) = actions_state(&b.otherwise, chans) {
            top["inputs"]["ELSE"] = json!({ "block": o });
        }
    }
    json!({ "blocks": { "languageVersion": 0, "blocks": [top] } })
}

fn cond_state(c: &Cond, chans: &mut Vec<EditorChannel>) -> Value {
    match c {
        Cond::Compare {
            node,
            channel,
            op,
            value,
        } => {
            remember(chans, *node, *channel, false);
            let scale = find(chans, *node, *channel).map_or(1.0, |c| c.scale);
            json!({ "type": "farmlab_compare", "fields": {
                "SENSOR": format!("{node}:{channel}"), "OP": op.code(), "VALUE": *value as f64 / scale
            }})
        }
        Cond::And { all } => chain(all, "farmlab_and", chans),
        Cond::Or { any } => chain(any, "farmlab_or", chans),
        Cond::Not { cond } => {
            json!({ "type": "farmlab_not", "inputs": { "A": { "block": cond_state(cond, chans) } } })
        }
    }
}

fn chain(list: &[Cond], ty: &str, chans: &mut Vec<EditorChannel>) -> Value {
    match list {
        [] => json!(null),
        [one] => cond_state(one, chans),
        [first, rest @ ..] => json!({ "type": ty, "inputs": {
            "A": { "block": cond_state(first, chans) },
            "B": { "block": chain(rest, ty, chans) },
        }}),
    }
}

fn actions_state(list: &[Action], chans: &mut Vec<EditorChannel>) -> Option<Value> {
    let mut next: Option<Value> = None;
    for a in list.iter().rev() {
        let mut s = match a {
            Action::Set {
                node,
                channel,
                value,
            } => {
                remember(chans, *node, *channel, true);
                json!({ "type": "farmlab_set", "fields": {
                    "OUTPUT": format!("{node}:{channel}"), "STATE": if *value != 0 { "1" } else { "0" }
                }})
            }
            Action::Alert { text } => {
                json!({ "type": "farmlab_alert", "fields": { "TEXT": text } })
            }
        };
        if let Some(n) = next {
            s["next"] = json!({ "block": n });
        }
        next = Some(s);
    }
    next
}

// ---- Blockly -> recipe ----

/// The recipe the blocks describe, or what to fix, in plain words.
pub fn read_workspace(state: &Value, chans: &[EditorChannel]) -> Result<RecipeBody, String> {
    let top = state["blocks"]["blocks"]
        .as_array()
        .and_then(|b| b.iter().find(|b| b["type"] == "farmlab_recipe"))
        .ok_or("The when / do / otherwise block is missing.")?;
    let when = cond(input(top, "WHEN"), chans)?;
    let then = actions(input(top, "THEN"))?;
    let otherwise = actions(input(top, "ELSE"))?;
    if then.is_empty() && otherwise.is_empty() {
        return Err("Add something to 'do' or 'otherwise'.".into());
    }
    Ok(RecipeBody {
        when,
        then,
        otherwise,
    })
}

fn input<'a>(b: &'a Value, name: &str) -> Option<&'a Value> {
    b["inputs"][name]["block"]
        .as_object()
        .map(|_| &b["inputs"][name]["block"])
}

fn picked(b: &Value, field: &str) -> Result<(u16, u8), String> {
    b["fields"][field]
        .as_str()
        .and_then(split_key)
        .ok_or_else(|| "Pick a sensor or output in every block.".into())
}

fn cond(b: Option<&Value>, chans: &[EditorChannel]) -> Result<Cond, String> {
    let b = b.ok_or("Fill every empty slot in the 'when' part.")?;
    match b["type"].as_str() {
        Some("farmlab_compare") => {
            let (node, channel) = picked(b, "SENSOR")?;
            let op = b["fields"]["OP"]
                .as_str()
                .and_then(Op::from_code)
                .ok_or("Pick a comparison in every block.")?;
            let shown = b["fields"]["VALUE"]
                .as_f64()
                .or_else(|| b["fields"]["VALUE"].as_str().and_then(|s| s.parse().ok()))
                .ok_or("Type a number in every comparison.")?;
            let scale = find(chans, node, channel).map_or(1.0, |c| c.scale);
            Ok(Cond::Compare {
                node,
                channel,
                op,
                value: (shown * scale).round() as i64,
            })
        }
        Some(ty @ ("farmlab_and" | "farmlab_or")) => {
            let is_and = ty == "farmlab_and";
            let mut parts = Vec::new();
            for name in ["A", "B"] {
                // and(and(a, b), c) is saved as and(a, b, c): less nesting for
                // the hub's depth limit.
                match (cond(input(b, name), chans)?, is_and) {
                    (Cond::And { all }, true) => parts.extend(all),
                    (Cond::Or { any }, false) => parts.extend(any),
                    (c, _) => parts.push(c),
                }
            }
            Ok(if is_and {
                Cond::And { all: parts }
            } else {
                Cond::Or { any: parts }
            })
        }
        Some("farmlab_not") => Ok(Cond::Not {
            cond: Box::new(cond(input(b, "A"), chans)?),
        }),
        _ => Err("Unknown block in the 'when' part.".into()),
    }
}

fn actions(first: Option<&Value>) -> Result<Vec<Action>, String> {
    let mut out = Vec::new();
    let mut b = first;
    while let Some(blk) = b {
        match blk["type"].as_str() {
            Some("farmlab_set") => {
                let (node, channel) = picked(blk, "OUTPUT")?;
                out.push(Action::Set {
                    node,
                    channel,
                    value: i64::from(blk["fields"]["STATE"] == "1"),
                });
            }
            Some("farmlab_alert") => {
                let text = blk["fields"]["TEXT"]
                    .as_str()
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if text.is_empty() {
                    return Err("Type the alert's message.".into());
                }
                out.push(Action::Alert { text });
            }
            _ => {}
        }
        b = blk["next"]["block"]
            .as_object()
            .map(|_| &blk["next"]["block"]);
    }
    Ok(out)
}

// ---- the simple builder ----

/// "When <sensor> is above/below <value>, turn <output> on/off
/// (and the opposite otherwise)", plus an optional alert. Produces ordinary
/// recipe JSON.
#[derive(Debug, Clone, PartialEq)]
pub struct SimpleRule {
    pub sensor: (u16, u8),
    pub above: bool,
    /// In display units (°C, V, 0/1).
    pub value: f64,
    pub output: Option<(u16, u8)>,
    pub turn_on: bool,
    /// Do the opposite when the condition stops being true.
    pub undo_otherwise: bool,
    /// Also send this alert (empty = none).
    pub alert: String,
}

impl SimpleRule {
    pub fn to_body(&self, chans: &[EditorChannel]) -> Result<RecipeBody, String> {
        let (node, channel) = self.sensor;
        let scale = find(chans, node, channel).map_or(1.0, |c| c.scale);
        let when = Cond::Compare {
            node,
            channel,
            op: if self.above { Op::Gt } else { Op::Lt },
            value: (self.value * scale).round() as i64,
        };
        let mut then = Vec::new();
        let mut otherwise = Vec::new();
        if let Some((node, channel)) = self.output {
            then.push(Action::Set {
                node,
                channel,
                value: i64::from(self.turn_on),
            });
            if self.undo_otherwise {
                otherwise.push(Action::Set {
                    node,
                    channel,
                    value: i64::from(!self.turn_on),
                });
            }
        }
        let text = self.alert.trim();
        if !text.is_empty() {
            then.push(Action::Alert { text: text.into() });
        }
        if then.is_empty() {
            return Err("Pick something to switch, or type an alert.".into());
        }
        Ok(RecipeBody {
            when,
            then,
            otherwise,
        })
    }

    /// The same rule back, if `body` has this simple shape (otherwise the
    /// recipe opens in the block editor).
    pub fn from_body(body: &RecipeBody, chans: &[EditorChannel]) -> Option<SimpleRule> {
        let Cond::Compare {
            node,
            channel,
            op,
            value,
        } = body.when
        else {
            return None;
        };
        let above = match op {
            Op::Gt => true,
            Op::Lt => false,
            _ => return None,
        };
        let mut output = None;
        let mut turn_on = true;
        let mut alert = String::new();
        for a in &body.then {
            match a {
                Action::Set {
                    node,
                    channel,
                    value,
                } if output.is_none() => {
                    output = Some((*node, *channel));
                    turn_on = *value != 0;
                }
                Action::Alert { text } if alert.is_empty() => alert = text.clone(),
                _ => return None,
            }
        }
        let undo_otherwise = match (body.otherwise.as_slice(), output) {
            ([], _) => false,
            (
                [
                    Action::Set {
                        node,
                        channel,
                        value,
                    },
                ],
                Some(o),
            ) if (*node, *channel) == o && (*value != 0) != turn_on => true,
            _ => return None,
        };
        let scale = find(chans, node, channel).map_or(1.0, |c| c.scale);
        Some(SimpleRule {
            sensor: (node, channel),
            above,
            value: value as f64 / scale,
            output,
            turn_on,
            undo_otherwise,
            alert,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chans() -> Vec<EditorChannel> {
        let ch = |node, channel, scale, output| EditorChannel {
            node,
            channel,
            label: format!("n{node}c{channel}"),
            scale,
            unit: String::new(),
            output,
        };
        vec![
            ch(1, 0, 1000.0, false),
            ch(1, 1, 1.0, false),
            ch(2, 3, 1.0, true),
        ]
    }

    fn sample() -> RecipeBody {
        RecipeBody {
            when: Cond::And {
                all: vec![
                    Cond::Compare {
                        node: 1,
                        channel: 0,
                        op: Op::Gt,
                        value: 30_500,
                    },
                    Cond::Compare {
                        node: 1,
                        channel: 1,
                        op: Op::Eq,
                        value: 1,
                    },
                    Cond::Not {
                        cond: Box::new(Cond::Compare {
                            node: 9,
                            channel: 9,
                            op: Op::Lt,
                            value: 4,
                        }),
                    },
                ],
            },
            then: vec![
                Action::Set {
                    node: 2,
                    channel: 3,
                    value: 1,
                },
                Action::Alert { text: "Hot".into() },
            ],
            otherwise: vec![Action::Set {
                node: 2,
                channel: 3,
                value: 0,
            }],
        }
    }

    #[test]
    fn blocks_round_trip() {
        let mut c = chans();
        let state = workspace_state(Some(&sample()), &mut c);
        // Shown in °C, stored in milli-°C.
        assert_eq!(
            state["blocks"]["blocks"][0]["inputs"]["WHEN"]["block"]["inputs"]["A"]["block"]["fields"]
                ["VALUE"],
            30.5
        );
        // The channel no node reports got a dropdown entry.
        assert!(c.iter().any(|x| (x.node, x.channel) == (9, 9)));
        assert_eq!(read_workspace(&state, &c).unwrap(), sample());
    }

    #[test]
    fn empty_and_unfinished_workspaces_explain_what_to_fix() {
        let mut c = chans();
        let state = workspace_state(None, &mut c);
        assert_eq!(
            read_workspace(&state, &c).unwrap_err(),
            "Fill every empty slot in the 'when' part."
        );
        let mut s = workspace_state(Some(&sample()), &mut c);
        s["blocks"]["blocks"][0]["inputs"]
            .as_object_mut()
            .unwrap()
            .remove("THEN");
        s["blocks"]["blocks"][0]["inputs"]
            .as_object_mut()
            .unwrap()
            .remove("ELSE");
        assert_eq!(
            read_workspace(&s, &c).unwrap_err(),
            "Add something to 'do' or 'otherwise'."
        );
        let mut s = workspace_state(Some(&sample()), &mut c);
        s["blocks"]["blocks"][0]["inputs"]["THEN"]["block"]["fields"]["OUTPUT"] = json!("none");
        assert_eq!(
            read_workspace(&s, &c).unwrap_err(),
            "Pick a sensor or output in every block."
        );
    }

    #[test]
    fn simple_rules_round_trip_and_reject_other_shapes() {
        let c = chans();
        let r = SimpleRule {
            sensor: (1, 0),
            above: true,
            value: 30.0,
            output: Some((2, 3)),
            turn_on: true,
            undo_otherwise: true,
            alert: String::new(),
        };
        let body = r.to_body(&c).unwrap();
        assert_eq!(
            body.when,
            Cond::Compare {
                node: 1,
                channel: 0,
                op: Op::Gt,
                value: 30_000
            }
        );
        assert_eq!(body.otherwise.len(), 1);
        assert_eq!(SimpleRule::from_body(&body, &c), Some(r));
        assert_eq!(SimpleRule::from_body(&sample(), &c), None);
        let nothing = SimpleRule {
            output: None,
            ..SimpleRule::from_body(&body, &c).unwrap()
        };
        assert!(nothing.to_body(&c).is_err());
    }
}
