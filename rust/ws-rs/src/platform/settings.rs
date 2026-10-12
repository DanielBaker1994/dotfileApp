//! App preferences addressed by a plain key (was direct `NSUserDefaults`
//! calls at each call site).
//!
//! macOS keeps the existing `NSUserDefaults.standardUserDefaults()` storage
//! verbatim — same keys, same value encodings — so nothing is migrated and
//! data stored by earlier builds stays readable. `NSUserDefaults` is
//! thread-safe and has no main-thread requirement, matching the calls this
//! replaces.
//!
//! Non-macOS targets (a branch that does not compile on macOS) keep a flat
//! JSON object at `<config dir>/ks-prefs.json`, best effort: a missing or
//! malformed file reads as empty, writes rewrite the whole object.

/// The string stored under `key` (`stringForKey`), if any.
#[cfg(target_os = "macos")]
pub fn get_string(key: &str) -> Option<String> {
    use objc2_foundation::{NSString, NSUserDefaults};
    NSUserDefaults::standardUserDefaults()
        .stringForKey(&NSString::from_str(key))
        .map(|s| s.to_string())
}

/// Store `val` under `key` (`setObject:forKey:` with an `NSString`).
#[cfg(target_os = "macos")]
pub fn set_string(key: &str, val: &str) {
    use objc2_foundation::{NSString, NSUserDefaults};
    unsafe {
        NSUserDefaults::standardUserDefaults()
            .setObject_forKey(Some(&NSString::from_str(val)), &NSString::from_str(key));
    }
}

/// The bool stored under `key` (`boolForKey`; false when unset).
#[cfg(target_os = "macos")]
pub fn get_bool(key: &str) -> bool {
    use objc2_foundation::{NSString, NSUserDefaults};
    NSUserDefaults::standardUserDefaults().boolForKey(&NSString::from_str(key))
}

/// Store a bool under `key` (`setBool:forKey:`).
#[cfg(target_os = "macos")]
pub fn set_bool(key: &str, on: bool) {
    use objc2_foundation::{NSString, NSUserDefaults};
    NSUserDefaults::standardUserDefaults().setBool_forKey(on, &NSString::from_str(key));
}

/// The f64 stored under `key` (`doubleForKey`; 0.0 when unset).
#[cfg(target_os = "macos")]
pub fn get_f64(key: &str) -> f64 {
    use objc2_foundation::{NSString, NSUserDefaults};
    NSUserDefaults::standardUserDefaults().doubleForKey(&NSString::from_str(key))
}

/// Store an f64 under `key` (`setDouble:forKey:`).
#[cfg(target_os = "macos")]
pub fn set_f64(key: &str, val: f64) {
    use objc2_foundation::{NSString, NSUserDefaults};
    NSUserDefaults::standardUserDefaults().setDouble_forKey(val, &NSString::from_str(key));
}

/// Remove whatever is stored under `key` (`removeObjectForKey`).
#[cfg(target_os = "macos")]
pub fn remove_key(key: &str) {
    use objc2_foundation::{NSString, NSUserDefaults};
    NSUserDefaults::standardUserDefaults().removeObjectForKey(&NSString::from_str(key));
}

// -- Non-macOS: a flat JSON object at `<config dir>/ks-prefs.json` ------------

#[cfg(not(target_os = "macos"))]
fn prefs_path() -> Option<std::path::PathBuf> {
    let paths = crate::app::paths::Paths::from_env();
    let home = paths.home_dir();
    if home.is_empty() {
        return None;
    }
    Some(std::path::Path::new(home).join("ks-prefs.json"))
}

#[cfg(not(target_os = "macos"))]
fn prefs_load() -> serde_json::Map<String, serde_json::Value> {
    prefs_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

#[cfg(not(target_os = "macos"))]
fn prefs_save(map: &serde_json::Map<String, serde_json::Value>) {
    let Some(path) = prefs_path() else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string_pretty(map) {
        let _ = std::fs::write(&path, text);
    }
}

#[cfg(not(target_os = "macos"))]
pub fn get_string(key: &str) -> Option<String> {
    prefs_load().get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
}

#[cfg(not(target_os = "macos"))]
pub fn set_string(key: &str, val: &str) {
    let mut map = prefs_load();
    map.insert(key.to_string(), serde_json::Value::String(val.to_string()));
    prefs_save(&map);
}

#[cfg(not(target_os = "macos"))]
pub fn get_bool(key: &str) -> bool {
    prefs_load().get(key).and_then(|v| v.as_bool()).unwrap_or(false)
}

#[cfg(not(target_os = "macos"))]
pub fn set_bool(key: &str, on: bool) {
    let mut map = prefs_load();
    map.insert(key.to_string(), serde_json::Value::Bool(on));
    prefs_save(&map);
}

#[cfg(not(target_os = "macos"))]
pub fn get_f64(key: &str) -> f64 {
    prefs_load().get(key).and_then(|v| v.as_f64()).unwrap_or(0.0)
}

#[cfg(not(target_os = "macos"))]
pub fn set_f64(key: &str, val: f64) {
    let mut map = prefs_load();
    if let Some(n) = serde_json::Number::from_f64(val) {
        map.insert(key.to_string(), serde_json::Value::Number(n));
    }
    prefs_save(&map);
}

#[cfg(not(target_os = "macos"))]
pub fn remove_key(key: &str) {
    let mut map = prefs_load();
    map.remove(key);
    prefs_save(&map);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live round-trip against the real `NSUserDefaults`. `#[ignore]`d because
    /// a plain `cargo test` must not write the user's defaults; run
    /// `cargo test -p ws-rs settings -- --ignored`. Uses a throwaway key and
    /// removes it afterwards.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "writes the user's real NSUserDefaults (throwaway key)"]
    fn macos_user_defaults_round_trip() {
        let key = format!("wsSeamTest.{}", std::process::id());
        remove_key(&key);
        assert_eq!(get_string(&key), None);
        set_string(&key, "seam");
        assert_eq!(get_string(&key).as_deref(), Some("seam"));
        set_bool(&key, true);
        assert!(get_bool(&key));
        set_f64(&key, 1.25);
        assert_eq!(get_f64(&key), 1.25);
        remove_key(&key);
    }
}
