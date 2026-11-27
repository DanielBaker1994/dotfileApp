//! The main daemon's hotkey launch messages (`SwitcherController.hotkeyModes`).
//! AeroSpace is gone: no window-manager IPC runs before a toggle.

/// The socket messages that toggle a view of the shared window.
pub const HOTKEY_MODES: &[&str] = &["window", "jira", "files", "confluence", "ai"];

pub fn is_hotkey_mode(name: &str) -> bool {
    HOTKEY_MODES.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkey_modes() {
        assert!(is_hotkey_mode("files"));
        assert!(is_hotkey_mode("window"));
        assert!(!is_hotkey_mode("screenshot"));
        assert!(!is_hotkey_mode("show"));
    }
}
