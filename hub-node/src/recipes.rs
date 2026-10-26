//! Recipes: "if this, then that" logic built from Blockly blocks in the phone
//! app and run by the hub itself, so they keep working with the PC switched off.
//!
//! A recipe is data, not code: a small tree of conditions plus lists of
//! actions. The hub checks every recipe before saving it (size, depth, number
//! of actions, targets) and only ever interprets it, so a phone can't make the
//! hub do anything it wasn't built to do.
//!
//! Recipes fire on the edge, like the PC's rules: `then` runs once when the
//! condition becomes true, and `otherwise` runs once when it becomes false again.
//! A condition on a sensor the hub hasn't heard from yet counts as false.
//!
//! ```json
//! { "name": "Too hot", "enabled": true,
//!   "when": { "type": "and", "all": [
//!       { "type": "compare", "node": 1, "channel": 0, "op": "gt", "value": 30000 },
//!       { "type": "not", "cond": { "type": "compare", "node": 1, "channel": 3, "op": "eq", "value": 1 } } ] },
//!   "then":      [ { "type": "set", "node": 2, "channel": 1, "value": 1 },
//!                  { "type": "alert", "text": "Greenhouse over 30 C" } ],
//!   "otherwise": [ { "type": "set", "node": 2, "channel": 1, "value": 0 } ] }
//! ```

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

pub const MAX_RECIPES: usize = 32;
pub const MAX_NAME: usize = 32;
pub const MAX_TEXT: usize = 64;
/// Per list (`then` and `otherwise` each).
pub const MAX_ACTIONS: usize = 8;
/// Condition blocks in one recipe, counting and/or/not.
pub const MAX_CONDITIONS: usize = 16;
pub const MAX_DEPTH: usize = 4;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Gt,
    Lt,
    Ge,
    Le,
    Eq,
    Ne,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Cond {
    Compare {
        node: u16,
        channel: u8,
        op: Op,
        value: i32,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    /// Set an output channel (0 = off, 1 = on).
    Set { node: u16, channel: u8, value: i32 },
    /// Add an alert (shown in both apps).
    Alert { text: String },
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    /// Given by the hub. 0 in a save request means "new recipe".
    #[serde(default)]
    pub id: u32,
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    pub when: Cond,
    #[serde(default)]
    pub then: Vec<Action>,
    #[serde(default)]
    pub otherwise: Vec<Action>,
}

impl Cond {
    /// Number of condition blocks, or an error if it nests too deep.
    fn count(&self, depth: usize) -> Result<usize, String> {
        if depth > MAX_DEPTH {
            return Err(format!("conditions nest more than {MAX_DEPTH} deep"));
        }
        let kids = match self {
            Cond::Compare { .. } => return Ok(1),
            Cond::And { all: v } | Cond::Or { any: v } => {
                if v.is_empty() {
                    return Err("an and/or block needs something inside it".into());
                }
                v.iter()
                    .map(|c| c.count(depth + 1))
                    .sum::<Result<usize, _>>()?
            }
            Cond::Not { cond } => cond.count(depth + 1)?,
        };
        Ok(kids + 1)
    }

    pub fn eval(&self, value: &impl Fn(u16, u8) -> Option<i32>) -> bool {
        match self {
            Cond::Compare {
                node,
                channel,
                op,
                value: want,
            } => value(*node, *channel).is_some_and(|v| match op {
                Op::Gt => v > *want,
                Op::Lt => v < *want,
                Op::Ge => v >= *want,
                Op::Le => v <= *want,
                Op::Eq => v == *want,
                Op::Ne => v != *want,
            }),
            Cond::And { all } => all.iter().all(|c| c.eval(value)),
            Cond::Or { any } => any.iter().any(|c| c.eval(value)),
            Cond::Not { cond } => !cond.eval(value),
        }
    }

    pub fn mentions(&self, node_id: u16, ch: u8) -> bool {
        match self {
            Cond::Compare { node, channel, .. } => *node == node_id && *channel == ch,
            Cond::And { all: v } | Cond::Or { any: v } => v.iter().any(|c| c.mentions(node_id, ch)),
            Cond::Not { cond } => cond.mentions(node_id, ch),
        }
    }

    pub fn sensors(&self, out: &mut Vec<(u16, u8)>) {
        match self {
            Cond::Compare { node, channel, .. } => out.push((*node, *channel)),
            Cond::And { all: v } | Cond::Or { any: v } => v.iter().for_each(|c| c.sensors(out)),
            Cond::Not { cond } => cond.sensors(out),
        }
    }
}

impl Recipe {
    /// Shape checks only. The hub separately checks that the nodes and
    /// output channels exist (it knows which ones do).
    pub fn validate(&self) -> Result<(), String> {
        let name = self.name.trim();
        if name.is_empty() || name.chars().count() > MAX_NAME {
            return Err(format!("name must be 1-{MAX_NAME} characters"));
        }
        if self.when.count(1)? > MAX_CONDITIONS {
            return Err(format!("at most {MAX_CONDITIONS} condition blocks"));
        }
        if self.then.is_empty() && self.otherwise.is_empty() {
            return Err("the recipe doesn't do anything".into());
        }
        for list in [&self.then, &self.otherwise] {
            if list.len() > MAX_ACTIONS {
                return Err(format!("at most {MAX_ACTIONS} actions per list"));
            }
            for a in list.iter() {
                if let Action::Alert { text } = a
                    && (text.trim().is_empty() || text.chars().count() > MAX_TEXT)
                {
                    return Err(format!("alert text must be 1-{MAX_TEXT} characters"));
                }
            }
        }
        Ok(())
    }

    pub fn outputs(&self) -> impl Iterator<Item = (u16, u8, i32)> + '_ {
        self.then
            .iter()
            .chain(&self.otherwise)
            .filter_map(|a| match a {
                Action::Set {
                    node,
                    channel,
                    value,
                } => Some((*node, *channel, *value)),
                Action::Alert { .. } => None,
            })
    }
}

#[derive(Serialize, Deserialize, Default)]
struct RecipesFile {
    next_id: u32,
    recipes: Vec<Recipe>,
}

/// A recipe that changed state: run these actions.
#[derive(Debug, PartialEq)]
pub struct Fired {
    pub id: u32,
    pub name: String,
    pub actions: Vec<Action>,
}

/// All recipes, saved to `recipes.json` on every change (like pairings, they
/// are too important to wait for the daily flush).
pub struct Recipes {
    list: Vec<Recipe>,
    /// Recipes whose condition is currently true. Not saved: after a reboot
    /// every recipe starts as "false" and fires again if it's still true.
    active: HashMap<u32, bool>,
    /// Ids are never reused, so an old "recipe:<id>" in the PC's command log
    /// always means the same recipe.
    next_id: u32,
    path: Option<PathBuf>,
}

impl Recipes {
    pub fn load(path: Option<PathBuf>) -> Result<Self> {
        let f: RecipesFile = match &path {
            Some(p) if p.exists() => serde_json::from_str(&std::fs::read_to_string(p)?)?,
            _ => RecipesFile::default(),
        };
        Ok(Self {
            next_id: f.next_id.max(1),
            list: f.recipes,
            active: HashMap::new(),
            path,
        })
    }

    fn save(&self) -> Result<()> {
        let Some(p) = &self.path else { return Ok(()) };
        let f = RecipesFile {
            next_id: self.next_id,
            recipes: self.list.clone(),
        };
        let tmp = p.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&f)?)?;
        std::fs::rename(tmp, p)?;
        Ok(())
    }

    pub fn list(&self) -> &[Recipe] {
        &self.list
    }

    pub fn get(&self, id: u32) -> Option<&Recipe> {
        self.list.iter().find(|r| r.id == id)
    }

    /// The `otherwise` actions of a recipe that's currently true, so taking
    /// it away (edit, disable, delete) also turns off what it turned on.
    fn wind_down(&mut self, id: u32) -> Option<Fired> {
        if self.active.remove(&id) != Some(true) {
            return None;
        }
        let r = self.get(id)?;
        Some(Fired {
            id,
            name: r.name.clone(),
            actions: r.otherwise.clone(),
        })
    }

    /// Adds (id 0) or replaces a recipe. Call [`Recipe::validate`] first.
    pub fn upsert(&mut self, mut r: Recipe) -> Result<(u32, Option<Fired>), String> {
        r.name = r.name.trim().to_string();
        let off = if r.id == 0 {
            if self.list.len() >= MAX_RECIPES {
                return Err(format!("the hub holds at most {MAX_RECIPES} recipes"));
            }
            r.id = self.next_id;
            self.next_id += 1;
            self.list.push(r.clone());
            None
        } else {
            let off = self.wind_down(r.id);
            let slot = self
                .list
                .iter_mut()
                .find(|x| x.id == r.id)
                .ok_or("no such recipe")?;
            *slot = r.clone();
            off
        };
        self.save().map_err(|e| format!("saving recipes: {e:#}"))?;
        Ok((r.id, off))
    }

    pub fn remove(&mut self, id: u32) -> Result<Option<Fired>, String> {
        if self.get(id).is_none() {
            return Err("no such recipe".into());
        }
        let off = self.wind_down(id);
        self.list.retain(|r| r.id != id);
        self.save().map_err(|e| format!("saving recipes: {e:#}"))?;
        Ok(off)
    }

    pub fn set_enabled(&mut self, id: u32, enabled: bool) -> Result<Option<Fired>, String> {
        let off = if enabled { None } else { self.wind_down(id) };
        let r = self
            .list
            .iter_mut()
            .find(|r| r.id == id)
            .ok_or("no such recipe")?;
        r.enabled = enabled;
        self.save().map_err(|e| format!("saving recipes: {e:#}"))?;
        Ok(off)
    }

    /// Recipes that point at a node. Used when a node is unpaired.
    pub fn uses_node(&self, node_id: u16) -> Vec<String> {
        self.list
            .iter()
            .filter(|r| {
                let mut s = Vec::new();
                r.when.sensors(&mut s);
                s.iter().any(|(n, _)| *n == node_id) || r.outputs().any(|(n, _, _)| n == node_id)
            })
            .map(|r| r.name.clone())
            .collect()
    }

    /// Re-checks every enabled recipe that reads `node_id`/`channel`. Returns
    /// the ones whose condition flipped, with the actions to run.
    pub fn on_reading(
        &mut self,
        node_id: u16,
        channel: u8,
        value: impl Fn(u16, u8) -> Option<i32>,
    ) -> Vec<Fired> {
        let mut fired = Vec::new();
        for r in self
            .list
            .iter()
            .filter(|r| r.enabled && r.when.mentions(node_id, channel))
        {
            let now = r.when.eval(&value);
            let was = self.active.get(&r.id).copied().unwrap_or(false);
            if now == was {
                continue;
            }
            self.active.insert(r.id, now);
            let actions = if now { &r.then } else { &r.otherwise };
            if !actions.is_empty() {
                fired.push(Fired {
                    id: r.id,
                    name: r.name.clone(),
                    actions: actions.clone(),
                });
            }
        }
        fired
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hot() -> Recipe {
        serde_json::from_str(
            r#"{ "name": "Too hot",
                 "when": { "type": "compare", "node": 1, "channel": 0, "op": "gt", "value": 30000 },
                 "then": [ { "type": "set", "node": 2, "channel": 1, "value": 1 },
                           { "type": "alert", "text": "hot" } ],
                 "otherwise": [ { "type": "set", "node": 2, "channel": 1, "value": 0 } ] }"#,
        )
        .unwrap()
    }

    #[test]
    fn fires_on_edges_only() {
        let mut rs = Recipes::load(None).unwrap();
        let (id, _) = rs.upsert(hot()).unwrap();
        let mut temp = 25_000;
        let f = |t: i32| move |n: u16, c: u8| (n == 1 && c == 0).then_some(t);
        assert!(rs.on_reading(1, 0, f(temp)).is_empty());
        temp = 31_000;
        let fired = rs.on_reading(1, 0, f(temp));
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].id, id);
        assert_eq!(fired[0].actions.len(), 2);
        assert!(
            rs.on_reading(1, 0, f(32_000)).is_empty(),
            "no repeat while still hot"
        );
        let cleared = rs.on_reading(1, 0, f(29_000));
        assert_eq!(
            cleared[0].actions,
            vec![Action::Set {
                node: 2,
                channel: 1,
                value: 0
            }]
        );
        assert!(
            rs.on_reading(9, 9, f(0)).is_empty(),
            "other sensors don't matter"
        );
    }

    #[test]
    fn unknown_sensor_is_false_and_logic_works() {
        let c: Cond = serde_json::from_str(
            r#"{ "type": "or", "any": [
                 { "type": "compare", "node": 5, "channel": 0, "op": "eq", "value": 1 },
                 { "type": "not", "cond": { "type": "compare", "node": 1, "channel": 0, "op": "lt", "value": 10 } } ] }"#,
        )
        .unwrap();
        // nothing known: the compares are false, so not(false) makes it true
        assert!(c.eval(&|_, _| None));
        // node 5 unknown -> false; node 1 = 20 -> not(20 < 10) = true
        assert!(c.eval(&|n, _| (n == 1).then_some(20)));
        assert!(!c.eval(&|n, _| (n == 1).then_some(5)));
    }

    #[test]
    fn rejects_bad_shapes() {
        let mut r = hot();
        r.name = " ".into();
        assert!(r.validate().is_err());

        let mut r = hot();
        r.then = vec![Action::Alert { text: "x".into() }; MAX_ACTIONS + 1];
        assert!(r.validate().is_err());

        let mut r = hot();
        let mut c = r.when.clone();
        for _ in 0..MAX_DEPTH {
            c = Cond::Not { cond: Box::new(c) };
        }
        r.when = c;
        assert!(r.validate().is_err(), "too deep");

        let mut r = hot();
        r.when = Cond::And { all: vec![] };
        assert!(r.validate().is_err(), "empty and");

        let mut r = hot();
        r.then.clear();
        r.otherwise.clear();
        assert!(r.validate().is_err(), "does nothing");

        assert!(hot().validate().is_ok());
        assert!(
            serde_json::from_str::<Recipe>(r#"{"name":"x","when":{"type":"compare","node":1,"channel":0,"op":"gt","value":1},"then":[],"script":"rm -rf /"}"#).is_err(),
            "unknown fields are refused"
        );
    }

    #[test]
    fn editing_an_active_recipe_turns_it_off_first() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recipes.json");
        let mut rs = Recipes::load(Some(path.clone())).unwrap();
        let (id, _) = rs.upsert(hot()).unwrap();
        rs.on_reading(1, 0, |_, _| Some(40_000));
        let mut edited = hot();
        edited.id = id;
        edited.name = "Renamed".into();
        let (_, off) = rs.upsert(edited).unwrap();
        assert_eq!(off.unwrap().actions.len(), 1, "otherwise actions ran");
        assert!(rs.set_enabled(id, false).unwrap().is_none(), "already off");

        // saved, and ids are never reused
        rs.remove(id).unwrap();
        let mut rs = Recipes::load(Some(path)).unwrap();
        assert!(rs.list().is_empty());
        let (id2, _) = rs.upsert(hot()).unwrap();
        assert!(id2 > id);
    }
}
