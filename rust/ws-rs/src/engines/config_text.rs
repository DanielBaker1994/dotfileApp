//! Port of `ConfigText.swift` — commands.toml transport.
//!
//! The codec lives in `pylib/config_text.py`; this is a thin forward to the
//! `config.*` helper methods. `configDecodedLines` / `configSectionEntries`
//! cache the decoded JSON under `~/.cache/kitchen-sink/config-<fnv>.json`
//! (FNV-1a 64 over the inputs), exactly like the Swift facade.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConfigDecodedLine {
    pub index: i64,
    pub trimmed: String,
    pub header: Option<String>,
    pub key: Option<String>,
    pub value: Option<String>,
}

fn helper_call(method: &str, params: Value, timeout_secs: u64) -> Option<Value> {
    crate::app::python_helper::PythonHelper::shared()
        .call(
            method,
            params,
            Duration::from_secs(timeout_secs),
            Duration::from_secs(5),
        )
        .ok()
}

fn cache_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".cache/kitchen-sink")
}

fn cache_key(parts: &[&str]) -> String {
    let mut hash: u64 = 14695981039346656037;
    for byte in parts.join("\u{1}").bytes() {
        hash = (hash ^ byte as u64).wrapping_mul(1099511628211);
    }
    format!("{hash:016x}")
}

fn cache_read(key: &str) -> Option<Value> {
    let data = std::fs::read(cache_dir().join(format!("config-{key}.json"))).ok()?;
    serde_json::from_slice(&data).ok()
}

fn cache_write(key: &str, value: &Value) {
    let dir = cache_dir();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(data) = serde_json::to_vec(value) {
        let _ = std::fs::write(dir.join(format!("config-{key}.json")), data);
    }
}

fn parse_decoded_lines(raw: &Value) -> Vec<ConfigDecodedLine> {
    raw.as_array()
        .map(|arr| {
            arr.iter()
                .map(|o| ConfigDecodedLine {
                    index: o.get("index").and_then(Value::as_i64).unwrap_or(0),
                    trimmed: o
                        .get("trimmed")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    header: o.get("header").and_then(Value::as_str).map(str::to_string),
                    key: o.get("key").and_then(Value::as_str).map(str::to_string),
                    value: o.get("value").and_then(Value::as_str).map(str::to_string),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_entries(raw: &Value) -> Vec<(i64, String, String)> {
    raw.as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|o| {
                    let index = o.get("index").and_then(Value::as_i64)?;
                    let key = o.get("key").and_then(Value::as_str)?.to_string();
                    let value = o.get("value").and_then(Value::as_str)?.to_string();
                    Some((index, key, value))
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn config_decoded_lines(text: &str) -> Vec<ConfigDecodedLine> {
    let key = cache_key(&["decode", text]);
    let raw = if let Some(cached) = cache_read(&key) {
        cached
    } else {
        let Some(boxed) = helper_call("config.decode", json!({"text": text}), 10) else {
            return Vec::new();
        };
        let Some(lines) = boxed.get("lines").cloned() else {
            return Vec::new();
        };
        cache_write(&key, &lines);
        lines
    };
    parse_decoded_lines(&raw)
}

pub fn config_section_entries(lines: &[String], section: &str) -> Vec<(i64, String, String)> {
    let text = lines.join("\n");
    let key = cache_key(&["section", section, &text]);
    let raw = if let Some(cached) = cache_read(&key) {
        cached
    } else {
        let params = json!({"text": text, "section": section});
        let Some(boxed) = helper_call("config.section_entries", params, 10) else {
            return Vec::new();
        };
        let Some(entries) = boxed.get("entries").cloned() else {
            return Vec::new();
        };
        cache_write(&key, &entries);
        entries
    };
    parse_entries(&raw)
}

pub fn config_line(key: &str, value: &str) -> Option<String> {
    helper_call("config.line", json!({"key": key, "value": value}), 10)?
        .get("line")
        .and_then(Value::as_str)
        .map(str::to_string)
}

pub fn config_setting_text(
    text: &str,
    section: &str,
    kv: &[(String, Option<String>)],
) -> Option<String> {
    let pairs: Vec<Value> = kv
        .iter()
        .map(|(k, v)| match v {
            Some(s) => json!([k, s]),
            None => json!([k, null]),
        })
        .collect();
    let params = json!({"text": text, "section": section, "kv": pairs});
    helper_call("config.setting", params, 10)?
        .get("text")
        .and_then(Value::as_str)
        .map(str::to_string)
}

pub fn config_lines(text: &str) -> Vec<String> {
    text.split('\n').map(str::to_string).collect()
}

static TRI_MEMO: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();

// The grammar lives in pylib/config_text.py; memoised because config loaders
// call this per key. A failed call is NEVER memoised (the helper may still be
// starting, so the next call retries).
pub fn tri(s: Option<&str>) -> Option<bool> {
    let key = s.unwrap_or("").to_lowercase();
    let memo = TRI_MEMO.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(hit) = memo.lock().unwrap().get(&key) {
        return Some(*hit);
    }
    let value = helper_call("config.tri", json!({"text": key}), 30)?
        .get("value")
        .and_then(Value::as_bool);
    if let Some(v) = value {
        memo.lock().unwrap().insert(key, v);
    }
    value
}

pub fn resolve_binary(name: &str) -> Option<String> {
    helper_call("config.resolve_binary", json!({"name": name}), 30)?
        .get("path")
        .and_then(Value::as_str)
        .map(str::to_string)
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
    fn config_lines_keeps_trailing_empty() {
        assert_eq!(config_lines("a\nb\n"), vec!["a", "b", ""]);
        assert_eq!(config_lines(""), vec![""]);
    }

    #[test]
    fn cache_key_is_stable_and_hex() {
        let a = cache_key(&["decode", "hello"]);
        assert_eq!(a, cache_key(&["decode", "hello"]));
        assert_ne!(a, cache_key(&["decode", "hellp"]));
        assert_eq!(a.len(), 16);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn parses_decoded_lines() {
        let raw = json!([
            {"index": 0, "trimmed": "[ai]", "header": "ai"},
            {"index": 1, "trimmed": "enabled = true", "key": "enabled", "value": "true"},
            {"index": 2, "trimmed": ""}
        ]);
        let lines = parse_decoded_lines(&raw);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].header.as_deref(), Some("ai"));
        assert_eq!(lines[1].key.as_deref(), Some("enabled"));
        assert_eq!(lines[1].value.as_deref(), Some("true"));
        assert!(lines[2].key.is_none());
    }

    #[test]
    fn parses_section_entries_requires_all_three() {
        let raw = json!([
            {"index": 3, "key": "a", "value": "1"},
            {"index": 4, "key": "b"}
        ]);
        assert_eq!(parse_entries(&raw), vec![(3, "a".to_string(), "1".to_string())]);
    }

    #[test]
    fn decode_round_trip_when_helper_present() {
        if !helper_ready() {
            eprintln!("skipping: no python 3.11+ or pylib/helper not found");
            return;
        }
        let lines = config_decoded_lines("[ai]\nenabled = true\n");
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].header.as_deref(), Some("ai"));
        assert_eq!(lines[1].key.as_deref(), Some("enabled"));

        let entries = config_section_entries(&config_lines("[ai]\nenabled = true"), "ai");
        assert_eq!(entries, vec![(1, "enabled".to_string(), "true".to_string())]);

        assert_eq!(config_line("x", "y"), Some("x = \"y\"".to_string()));
        assert_eq!(tri(Some("TRUE")), Some(true));
        assert_eq!(tri(Some("nope")), None);
        assert_eq!(resolve_binary("/bin/sh"), Some("/bin/sh".to_string()));
    }
}
