//! App preferences addressed by a plain key (was direct `NSUserDefaults`
//! calls at each call site).
//!
//! macOS keeps the existing `NSUserDefaults.standardUserDefaults()` storage
//! verbatim — same keys, same value encodings — so nothing is migrated and
//! data stored by earlier builds stays readable. `NSUserDefaults` is
//! thread-safe and has no main-thread requirement, matching the calls this
//! replaces.
//!
//! `WS_DEFAULTS_SUITE=<name>` swaps the store for that suite: the MCP sandbox
//! sets it so a test copy of the app (same bundle id) never reads or writes
//! the live app's preferences — redirecting `HOME` does not move them.
//!
//! Non-macOS targets (a branch that does not compile on macOS) keep a flat
//! JSON object at `<config dir>/ks-prefs.json`, best effort: a missing or
//! malformed file reads as empty, writes rewrite the whole object.

/// The store: `standardUserDefaults`, or the `WS_DEFAULTS_SUITE` suite.
#[cfg(target_os = "macos")]
fn defaults() -> objc2::rc::Retained<objc2_foundation::NSUserDefaults> {
    use objc2::AllocAnyThread;
    use objc2_foundation::{NSString, NSUserDefaults};
    match std::env::var("WS_DEFAULTS_SUITE") {
        Ok(name) if !name.is_empty() => {
            NSUserDefaults::initWithSuiteName(NSUserDefaults::alloc(), Some(&NSString::from_str(&name)))
                .unwrap_or_else(NSUserDefaults::standardUserDefaults)
        }
        _ => NSUserDefaults::standardUserDefaults(),
    }
}

/// The string stored under `key` (`stringForKey`), if any.
#[cfg(target_os = "macos")]
pub fn get_string(key: &str) -> Option<String> {
    use objc2_foundation::NSString;
    defaults()
        .stringForKey(&NSString::from_str(key))
        .map(|s| s.to_string())
}

/// Store `val` under `key` (`setObject:forKey:` with an `NSString`).
#[cfg(target_os = "macos")]
pub fn set_string(key: &str, val: &str) {
    use objc2_foundation::NSString;
    unsafe {
        defaults()
            .setObject_forKey(Some(&NSString::from_str(val)), &NSString::from_str(key));
    }
}

/// The bool stored under `key` (`boolForKey`; false when unset).
#[cfg(target_os = "macos")]
pub fn get_bool(key: &str) -> bool {
    use objc2_foundation::NSString;
    defaults().boolForKey(&NSString::from_str(key))
}

/// Store a bool under `key` (`setBool:forKey:`).
#[cfg(target_os = "macos")]
pub fn set_bool(key: &str, on: bool) {
    use objc2_foundation::NSString;
    defaults().setBool_forKey(on, &NSString::from_str(key));
}

/// The f64 stored under `key` (`doubleForKey`; 0.0 when unset).
#[cfg(target_os = "macos")]
pub fn get_f64(key: &str) -> f64 {
    use objc2_foundation::NSString;
    defaults().doubleForKey(&NSString::from_str(key))
}

/// Store an f64 under `key` (`setDouble:forKey:`).
#[cfg(target_os = "macos")]
pub fn set_f64(key: &str, val: f64) {
    use objc2_foundation::NSString;
    defaults().setDouble_forKey(val, &NSString::from_str(key));
}

/// Remove whatever is stored under `key` (`removeObjectForKey`).
#[cfg(target_os = "macos")]
pub fn remove_key(key: &str) {
    use objc2_foundation::NSString;
    defaults().removeObjectForKey(&NSString::from_str(key));
}

// -- Non-macOS: a flat JSON object at `<config dir>/ks-prefs.json` ------------

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
