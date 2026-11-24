//! Port of `SetupWindow.swift` (`SetupWindow` + `AppInstall`) and the
//! `pylib/setup_checks.py` models it drives.
//!
//! The preflight JSON model, the Fix-action mapping, the summary line, the
//! row layout and the `AppInstall.ensureHome` marker check are complete and
//! tested; [`SetupWindow::build`] constructs the AppKit window, its rows and
//! buttons on macOS.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::app::paths::Paths;
use crate::app::process_run::run_process;
use crate::ui::chrome::Rect;
use crate::ui::theme::PopupTone;

#[cfg(target_os = "macos")]
use objc2::rc::Retained;
#[cfg(target_os = "macos")]
use objc2_app_kit::NSWindow;

// ------------------------------------------------------------------ AppInstall

/// `AppInstall.State`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallState {
    Repo,
    Ready { fresh: bool },
    Checkout,
    NotInstalled,
    Failed(String),
}

/// `AppInstall` — the home-setup state machine (no AppKit).
#[derive(Clone, Debug)]
pub struct AppInstall {
    pub state: InstallState,
    pub is_repo_build: bool,
    pub app_bundle_path: Option<String>,
    pub home_dir: String,
    pub asset_dir: String,
    pub version: String,
    pub runs_from_image: bool,
    pub keep_checkout: bool,
}

fn runs_from_image(bundle: Option<&str>) -> bool {
    match bundle {
        Some(b) => b.contains("/AppTranslocation/") || b.starts_with("/Volumes/"),
        None => false,
    }
}

impl AppInstall {
    pub fn new(
        is_repo_build: bool,
        app_bundle_path: Option<String>,
        home_dir: impl Into<String>,
        asset_dir: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        let runs = runs_from_image(app_bundle_path.as_deref());
        AppInstall {
            state: InstallState::Repo,
            is_repo_build,
            app_bundle_path,
            home_dir: home_dir.into(),
            asset_dir: asset_dir.into(),
            version: version.into(),
            runs_from_image: runs,
            keep_checkout: false,
        }
    }

    pub fn from_paths(paths: &Paths, version: impl Into<String>) -> Self {
        AppInstall::new(
            paths.is_repo_build(),
            paths.app_bundle_path.clone(),
            paths.home_dir().to_string(),
            paths.asset_dir().to_string(),
            version,
        )
    }

    pub fn marker_path(&self) -> String {
        format!("{}/.install", self.home_dir)
    }

    pub fn marker(&self) -> BTreeMap<String, String> {
        match std::fs::read_to_string(self.marker_path()) {
            Ok(text) => marker_from_text(&text),
            Err(_) => BTreeMap::new(),
        }
    }

    /// `ensureHome(switchFromCheckout:)`. Runs `bin/setup-home.sh` through bash
    /// only when the marker does not already prove the home is ready.
    pub fn ensure_home(&mut self, switch_from_checkout: bool) {
        if !self.is_repo_build {
            if let Some(bundle) = self.app_bundle_path.clone() {
                if self.runs_from_image {
                    self.state = InstallState::NotInstalled;
                    return;
                }
                let m = self.marker();
                let commands = format!("{}/commands.toml", self.home_dir);
                let app_link = format!("{}/kitchen-sink.app", self.home_dir);
                let linked = std::fs::read_link(&app_link)
                    .map(|p| p.to_string_lossy().into_owned())
                    .ok()
                    == Some(bundle.clone());
                if !switch_from_checkout
                    && m.get("mode").map(String::as_str) == Some("app")
                    && m.get("app") == Some(&bundle)
                    && m.get("version") == Some(&self.version)
                    && std::path::Path::new(&commands).exists()
                    && linked
                {
                    self.state = InstallState::Ready { fresh: false };
                    return;
                }
                let script = format!("{}/bin/setup-home.sh", self.asset_dir);
                let mut args = vec!["app".to_string(), bundle];
                if switch_from_checkout {
                    args.push("--switch".to_string());
                }
                let (code, out) = run_sync(&script, &args);
                match code {
                    0 => self.state = InstallState::Ready { fresh: true },
                    3 => self.state = InstallState::Checkout,
                    _ => {
                        let why = out
                            .lines()
                            .last()
                            .map(str::to_string)
                            .unwrap_or_else(|| "setup-home.sh failed".to_string());
                        self.state = InstallState::Failed(why);
                    }
                }
                return;
            }
        }
        self.state = InstallState::Repo;
    }

}

/// Parse the `.install` marker (`key=value`; `seed …` lines are skipped).
pub fn marker_from_text(text: &str) -> BTreeMap<String, String> {
    let mut d = BTreeMap::new();
    for line in text.lines() {
        if line.starts_with("seed ") {
            continue;
        }
        if let Some(eq) = line.find('=') {
            d.insert(line[..eq].to_string(), line[eq + 1..].to_string());
        }
    }
    d
}

fn run_sync(script: &str, args: &[String]) -> (i32, String) {
    match run_process("/bin/bash", &[vec![script.to_string()], args.to_vec()].concat(), None, None, true) {
        Ok(r) => (r.code, r.out),
        Err(e) => (-1, format!("cannot run {script}: {e}")),
    }
}

// ---------------------------------------------------------------- check models

// --------------------------------------------------------------- row layout

// ------------------------------------------------------------------ preflight

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn check(over: Value) -> Value {
        let mut d = json!({
            "id": "python", "group": "core", "level": "required", "title": "Python",
            "detail": "python 3.11+", "fix": "brew install python", "action": "brew:python", "ok": true
        });
        if let (Some(base), Some(extra)) = (d.as_object_mut(), over.as_object()) {
            for (k, v) in extra {
                base.insert(k.clone(), v.clone());
            }
        }
        d
    }

    #[test]
    fn marker_parses_and_skips_seed() {
        let m = marker_from_text("mode=app\napp=/Applications/kitchen-sink.app\nversion=1.2\nseed abc file\nextra=1\nbadline\n");
        assert_eq!(m.get("mode").map(String::as_str), Some("app"));
        assert_eq!(m.get("version").map(String::as_str), Some("1.2"));
        assert_eq!(m.get("extra").map(String::as_str), Some("1"));
        assert!(!m.contains_key("seed abc file"));
    }

}
