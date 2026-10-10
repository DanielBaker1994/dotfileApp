//! Port of the path constants at the top of `kitchen_sink.swift`
//! (`bundleParentDir` / `appBundlePath` / `isRepoBuild` / `homeDir` /
//! `assetDir` / `userDir` / `commandsConfPath` / `popupTmpDir` and the
//! socket + focus file names), plus the `JiraPaths` helper facade.

use crate::app::python_helper::PythonHelper;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const COMMANDS_CONF_NAME: &str = "commands.toml";
pub const NOTES_SOCKET_NAME: &str = "ws-notes.sock";
pub const FOCUS_FILE_NAME: &str = "kitchen-sink-focus";
pub const FOCUS_BRIDGE_NAME: &str = "ws-aerospace-focus";
pub const SWITCHER_WINDOW_NAME: &str = "kitchen-sink";

/// `popupTmpDir()` — `$TMPDIR`, else `/tmp/`, always with a trailing slash.
pub fn popup_tmp_dir(tmpdir: Option<&str>) -> String {
    let t = tmpdir.unwrap_or("");
    let d = if t.is_empty() { "/tmp/" } else { t };
    if d.ends_with('/') {
        d.to_string()
    } else {
        format!("{d}/")
    }
}

fn parent_of(path: &str) -> String {
    Path::new(path)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Mirror of `URL(...).resolvingSymlinksInPath().path`: canonicalize when the
/// path exists, else lexically absolutize (Swift still returns a path).
fn resolve_symlinks(path: &str) -> String {
    if let Ok(c) = std::fs::canonicalize(path) {
        return c.to_string_lossy().into_owned();
    }
    if path.starts_with('/') {
        return PathBuf::from(path).to_string_lossy().into_owned();
    }
    match std::env::current_dir() {
        Ok(cwd) => cwd.join(path).to_string_lossy().into_owned(),
        Err(_) => path.to_string(),
    }
}

fn compute_bundle_parent_dir(resolved: &str) -> String {
    if let Some(idx) = resolved.find("/Contents/MacOS/") {
        let mut d = resolved[..idx].to_string();
        if d.ends_with(".app") {
            d = parent_of(&d);
        }
        d
    } else {
        parent_of(resolved)
    }
}

fn compute_app_bundle_path(resolved: &str) -> Option<String> {
    resolved
        .find("/Contents/MacOS/")
        .map(|idx| resolved[..idx].to_string())
}

#[derive(Debug, Clone)]
pub struct Paths {
    pub argv0: String,
    pub bundle_parent_dir: String,
    pub app_bundle_path: Option<String>,
    pub is_repo_build: bool,
    pub home_dir: String,
    pub asset_dir: String,
    pub user_dir: String,
    pub tmp_dir: String,
    pub notes_socket_name: String,
    pub focus_file_name: String,
    pub focus_bridge_name: String,
    pub switcher_window_name: String,
}

impl Paths {
    /// Resolve with explicit inputs so tests never rely on the real env.
    pub fn resolve_at(
        argv0: &str,
        ws_home: Option<&str>,
        tmpdir: Option<&str>,
        home: Option<&str>,
    ) -> Paths {
        let resolved = resolve_symlinks(argv0);
        let bundle_parent_dir = compute_bundle_parent_dir(&resolved);
        let app_bundle_path = compute_app_bundle_path(&resolved);

        let fm_stub = |rel: &str| Path::new(&bundle_parent_dir).join(rel).exists();
        let is_repo_build = fm_stub(COMMANDS_CONF_NAME) && fm_stub("bin/build-app.sh");

        let home_dir = match ws_home {
            Some(h) if !h.is_empty() => h.to_string(),
            _ => {
                let h = home.unwrap_or("");
                format!("{h}/.config/kitchen-sink")
            }
        };

        let asset_dir = if is_repo_build {
            bundle_parent_dir.clone()
        } else if let Some(dir) = std::env::var("WS_RS_ASSET_DIR")
            .ok()
            .filter(|d| !d.is_empty())
        {
            // Dev override: a non-dist bundle has no Resources payload, so the
            // daemon can be pointed at a checkout (`pylib/`, `vim/`, `rules/`).
            dir
        } else if let Some(b) = &app_bundle_path {
            format!("{b}/Contents/Resources")
        } else {
            bundle_parent_dir.clone()
        };
        let user_dir = if is_repo_build {
            bundle_parent_dir.clone()
        } else {
            home_dir.clone()
        };

        Paths {
            argv0: argv0.to_string(),
            bundle_parent_dir,
            app_bundle_path,
            is_repo_build,
            home_dir,
            asset_dir,
            user_dir,
            tmp_dir: popup_tmp_dir(tmpdir),
            notes_socket_name: NOTES_SOCKET_NAME.to_string(),
            focus_file_name: FOCUS_FILE_NAME.to_string(),
            focus_bridge_name: FOCUS_BRIDGE_NAME.to_string(),
            switcher_window_name: SWITCHER_WINDOW_NAME.to_string(),
        }
    }

    pub fn from_env() -> Paths {
        let argv0 = std::env::args().next().unwrap_or_default();
        let ws_home = std::env::var("WS_HOME").ok();
        let tmpdir = std::env::var("TMPDIR").ok();
        let home = std::env::var("HOME").ok();
        Paths::resolve_at(
            &argv0,
            ws_home.as_deref(),
            tmpdir.as_deref(),
            home.as_deref(),
        )
    }

    pub fn is_repo_build(&self) -> bool {
        self.is_repo_build
    }

    pub fn asset_dir(&self) -> &str {
        &self.asset_dir
    }

    pub fn user_dir(&self) -> &str {
        &self.user_dir
    }

    pub fn home_dir(&self) -> &str {
        &self.home_dir
    }

    pub fn commands_conf_path(&self) -> String {
        format!("{}/{}", self.user_dir, COMMANDS_CONF_NAME)
    }

    pub fn popup_tmp_dir(&self) -> String {
        self.tmp_dir.clone()
    }

    pub fn focus_file_path(&self) -> String {
        format!("{}{}", self.tmp_dir, self.focus_file_name)
    }

    pub fn notes_socket_path(&self) -> String {
        format!("{}{}", self.tmp_dir, self.notes_socket_name)
    }

    pub fn focus_bridge_path(&self) -> String {
        format!("{}{}", self.tmp_dir, self.focus_bridge_name)
    }
}

/// `JiraPaths` — resolved once through the helper (`jira.paths`, the same
/// method the Swift reads and `jira_paths.py` owns). A failed resolution is
/// an EXPLICIT state so Jira is disabled visibly; it never turns into
/// relative / empty paths (REVIEW-architecture #6b).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JiraPaths {
    pub config_json: String,
    pub team_json: String,
    pub legacy_config: String,
    pub cache_dir: String,
    pub out_dir: String,
    pub cache: BTreeMap<String, String>,
    pub tabs: BTreeMap<String, String>,
    pub side_dirs: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JiraPathsState {
    Resolved(JiraPaths),
    Failed(String),
}

impl JiraPaths {
    pub fn from_helper_value(v: &Value) -> JiraPathsState {
        let s = |k: &str| -> String {
            v.get(k)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        let map = |k: &str| -> BTreeMap<String, String> {
            v.get(k)
                .and_then(Value::as_object)
                .map(|o| {
                    o.iter()
                        .filter_map(|(key, val)| {
                            val.as_str().map(|s| (key.clone(), s.to_string()))
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        let config_json = s("configJson");
        let cache_dir = s("cacheDir");
        if config_json.is_empty() || cache_dir.is_empty() {
            return JiraPathsState::Failed(
                "jira: paths unresolved (helper returned empty paths)".into(),
            );
        }
        JiraPathsState::Resolved(JiraPaths {
            config_json,
            team_json: s("teamJson"),
            legacy_config: s("legacyConfig"),
            cache_dir,
            out_dir: s("outDir"),
            cache: map("cache"),
            tabs: map("tabs"),
            side_dirs: map("sideDirs"),
        })
    }

    pub fn resolve() -> JiraPathsState {
        match PythonHelper::shared().call_default("jira.paths", json!({})) {
            Ok(v) => JiraPaths::from_helper_value(&v),
            Err(e) => JiraPathsState::Failed(format!("jira: paths unresolved ({e})")),
        }
    }

    pub fn cache_file(&self, name: &str) -> String {
        let file = self.cache.get(name).map(String::as_str).unwrap_or(name);
        Path::new(&self.cache_dir)
            .join(file)
            .to_string_lossy()
            .into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(name: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!("ws-rs-paths-{}-{}", std::process::id(), name));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        fs::canonicalize(&base).unwrap()
    }

    #[test]
    fn repo_build_detects_markers() {
        let root = tmp("repo");
        fs::write(root.join(COMMANDS_CONF_NAME), "").unwrap();
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::write(root.join("bin/build-app.sh"), "").unwrap();

        let bin = root.join("kitchen-sink");
        fs::write(&bin, "").unwrap();
        let p = Paths::resolve_at(bin.to_str().unwrap(), None, None, Some("/Users/x"));

        assert!(p.is_repo_build());
        assert_eq!(p.asset_dir(), root.to_str().unwrap());
        assert_eq!(p.user_dir(), root.to_str().unwrap());
        assert_eq!(
            p.commands_conf_path(),
            root.join(COMMANDS_CONF_NAME).to_str().unwrap()
        );
    }

    #[test]
    fn app_build_uses_bundle_resources_and_home() {
        let root = tmp("app");
        let app = root.join("Kitchen Sink.app");
        let macos = app.join("Contents/MacOS");
        fs::create_dir_all(&macos).unwrap();
        let bin = macos.join("kitchen-sink");
        fs::write(&bin, "").unwrap();

        let p = Paths::resolve_at(
            bin.to_str().unwrap(),
            None,
            Some("/tmp/ws-tmp"),
            Some("/Users/x"),
        );

        assert!(!p.is_repo_build());
        assert_eq!(p.app_bundle_path.as_deref(), Some(app.to_str().unwrap()));
        assert_eq!(
            p.asset_dir(),
            app.join("Contents/Resources").to_str().unwrap()
        );
        assert_eq!(p.user_dir(), "/Users/x/.config/kitchen-sink");
        assert_eq!(p.commands_conf_path(), "/Users/x/.config/kitchen-sink/commands.toml");
    }

    #[test]
    fn ws_home_overrides_home_dir() {
        let root = tmp("home");
        let bin = root.join("kitchen-sink");
        fs::write(&bin, "").unwrap();
        let p = Paths::resolve_at(bin.to_str().unwrap(), Some("/custom/home"), None, Some("/Users/x"));
        assert_eq!(p.user_dir(), "/custom/home");
    }

    #[test]
    fn partial_markers_are_not_repo_build() {
        let root = tmp("partial");
        fs::write(root.join(COMMANDS_CONF_NAME), "").unwrap();
        let bin = root.join("kitchen-sink");
        fs::write(&bin, "").unwrap();
        let p = Paths::resolve_at(bin.to_str().unwrap(), None, None, Some("/Users/x"));
        assert!(!p.is_repo_build());
        assert_eq!(p.asset_dir(), root.to_str().unwrap());
    }

    #[test]
    fn tmp_dir_trailing_slash_and_unix_socket_names() {
        assert_eq!(popup_tmp_dir(None), "/tmp/");
        assert_eq!(popup_tmp_dir(Some("")), "/tmp/");
        assert_eq!(popup_tmp_dir(Some("/var/x")), "/var/x/");
        assert_eq!(popup_tmp_dir(Some("/var/x/")), "/var/x/");

        let root = tmp("sockets");
        let bin = root.join("kitchen-sink");
        fs::write(&bin, "").unwrap();
        let p = Paths::resolve_at(bin.to_str().unwrap(), None, Some("/var/x"), Some("/Users/x"));
        assert_eq!(p.focus_file_path(), "/var/x/kitchen-sink-focus");
        assert_eq!(p.notes_socket_path(), "/var/x/ws-notes.sock");
        assert_eq!(p.focus_bridge_path(), "/var/x/ws-aerospace-focus");
    }

    #[test]
    fn jira_paths_helper_value_parses() {
        let v = json!({
            "configJson": "/c/config.json",
            "teamJson": "/c/team.json",
            "legacyConfig": "/c/legacy.json",
            "cacheDir": "/c/cache",
            "outDir": "/c/out",
            "cache": {"status": "status.json"},
            "tabs": {"liveSearch": "search.json"},
            "sideDirs": {"boards": "jira_boards"}
        });
        match JiraPaths::from_helper_value(&v) {
            JiraPathsState::Resolved(p) => {
                assert_eq!(p.config_json, "/c/config.json");
                assert_eq!(p.cache_file("status"), "/c/cache/status.json");
                assert_eq!(p.cache_file("unknown"), "/c/cache/unknown");
                assert_eq!(p.tabs.get("liveSearch").unwrap(), "search.json");
            }
            JiraPathsState::Failed(e) => panic!("unexpected failure: {e}"),
        }
    }

    #[test]
    fn jira_paths_empty_is_an_explicit_failure() {
        let v = json!({"configJson": "", "cacheDir": ""});
        match JiraPaths::from_helper_value(&v) {
            JiraPathsState::Failed(_) => {}
            JiraPathsState::Resolved(_) => panic!("empty paths must not resolve"),
        }
        match JiraPaths::from_helper_value(&json!({})) {
            JiraPathsState::Failed(_) => {}
            JiraPathsState::Resolved(_) => panic!("missing paths must not resolve"),
        }
    }
}
