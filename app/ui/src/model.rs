//! Data shapes shared by every screen. Desktop data comes from pc-daemon
//! (pc-daemon/src/api/mod.rs); phone data comes from the hub
//! (hub-node/src/hub/app.rs) and is converted into the same shapes in `api.rs`.
//!
//! Readings are integers in "milli" units (crypto-sharecrate/src/packet.rs):
//! milli-degrees C, millivolts, 0/1.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ChannelKind {
    DigitalIn,
    DigitalOut,
    AnalogIn,
    Temperature,
    #[default]
    #[serde(other)]
    Unknown,
}

impl ChannelKind {
    pub fn is_output(self) -> bool {
        self == Self::DigitalOut
    }
    pub fn is_digital(self) -> bool {
        matches!(self, Self::DigitalIn | Self::DigitalOut)
    }
    /// Stored value per display unit (°C and V are shown, milli units are stored).
    pub fn scale(self) -> f64 {
        match self {
            Self::Temperature | Self::AnalogIn => 1000.0,
            _ => 1.0,
        }
    }
    pub fn unit(self) -> &'static str {
        match self {
            Self::Temperature => "°C",
            Self::AnalogIn => "V",
            _ => "",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Temperature => "Temperature",
            Self::AnalogIn => "Analog in",
            Self::DigitalIn => "Digital in",
            Self::DigitalOut => "Output",
            Self::Unknown => "Unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Channel {
    pub channel: u8,
    pub kind: ChannelKind,
    pub name: String,
    pub last_value: Option<i64>,
    pub last_ts: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeInfo {
    pub id: u16,
    pub name: String,
    #[serde(default)]
    pub fingerprint: String,
    #[serde(default)]
    pub paired_at: i64,
    pub last_seen: Option<i64>,
    #[serde(default)]
    pub transport: Option<String>,
    #[serde(default)]
    pub sessions: i64,
    #[serde(default)]
    pub channels: Vec<Channel>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct HubLive {
    pub paired: bool,
    pub fingerprint: Option<String>,
    pub connected: bool,
    pub last_seen: Option<i64>,
    #[serde(default)]
    pub backlog: i64,
    #[serde(default)]
    pub sessions: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Status {
    /// This device's fingerprint (the PC, or the phone on the phone app).
    #[serde(default)]
    pub pc_fingerprint: String,
    #[serde(default)]
    pub pc_key_id: String,
    pub time: i64,
    pub hub: HubLive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PairKind {
    Node,
    Phone,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingPair {
    pub kind: PairKind,
    pub public_key: String,
    pub fingerprint: String,
    pub transport: Option<String>,
    pub seen_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Phone {
    pub id: u16,
    pub fingerprint: String,
    pub name: String,
    pub paired_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PhoneTicket {
    pub phone_id: u16,
    pub uri: String,
    pub svg: String,
    pub expires: i64,
    pub hub_fingerprint: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Reading {
    pub ts: i64,
    pub value: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Alert {
    pub id: i64,
    pub node_id: Option<i64>,
    pub node_name: Option<String>,
    pub channel: Option<i64>,
    #[serde(default)]
    pub code: i64,
    pub value: Option<i64>,
    pub message: String,
    pub ts: i64,
    pub acked: bool,
}

// ---- recipes (hub-node/src/recipes.rs) ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Op {
    Gt,
    Lt,
    Ge,
    Le,
    Eq,
    Ne,
}

impl Op {
    pub const ALL: [Op; 6] = [Op::Gt, Op::Lt, Op::Ge, Op::Le, Op::Eq, Op::Ne];
    pub fn code(self) -> &'static str {
        match self {
            Op::Gt => "gt",
            Op::Lt => "lt",
            Op::Ge => "ge",
            Op::Le => "le",
            Op::Eq => "eq",
            Op::Ne => "ne",
        }
    }
    pub fn from_code(s: &str) -> Option<Op> {
        Op::ALL.into_iter().find(|o| o.code() == s)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Cond {
    Compare {
        node: u16,
        channel: u8,
        op: Op,
        value: i64,
    },
    And {
        all: Vec<Cond>,
    },
    Or {
        any: Vec<Cond>,
    },
    Not {
        cond: Box<Cond>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Action {
    Set { node: u16, channel: u8, value: i64 },
    Alert { text: String },
}

/// What a recipe does, without its id, name and on/off state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecipeBody {
    pub when: Cond,
    pub then: Vec<Action>,
    pub otherwise: Vec<Action>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recipe {
    #[serde(default)]
    pub id: u32,
    pub name: String,
    pub enabled: bool,
    #[serde(flatten)]
    pub body: RecipeBody,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecipeSummary {
    pub id: u32,
    pub name: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct RecipeList {
    pub recipes: Vec<RecipeSummary>,
    pub max_recipes: usize,
}

/// A sensor or output as the recipe editors offer it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditorChannel {
    pub node: u16,
    pub channel: u8,
    pub label: String,
    /// Stored value per number shown (°C vs milli-°C).
    pub scale: f64,
    pub unit: String,
    pub output: bool,
}

impl EditorChannel {
    pub fn key(&self) -> String {
        format!("{}:{}", self.node, self.channel)
    }
}

/// Every channel of every node, for the recipe editors.
pub fn editor_channels(nodes: &[NodeInfo]) -> Vec<EditorChannel> {
    nodes
        .iter()
        .flat_map(|n| {
            n.channels.iter().map(move |c| EditorChannel {
                node: n.id,
                channel: c.channel,
                label: format!("{} / {}", n.name, c.name),
                scale: c.kind.scale(),
                unit: c.kind.unit().to_string(),
                output: c.kind.is_output(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn recipe_json_matches_the_hub() {
        let v = json!({
            "id": 3, "name": "Cool", "enabled": true,
            "when": {"type": "and", "all": [
                {"type": "compare", "node": 1, "channel": 0, "op": "gt", "value": 30000},
                {"type": "not", "cond": {"type": "compare", "node": 1, "channel": 1, "op": "eq", "value": 1}}
            ]},
            "then": [{"type": "set", "node": 2, "channel": 3, "value": 1}],
            "otherwise": [{"type": "alert", "text": "ok"}]
        });
        let r: Recipe = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(serde_json::to_value(&r).unwrap(), v);
    }

    #[test]
    fn unknown_channel_kinds_still_load() {
        let c: Channel = serde_json::from_value(
            json!({"channel": 1, "kind": "pressure", "name": "x", "last_value": null, "last_ts": null}),
        )
        .unwrap();
        assert_eq!(c.kind, ChannelKind::Unknown);
    }
}
