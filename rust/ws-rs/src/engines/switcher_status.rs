//! Port of `SwitcherStatus.swift` — the Hyper+S status row's data.
//!
//! CPU / RAM / battery / unread chips are gathered by `pylib/status.py`
//! through the `status.gather` helper method (`gather()` blocks for the CPU
//! sampling delay, ~0.3 s).

use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreadChip {
    pub app: String,
    pub count: String,
    pub mentions: i64,
    pub warn: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SwitcherStatus {
    pub cpu: Option<i64>,
    pub ram: Option<i64>,
    pub battery: Option<(i64, bool)>,
    pub chips: Vec<UnreadChip>,
}

impl SwitcherStatus {
    pub fn unread_by_app(&self) -> HashMap<String, String> {
        let mut out = HashMap::new();
        for chip in &self.chips {
            if !chip.count.is_empty() {
                out.insert(chip.app.clone(), chip.count.clone());
            }
        }
        out
    }

    pub fn from_json(value: &Value) -> Self {
        let cpu = value.get("cpu").and_then(Value::as_i64);
        let ram = value.get("ram").and_then(Value::as_i64);
        let battery = value.get("battery").and_then(|b| {
            let pct = b.get("pct").and_then(Value::as_i64)?;
            let charging = b.get("charging").and_then(Value::as_bool).unwrap_or(false);
            Some((pct, charging))
        });
        let chips = value
            .get("chips")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .map(|c| UnreadChip {
                        app: c.get("app").and_then(Value::as_str).unwrap_or("").to_string(),
                        count: c
                            .get("count")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        mentions: c.get("mentions").and_then(Value::as_i64).unwrap_or(0),
                        warn: c.get("warn").and_then(Value::as_bool).unwrap_or(false),
                    })
                    .collect()
            })
            .unwrap_or_default();
        SwitcherStatus {
            cpu,
            ram,
            battery,
            chips,
        }
    }

    pub fn gather(notify_script: &str) -> SwitcherStatus {
        let params = json!({"notify_script": notify_script});
        match crate::app::python_helper::PythonHelper::shared().call(
            "status.gather",
            params,
            Duration::from_secs(60),
            Duration::from_secs(5),
        ) {
            Ok(value) => SwitcherStatus::from_json(&value),
            Err(_) => SwitcherStatus::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn helper_ready() -> bool {
        let lib = format!("{}/../../pylib", env!("CARGO_MANIFEST_DIR"));
        if !crate::app::python_helper::lib_has_helper(&lib) {
            return false;
        }
        let h = crate::app::python_helper::PythonHelper::shared();
        h.configure(&lib);
        h.call_default("ping", json!({})).is_ok()
    }

    #[test]
    fn parses_status_payload() {
        let raw = json!({
            "cpu": 12,
            "ram": 55,
            "battery": {"pct": 88, "charging": true},
            "chips": [
                {"app": "Webex", "count": "3", "mentions": 1, "warn": true},
                {"app": "Outlook", "count": "", "mentions": 0, "warn": false}
            ]
        });
        let s = SwitcherStatus::from_json(&raw);
        assert_eq!(s.cpu, Some(12));
        assert_eq!(s.ram, Some(55));
        assert_eq!(s.battery, Some((88, true)));
        assert_eq!(s.chips.len(), 2);
        let unread = s.unread_by_app();
        assert_eq!(unread.get("Webex"), Some(&"3".to_string()));
        assert!(!unread.contains_key("Outlook"));
    }

    #[test]
    fn handles_missing_fields() {
        let s = SwitcherStatus::from_json(&json!({"cpu": null, "battery": null}));
        assert_eq!(s.cpu, None);
        assert!(s.battery.is_none());
        assert!(s.chips.is_empty());
    }

    #[test]
    fn gather_when_helper_present() {
        if !helper_ready() {
            eprintln!("skipping: no python 3.11+ or pylib/helper not found");
            return;
        }
        let s = SwitcherStatus::gather("/bin/echo");
        // The helper always answers an object; RAM is read on this machine.
        assert!(s.ram.is_some());
    }
}
