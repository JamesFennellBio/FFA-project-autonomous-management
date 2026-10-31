//! Turning stored values and times into text.

use crate::model::{Alert, ChannelKind};

/// A reading in display units, e.g. "21.4 °C".
pub fn value(kind: ChannelKind, v: Option<i64>) -> String {
    let Some(v) = v else {
        return "—".into();
    };
    match kind {
        ChannelKind::Temperature => format!("{:.1} °C", v as f64 / 1000.0),
        ChannelKind::AnalogIn => format!("{:.2} V", v as f64 / 1000.0),
        ChannelKind::DigitalIn | ChannelKind::DigitalOut => {
            if v != 0 { "On" } else { "Off" }.into()
        }
        ChannelKind::Unknown => v.to_string(),
    }
}

/// The number part only, for big readouts ("21.4"), with the unit separate.
pub fn value_parts(kind: ChannelKind, v: Option<i64>) -> (String, &'static str) {
    match (kind, v) {
        (_, None) => ("—".into(), ""),
        (ChannelKind::Temperature, Some(v)) => (format!("{:.1}", v as f64 / 1000.0), "°C"),
        (ChannelKind::AnalogIn, Some(v)) => (format!("{:.2}", v as f64 / 1000.0), "V"),
        (k, Some(v)) if k.is_digital() => ((if v != 0 { "On" } else { "Off" }).into(), ""),
        (_, Some(v)) => (v.to_string(), ""),
    }
}

/// Stored value -> chart number in display units.
pub fn chart_value(kind: ChannelKind, v: i64) -> f64 {
    v as f64 / kind.scale()
}

/// "12s ago", "4m ago", "never".
pub fn ago(ts: Option<i64>, now: i64) -> String {
    let Some(ts) = ts.filter(|t| *t > 0) else {
        return "never".into();
    };
    let s = (now - ts).max(0);
    match s {
        0..60 => format!("{s}s ago"),
        60..3600 => format!("{}m ago", s / 60),
        3600..86400 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86400),
    }
}

/// Online = heard from in the last 2 minutes (nodes report every 10 s).
pub fn is_online(last_seen: Option<i64>, now: i64) -> bool {
    last_seen.is_some_and(|t| now - t < 120)
}

/// "Barn / Temperature" or "System" for an alert's source.
pub fn alert_source(a: &Alert) -> String {
    match (&a.node_name, a.node_id) {
        (Some(n), _) => n.clone(),
        (None, Some(id)) => format!("Node {id}"),
        _ => "System".into(),
    }
}

/// Hub alert codes (crypto-sharecrate/src/packet.rs) in plain words.
pub fn alert_code(code: i64) -> &'static str {
    match code {
        1 => "Sensor fault",
        2 => "Threshold crossed",
        3 => "Node restarted",
        _ => "Alert",
    }
}

/// Local time as HH:MM, or "Mon 3" for older times (charts, tooltips).
pub fn clock(ts: i64, long_range: bool) -> String {
    let d = js_date(ts);
    if long_range {
        const MONTHS: [&str; 12] = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        format!("{} {}", MONTHS[d.month as usize % 12], d.day)
    } else {
        format!("{:02}:{:02}", d.hour, d.minute)
    }
}

struct Parts {
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
}

#[cfg(target_arch = "wasm32")]
fn js_date(ts: i64) -> Parts {
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ts as f64 * 1000.0));
    Parts {
        month: d.get_month(),
        day: d.get_date(),
        hour: d.get_hours(),
        minute: d.get_minutes(),
    }
}

/// UTC on native builds (only tests run there).
#[cfg(not(target_arch = "wasm32"))]
fn js_date(ts: i64) -> Parts {
    let secs = ts.rem_euclid(86_400);
    let days = ts.div_euclid(86_400);
    // Civil-from-days (Howard Hinnant), enough for test output.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32 - 1;
    Parts {
        month,
        day,
        hour: (secs / 3600) as u32,
        minute: (secs % 3600 / 60) as u32,
    }
}

/// Seconds since 1970, from the browser clock.
pub fn now() -> i64 {
    #[cfg(target_arch = "wasm32")]
    {
        (js_sys::Date::now() / 1000.0) as i64
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_in_display_units() {
        assert_eq!(value(ChannelKind::Temperature, Some(21_437)), "21.4 °C");
        assert_eq!(value(ChannelKind::AnalogIn, Some(3_300)), "3.30 V");
        assert_eq!(value(ChannelKind::DigitalOut, Some(1)), "On");
        assert_eq!(value(ChannelKind::Temperature, None), "—");
        assert_eq!(
            value_parts(ChannelKind::Temperature, Some(-500)),
            ("-0.5".into(), "°C")
        );
    }

    #[test]
    fn relative_times() {
        assert_eq!(ago(Some(100), 112), "12s ago");
        assert_eq!(ago(Some(100), 100 + 3 * 3600), "3h ago");
        assert_eq!(ago(None, 5), "never");
        assert!(is_online(Some(100), 200));
        assert!(!is_online(Some(100), 400));
    }

    #[test]
    fn clock_text() {
        // 2026-10-31 14:05 UTC
        assert_eq!(clock(1_793_455_500, false), "14:05");
        assert_eq!(clock(1_793_455_500, true), "Oct 31");
    }
}
