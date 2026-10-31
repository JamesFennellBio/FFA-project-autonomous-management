//! Preferences picked in the first-run wizard and changed in Settings, and the
//! words each mode uses. Saved by the Tauri side (app config folder), so the
//! RAM / CPU limits can be applied before the window opens next time.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Lab,
    #[default]
    Farm,
    Home,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Lab, Mode::Farm, Mode::Home];
    pub fn code(self) -> &'static str {
        match self {
            Mode::Lab => "lab",
            Mode::Farm => "farm",
            Mode::Home => "home",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Mode::Lab => "Lab",
            Mode::Farm => "Farm",
            Mode::Home => "Home",
        }
    }
    pub fn copy(self) -> &'static Copy {
        match self {
            Mode::Lab => &LAB,
            Mode::Farm => &FARM,
            Mode::Home => &HOME,
        }
    }
}

/// Mode-specific words. Everything else in the UI is the same in every mode.
pub struct Copy {
    /// Nav item and page title for the node list.
    pub devices: &'static str,
    /// One node, lower case ("sensor", "instrument", "device").
    pub device: &'static str,
    pub tagline: &'static str,
    /// Example names in placeholders.
    pub example_node: &'static str,
    pub example_rule: &'static str,
}

const FARM: Copy = Copy {
    devices: "Sensors",
    device: "sensor",
    tagline: "Fields, barns and greenhouses at a glance.",
    example_node: "Greenhouse 1",
    example_rule: "When the greenhouse is above 30 °C, turn the fans on.",
};
const LAB: Copy = Copy {
    devices: "Instruments",
    device: "instrument",
    tagline: "Benches and instruments at a glance.",
    example_node: "Incubator A",
    example_rule: "When the incubator is above 37.5 °C, turn the heater off.",
};
const HOME: Copy = Copy {
    devices: "Devices",
    device: "device",
    tagline: "Every room at a glance.",
    example_node: "Living room",
    example_rule: "When the living room is below 18 °C, turn the heater on.",
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Light,
    Dark,
    #[default]
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Perf {
    /// Under 3 GB of RAM, fewer animations, smaller history.
    Lite,
    /// Under 6 GB of RAM.
    #[default]
    Normal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub mode: Mode,
    pub theme: Theme,
    /// Big text, big buttons, plain words, no motion, simple rule builder.
    pub boomer: bool,
    /// Maximum-contrast white theme for outdoors (phone). None = automatic:
    /// on for Farm mode on the phone.
    pub sunlight: Option<bool>,
    pub perf: Perf,
    /// CPU limit, 10–100 % of the whole machine.
    pub cpu_pct: u8,
    /// False until the first-run wizard is finished.
    pub setup_done: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            mode: Mode::Farm,
            theme: Theme::System,
            boomer: false,
            sunlight: None,
            perf: Perf::Normal,
            cpu_pct: 50,
            setup_done: false,
        }
    }
}

impl Prefs {
    pub fn sunlight_on(&self, phone: bool) -> bool {
        self.sunlight.unwrap_or(phone && self.mode == Mode::Farm)
    }

    /// Poll intervals are stretched when the CPU limit is low (1x at 50 %+,
    /// up to 4x at 10 %), and in Lite.
    pub fn poll_ms(&self, base: u32) -> u32 {
        let pct = u32::from(self.cpu_pct.clamp(10, 100));
        let mut ms = if pct >= 50 { base } else { base * 50 / pct };
        if self.perf == Perf::Lite {
            ms = ms * 3 / 2;
        }
        ms.min(base * 4)
    }

    /// Most history points to load for one chart.
    pub fn history_limit(&self) -> u32 {
        match self.perf {
            Perf::Lite => 2_000,
            Perf::Normal => 10_000,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_or_partial_prefs_still_load() {
        let p: Prefs = serde_json::from_str(r#"{"mode":"lab","boomer":true}"#).unwrap();
        assert_eq!(p.mode, Mode::Lab);
        assert!(p.boomer);
        assert_eq!(p.cpu_pct, 50);
        assert!(!p.setup_done);
    }

    #[test]
    fn sunlight_defaults_to_farm_on_the_phone() {
        let mut p = Prefs::default();
        assert!(p.sunlight_on(true));
        assert!(!p.sunlight_on(false));
        p.mode = Mode::Lab;
        assert!(!p.sunlight_on(true));
        p.sunlight = Some(true);
        assert!(p.sunlight_on(false));
    }

    #[test]
    fn low_cpu_limits_slow_polling() {
        let mut p = Prefs::default();
        assert_eq!(p.poll_ms(3000), 3000);
        p.cpu_pct = 25;
        assert_eq!(p.poll_ms(3000), 6000);
        p.cpu_pct = 10;
        p.perf = Perf::Lite;
        assert_eq!(p.poll_ms(3000), 12000);
    }
}
