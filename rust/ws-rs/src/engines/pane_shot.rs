//! Port of `PaneShot.swift` — the pane-shot CLI grammar, the `[pane-shot]`
//! config and the `herdr` wrapper.
//!
//! The Swift type delegates arg/config parsing and the JSON envelope to
//! `pylib/paneshot.py`; this is the pure-Rust mirror of that python reference
//! (the accepted port of the Swift suite), so the logic is testable without
//! the helper. Process spawning goes through `run_process`.

use std::collections::HashMap;
use std::path::Path;

use crate::app::process_run::run_process;

pub const MAX_LINES: i64 = 1000;
pub const USAGE: &str = "pane-shot [--pane ID] [--lines N|all] [--file PATH|-] [--no-save] [--no-copy]";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub message: String,
}

impl Problem {
    pub fn new(message: impl Into<String>) -> Self {
        Problem {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Problem {}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PaneShotArgs {
    pub pane: Option<String>,
    pub lines: Option<i64>,
    pub all: bool,
    pub file: Option<String>,
    pub save: Option<bool>,
    pub copy: Option<bool>,
}

impl PaneShotArgs {
    pub fn parse(words: &[String]) -> Result<PaneShotArgs, Problem> {
        let mut a = PaneShotArgs::default();
        let mut i = 0usize;
        while i < words.len() {
            let w = words[i].as_str();
            match w {
                "--pane" | "-p" => a.pane = Some(value(words, &mut i, w)?),
                "--lines" | "-n" => {
                    let v = value(words, &mut i, w)?;
                    if v == "all" {
                        a.all = true;
                    } else {
                        let n: i64 = v.trim().parse().unwrap_or(-1);
                        if n < 0 {
                            return Err(Problem::new(format!(
                                "--lines takes a number or \"all\", not {v}"
                            )));
                        }
                        a.lines = Some(n);
                    }
                }
                "--file" | "-f" => a.file = Some(value(words, &mut i, w)?),
                "--save" => a.save = Some(true),
                "--no-save" => a.save = Some(false),
                "--copy" => a.copy = Some(true),
                "--no-copy" => a.copy = Some(false),
                "-h" | "--help" => {
                    return Err(Problem::new(format!("usage: kitchen-sink {USAGE}")))
                }
                _ => {
                    return Err(Problem::new(format!(
                        "unknown argument {w}\nusage: kitchen-sink {USAGE}"
                    )))
                }
            }
            i += 1;
        }
        Ok(a)
    }
}

fn value(words: &[String], i: &mut usize, flag: &str) -> Result<String, Problem> {
    *i += 1;
    if *i >= words.len() || words[*i].is_empty() {
        return Err(Problem::new(format!("{flag} needs a value")));
    }
    Ok(words[*i].clone())
}

#[derive(Debug, Clone, PartialEq)]
pub struct PaneShotConfig {
    pub lines: i64,
    pub save: bool,
    pub copy: bool,
    pub preview: bool,
    pub herdr_bin: String,
    pub ghostty_bin: String,
    pub font: String,
    pub font_size: f64,
    pub background: String,
    pub padding: f64,
    pub save_path: String,
    pub filename_pattern: String,
    pub toast: String,
}

impl Default for PaneShotConfig {
    fn default() -> Self {
        PaneShotConfig {
            lines: 200,
            save: true,
            copy: true,
            preview: true,
            herdr_bin: "~/.local/bin/herdr".to_string(),
            ghostty_bin: "/Applications/Ghostty.app/Contents/MacOS/ghostty".to_string(),
            font: String::new(),
            font_size: 0.0,
            background: String::new(),
            padding: 16.0,
            save_path: String::new(),
            filename_pattern: "%F_%H-%M-%S pane".to_string(),
            toast: "Screenshot of {pane} copied ({n} lines)".to_string(),
        }
    }
}

impl PaneShotConfig {
    pub fn from_entries(entries: &HashMap<String, String>) -> PaneShotConfig {
        let d = PaneShotConfig::default();
        let s = |k: &str, dflt: &str| -> String {
            match entries.get(k) {
                Some(v) if !v.trim().is_empty() => v.trim().to_string(),
                _ => dflt.to_string(),
            }
        };
        let n = |k: &str, dflt: f64, lo: f64, hi: f64| -> f64 {
            let v = entries
                .get(k)
                .and_then(|s| s.trim().parse::<f64>().ok())
                .unwrap_or(dflt);
            v.max(lo).min(hi)
        };
        PaneShotConfig {
            lines: n("lines", d.lines as f64, 0.0, MAX_LINES as f64) as i64,
            save: tri(entries.get("save").map(String::as_str)).unwrap_or(d.save),
            copy: tri(entries.get("copy").map(String::as_str)).unwrap_or(d.copy),
            preview: tri(entries.get("preview").map(String::as_str)).unwrap_or(d.preview),
            herdr_bin: s("herdr-bin", &d.herdr_bin),
            ghostty_bin: s("ghostty-bin", &d.ghostty_bin),
            font: s("font", &d.font),
            font_size: n("font-size", d.font_size, 0.0, 72.0),
            background: s("background", &d.background),
            padding: n("padding", d.padding, 0.0, 200.0),
            save_path: s("save-path", &d.save_path),
            filename_pattern: s("filename-pattern", &d.filename_pattern),
            toast: match entries.get("toast") {
                Some(t) => t.clone(),
                None => d.toast,
            },
        }
    }
}

fn tri(s: Option<&str>) -> Option<bool> {
    match s.map(str::to_lowercase).as_deref() {
        Some("true") | Some("yes") | Some("1") | Some("on") => Some(true),
        Some("false") | Some("no") | Some("0") | Some("off") => Some(false),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub message: String,
}

impl Failure {
    pub fn new(message: impl Into<String>) -> Self {
        Failure {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Failure {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pane {
    pub id: String,
    pub title: String,
    pub viewport_rows: i64,
}

pub struct Herdr;

impl Herdr {
    pub const MAX_LINES: i64 = MAX_LINES;

    pub fn pane_from_json(text: &str) -> Result<Pane, Failure> {
        let d: serde_json::Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(_) => serde_json::Value::Null,
        };
        let Some(obj) = d.as_object() else {
            return Err(Failure::new("herdr answered no JSON"));
        };
        if let Some(err) = obj.get("error").and_then(|e| e.as_object()) {
            let msg = err.get("message").and_then(|m| m.as_str());
            return Err(Failure::new(msg.unwrap_or("herdr error")));
        }
        let p = obj
            .get("result")
            .and_then(|r| r.as_object())
            .and_then(|r| r.get("pane"))
            .and_then(|p| p.as_object());
        let Some(p) = p else {
            return Err(Failure::new("herdr answered without a pane"));
        };
        let Some(pane_id) = p.get("pane_id").and_then(|v| v.as_str()) else {
            return Err(Failure::new("herdr answered without a pane"));
        };
        let rows = p
            .get("scroll")
            .and_then(|s| s.as_object())
            .and_then(|s| s.get("viewport_rows"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let mut title = p
            .get("terminal_title_stripped")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        if title.is_none() {
            title = p
                .get("agent")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string);
        }
        let title = title.unwrap_or_else(|| pane_id.to_string());
        Ok(Pane {
            id: pane_id.to_string(),
            title,
            viewport_rows: rows,
        })
    }

    pub fn lines(viewport: i64, history: i64, all: bool) -> i64 {
        if all {
            MAX_LINES
        } else {
            MAX_LINES.min(viewport.saturating_add(history).max(1))
        }
    }

    pub fn environment(env: &HashMap<String, String>) -> HashMap<String, String> {
        const DROP: [&str; 3] = ["HERDR_PANE_ID", "HERDR_TAB_ID", "HERDR_WORKSPACE_ID"];
        env.iter()
            .filter(|(k, _)| !DROP.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    pub fn run(bin: &str, args: &[String]) -> Result<String, Failure> {
        let exe = expand_tilde(bin);
        if !is_executable_file(&exe) {
            return Err(Failure::new(format!(
                "herdr not found at {bin} ([pane-shot] herdr-bin)"
            )));
        }
        let env = Self::environment(&current_environment());
        let pairs: Vec<(String, String)> = env.into_iter().collect();
        let r = match run_process(&exe, args, None, Some(&pairs), false) {
            Ok(r) => r,
            Err(_) => return Err(Failure::new(format!("could not run {bin}"))),
        };
        if r.code != 0 {
            let msg = r.err.trim().to_string();
            if msg.starts_with('{') {
                if let Err(f) = Self::pane_from_json(&msg) {
                    return Err(f);
                }
            }
            return Err(Failure::new(if msg.is_empty() {
                format!("herdr exited {}", r.code)
            } else {
                msg
            }));
        }
        Ok(r.out)
    }

    pub fn pane(bin: &str, id: Option<&str>) -> Result<Pane, Failure> {
        let args: Vec<String> = match id {
            Some(id) => vec!["pane".to_string(), "get".to_string(), id.to_string()],
            None => vec!["pane".to_string(), "current".to_string()],
        };
        Self::run(bin, &args).and_then(|out| Self::pane_from_json(&out))
    }

    pub fn read(bin: &str, id: &str, lines: i64) -> Result<String, Failure> {
        let args = vec![
            "pane".to_string(),
            "read".to_string(),
            id.to_string(),
            "--source".to_string(),
            "recent".to_string(),
            "--format".to_string(),
            "ansi".to_string(),
            "--lines".to_string(),
            lines.to_string(),
        ];
        Self::run(bin, &args)
    }
}

fn current_environment() -> HashMap<String, String> {
    std::env::vars_os()
        .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
        .collect()
}

fn expand_tilde(bin: &str) -> String {
    if let Some(rest) = bin.strip_prefix('~') {
        if rest.is_empty() || rest.starts_with('/') {
            let home = std::env::var("HOME").unwrap_or_default();
            return format!("{home}{rest}");
        }
    }
    bin.to_string()
}

fn is_executable_file(path: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(Path::new(path)) {
        Ok(m) => m.is_file() && (m.permissions().mode() & 0o111) != 0,
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(words: &[&str]) -> Option<PaneShotArgs> {
        let owned: Vec<String> = words.iter().map(|s| s.to_string()).collect();
        PaneShotArgs::parse(&owned).ok()
    }

    fn split(s: &str) -> Vec<String> {
        if s.is_empty() {
            Vec::new()
        } else {
            s.split(' ').map(str::to_string).collect()
        }
    }

    fn p(s: &str) -> Option<PaneShotArgs> {
        PaneShotArgs::parse(&split(s)).ok()
    }

    fn entries(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn args_no_args() {
        assert_eq!(p(""), Some(PaneShotArgs::default()), "no args");
    }

    #[test]
    fn args_pane_and_lines() {
        assert_eq!(parse(&["--pane", "wB:p1", "--lines", "50"]).unwrap().pane, Some("wB:p1".into()));
        assert_eq!(parse(&["--lines", "50"]).unwrap().lines, Some(50), "pane + lines");
        assert!(p("-n all").unwrap().all, "lines all");
    }

    #[test]
    fn args_file_and_switches() {
        let a = p("--file - --no-save --no-copy").unwrap();
        assert_eq!((a.file, a.save, a.copy), (Some("-".into()), Some(false), Some(false)), "file + switches");
    }

    #[test]
    fn args_bad_refused() {
        for s in ["--lines x", "--lines", "--bogus", "--lines -3"] {
            assert_eq!(p(s), None, "{s}");
        }
    }

    #[test]
    fn config_defaults() {
        let d = PaneShotConfig::from_entries(&HashMap::new());
        assert_eq!(d.lines, 200);
        assert!(d.save && d.copy);
        assert_eq!(d.font, "");
        assert_eq!(d.font_size, 0.0);
    }

    #[test]
    fn config_values_and_clamps() {
        let c = PaneShotConfig::from_entries(&entries(&[
            ("lines", "5000"),
            ("save", "false"),
            ("font-size", "14"),
            ("padding", "-3"),
            ("herdr-bin", "/x/herdr"),
        ]));
        assert_eq!(c.lines, MAX_LINES, "lines clamped to herdr's cap");
        assert!(!c.save);
        assert_eq!(c.font_size, 14.0);
        assert_eq!(c.padding, 0.0);
        assert_eq!(c.herdr_bin, "/x/herdr", "values");
    }

    const OK: &str = "{\"id\":\"cli:pane:current\",\"result\":{\"pane\":{\"pane_id\":\"wB:p1J\",\"scroll\":{\"viewport_rows\":60},\"terminal_title_stripped\":\"build things\",\"agent\":\"claude\"},\"type\":\"pane_current\"}}";

    #[test]
    fn herdr_pane_parsed() {
        assert_eq!(
            Herdr::pane_from_json(OK).unwrap(),
            Pane {
                id: "wB:p1J".into(),
                title: "build things".into(),
                viewport_rows: 60
            }
        );
    }

    #[test]
    fn herdr_no_title_falls_back_to_id() {
        let bare = "{\"result\":{\"pane\":{\"pane_id\":\"wB:p2\",\"scroll\":{\"viewport_rows\":26}}}}";
        assert_eq!(Herdr::pane_from_json(bare).unwrap().title, "wB:p2");
    }

    #[test]
    fn herdr_error_envelope() {
        let err = "{\"id\":\"x\",\"error\":{\"code\":\"server_not_running\",\"message\":\"no herdr server is running\"}}";
        assert_eq!(Herdr::pane_from_json(err).unwrap_err().message, "no herdr server is running");
    }

    #[test]
    fn herdr_lines() {
        assert_eq!(Herdr::lines(60, 200, false), 260, "screen + history");
        assert_eq!(Herdr::lines(60, 5000, false), MAX_LINES, "capped");
        assert_eq!(Herdr::lines(60, 0, true), MAX_LINES, "all");
    }

    #[test]
    fn herdr_environment_drops_pane_vars() {
        let env = entries(&[
            ("HERDR_PANE_ID", "a"),
            ("HERDR_TAB_ID", "b"),
            ("HERDR_SOCKET_PATH", "/s"),
            ("HOME", "/h"),
        ]);
        assert_eq!(
            Herdr::environment(&env),
            entries(&[("HERDR_SOCKET_PATH", "/s"), ("HOME", "/h")]),
            "pane env dropped, socket kept"
        );
    }

    #[test]
    fn herdr_non_json_and_missing_pane() {
        assert!(Herdr::pane_from_json("not json").is_err());
        assert!(Herdr::pane_from_json("{\"result\":{}}").is_err());
    }

    #[test]
    fn herdr_stderr_error_surfaces() {
        use std::os::unix::fs::PermissionsExt;
        let path = std::env::temp_dir().join(format!("fake-herdr-{}", std::process::id()));
        let script = "#!/bin/sh\necho '{\"error\":{\"code\":\"pane_not_found\",\"message\":\"pane x not found\"}}' >&2\nexit 1\n";
        std::fs::write(&path, script).unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        let res = Herdr::pane(path.to_str().unwrap(), Some("x"));
        let _ = std::fs::remove_file(&path);
        let f = res.expect_err("expected failure");
        assert_eq!(f.message, "pane x not found", "herdr's stderr error surfaces");
    }

    #[test]
    fn herdr_missing_binary_names_config_key() {
        let res = Herdr::run(
            "/no/such/herdr",
            &["pane".to_string(), "current".to_string()],
        );
        let f = res.expect_err("expected failure");
        assert!(f.message.contains("herdr-bin"), "{}", f.message);
    }
}
