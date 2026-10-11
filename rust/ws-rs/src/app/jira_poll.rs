//! `JiraPoll` + `toggleJiraPoll` / `enableJiraChecked` / `setJiraEnabled` /
//! `syncJiraLaunchAgent` (kitchen_sink.swift): turning the Jira poller on and
//! off from `kitchen-sink jira-poll on|off|toggle` and the launchd agent that
//! runs `jira_poll.py` every minute.

use crate::app::config;
use crate::app::paths::Paths;
use serde_json::Value;
use std::process::Command;

fn jira_dir() -> String {
    format!("{}/jira", Paths::from_env().asset_dir())
}

/// `configSectionValue("jira", key)` as a tri-state flag.
fn jira_flag(key: &str) -> bool {
    let text = config::read_config_text().unwrap_or_default();
    let mut section = String::new();
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            section = t[1..t.len() - 1].trim().to_string();
            continue;
        }
        if section != "jira" {
            continue;
        }
        if let Some((k, v)) = t.split_once('=') {
            if k.trim() == key {
                let v = v.trim().trim_matches('"');
                return crate::engines::config_text::tri(Some(v)) == Some(true);
            }
        }
    }
    false
}

/// `jiraEnabledInConfig()`.
pub fn enabled_in_config() -> bool {
    jira_flag("enabled")
}

/// `jiraPollActiveInConfig()`: enabled, or polling kept while disabled.
pub fn poll_active_in_config() -> bool {
    enabled_in_config() || jira_flag("poll-when-disabled")
}

fn python() -> String {
    for c in ["/opt/homebrew/bin/python3", "/usr/local/bin/python3"] {
        if std::path::Path::new(c).exists() {
            return c.to_string();
        }
    }
    std::env::var("WS_PYTHON").ok().filter(|p| !p.is_empty()).unwrap_or_else(|| "python3".into())
}

/// `JiraPoll.run(script, args)` — blocking; call off the main thread.
pub fn run(script: &str, args: &[&str]) -> (i32, String, String) {
    run_stdin(script, args, None)
}

/// `JiraPoll.run(script, args, stdin:)` — blocking; call off the main thread.
pub fn run_stdin(script: &str, args: &[&str], stdin: Option<&str>) -> (i32, String, String) {
    use std::io::Write;
    use std::process::Stdio;
    let child = Command::new(python())
        .arg(format!("{}/{script}", jira_dir()))
        .args(args)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut c| {
            if let (Some(text), Some(mut pipe)) = (stdin, c.stdin.take()) {
                let _ = pipe.write_all(text.as_bytes());
            }
            c.wait_with_output()
        });
    match child
    {
        Ok(o) => (
            o.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&o.stdout).into_owned(),
            String::from_utf8_lossy(&o.stderr).into_owned(),
        ),
        Err(e) => (-1, String::new(), e.to_string()),
    }
}

/// `JiraPoll.errorLine(err, fallback:)`: the last non-empty stderr line.
pub fn error_line(err: &str, fallback: &str) -> String {
    err.lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| fallback.to_string())
}

/// What `jira_config.py --check` decided about enabling.
#[derive(Debug, PartialEq)]
pub enum EnableCheck {
    Ok,
    /// Needs the setup window: the reason to show.
    Setup(String),
    /// The login test failed.
    Login(String),
}

/// The pure half of `enableJiraChecked`: read the `--check` JSON.
pub fn read_check(out: &str, err: &str) -> Result<(), String> {
    let chk: Option<Value> = serde_json::from_str(out).ok();
    let problems: Vec<String> = match chk.as_ref().and_then(|c| c.get("problems")).and_then(Value::as_array) {
        Some(a) => a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
        None => vec![error_line(err, "config unreadable")],
    };
    let exists = chk.as_ref().and_then(|c| c.get("exists")).and_then(Value::as_bool) == Some(true);
    if !exists || !problems.is_empty() {
        return Err(if problems.is_empty() {
            "No Jira config yet — fill in your site and API token.".into()
        } else {
            problems.join("\n")
        });
    }
    let keys = chk
        .as_ref()
        .and_then(|c| c.get("projectKeys"))
        .and_then(Value::as_array)
        .map_or(0, |a| a.len());
    if keys == 0 {
        return Err("Enter the projects in scope — every Jira query is limited to them.".into());
    }
    Ok(())
}

/// `enableJiraChecked`'s checks (config, then a login test). Blocking.
pub fn enable_check() -> EnableCheck {
    let (_, out, err) = run("jira_config.py", &["--check"]);
    if let Err(reason) = read_check(&out, &err) {
        return EnableCheck::Setup(reason);
    }
    let (code, _, err) = run("jira_api.py", &["--myself"]);
    if code == 0 {
        return EnableCheck::Ok;
    }
    let msg = error_line(&err, &format!("login test failed (exit {code})"));
    run("jira_status.py", &["--note-error", &format!("login failed: {msg}")]);
    EnableCheck::Login(msg)
}

/// `setJiraEnabled(_:keepPolling:)`'s config write + agent sync.
pub fn set_enabled(on: bool, keep_polling: bool) {
    config::save_config_values(
        "jira",
        &[
            ("enabled".into(), Some(if on { "true" } else { "false" }.into())),
            ("poll-when-disabled".into(), (!on && keep_polling).then(|| "true".into())),
        ],
    );
    if on {
        run("jira_status.py", &["--note-error"]);
    }
    sync_launch_agent();
}

fn launchctl(args: &[&str]) -> i32 {
    Command::new("/bin/launchctl").args(args).output().map(|o| o.status.code().unwrap_or(-1)).unwrap_or(-1)
}

/// `syncJiraLaunchAgent()`: install/refresh `com.jira.poll.plist` while the
/// poller is active, boot it out otherwise.
pub fn sync_launch_agent() {
    let home = std::env::var("HOME").unwrap_or_default();
    let plist = format!("{home}/Library/LaunchAgents/com.jira.poll.plist");
    let uid = unsafe { libc::getuid() };
    let gui = format!("gui/{uid}");
    if !poll_active_in_config() {
        if std::path::Path::new(&plist).exists() {
            launchctl(&["bootout", &gui, &plist]);
        }
        return;
    }
    let mut changed = false;
    if let Ok(tmpl) = std::fs::read_to_string(format!("{}/com.jira.poll.plist", jira_dir())) {
        let want = tmpl.replace("__WS_CONFIG__", Paths::from_env().asset_dir());
        if std::fs::read_to_string(&plist).ok().as_deref() != Some(want.as_str()) {
            let _ = std::fs::create_dir_all(format!("{home}/Library/LaunchAgents"));
            changed = std::fs::write(&plist, want).is_ok();
        }
    }
    if !std::path::Path::new(&plist).exists() {
        return;
    }
    let loaded = launchctl(&["print", &format!("{gui}/com.jira.poll")]) == 0;
    if loaded && !changed {
        return;
    }
    if loaded {
        launchctl(&["bootout", &gui, &plist]);
    }
    launchctl(&["bootstrap", &gui, &plist]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_needs_config_projects_and_no_problems() {
        assert_eq!(
            read_check("", "boom\n"),
            Err("boom".to_string())
        );
        assert_eq!(
            read_check(r#"{"exists": false, "problems": []}"#, ""),
            Err("No Jira config yet — fill in your site and API token.".to_string())
        );
        assert_eq!(
            read_check(r#"{"exists": true, "problems": ["bad token", "no site"]}"#, ""),
            Err("bad token\nno site".to_string())
        );
        assert!(read_check(r#"{"exists": true, "problems": [], "projectKeys": []}"#, "").is_err());
        assert_eq!(read_check(r#"{"exists": true, "problems": [], "projectKeys": ["AB"]}"#, ""), Ok(()));
    }

    #[test]
    fn error_line_takes_last_nonempty() {
        assert_eq!(error_line("a\nb\n\n", "x"), "b");
        assert_eq!(error_line("  \n", "x"), "x");
    }
}
