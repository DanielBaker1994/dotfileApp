//! Port of `SwitcherController.hotkeyModes` / `hotkeyPrep` and the AeroSpace
//! IPC it drives (`kitchen_sink.swift`). Queries go through `run_process`;
//! the focus file is written atomically.

use crate::app::process_run::run_process;
use std::time::Instant;

/// `SwitcherController.hotkeyModes` — the socket messages that run
/// `hotkeyPrep` before the main thread sees them.
pub const HOTKEY_MODES: &[&str] = &[
    "window",
    "notes",
    "voice",
    "jira",
    "files",
    "confluence",
    "ai",
    "compare",
];

pub fn is_hotkey_mode(name: &str) -> bool {
    HOTKEY_MODES.contains(&name)
}

/// `settings.aerospaceCLI` candidates, in order.
pub const AEROSPACE_CLI_CANDIDATES: &[&str] = &[
    "/opt/homebrew/bin/aerospace",
    "/usr/local/bin/aerospace",
    "aerospace",
];

/// Small injectable IPC surface so argument construction is unit-testable.
pub trait AeroCall: Send + Sync {
    fn call(&self, args: &[String]) -> Option<String>;
}

fn executable(path: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(path) {
        Ok(m) => m.is_file() && (m.permissions().mode() & 0o111) != 0,
        Err(_) => false,
    }
}

pub fn discover_aerospace_binary() -> String {
    for c in AEROSPACE_CLI_CANDIDATES {
        if c.contains('/') {
            if executable(c) {
                return (*c).to_string();
            }
        } else {
            return (*c).to_string();
        }
    }
    "aerospace".to_string()
}

pub struct AeroIpc {
    binary: String,
    runner: Box<dyn Fn(&[String]) -> Option<String> + Send + Sync>,
}

impl AeroIpc {
    pub fn new(binary: impl Into<String>) -> Self {
        let binary = binary.into();
        let exe = binary.clone();
        AeroIpc {
            binary,
            runner: Box::new(move |args: &[String]| {
                match run_process(&exe, args, None, None, false) {
                    Ok(o) if o.code == 0 => Some(o.out),
                    _ => None,
                }
            }),
        }
    }

    pub fn discover() -> Self {
        AeroIpc::new(discover_aerospace_binary())
    }

    pub fn with_runner(
        binary: impl Into<String>,
        runner: impl Fn(&[String]) -> Option<String> + Send + Sync + 'static,
    ) -> Self {
        AeroIpc {
            binary: binary.into(),
            runner: Box::new(runner),
        }
    }

    pub fn binary(&self) -> &str {
        &self.binary
    }
}

impl AeroCall for AeroIpc {
    fn call(&self, args: &[String]) -> Option<String> {
        (self.runner)(args)
    }
}

pub fn focused_args() -> Vec<String> {
    vec![
        "list-windows".into(),
        "--focused".into(),
        "--format".into(),
        "%{window-id} %{app-pid}".into(),
    ]
}

pub fn rows_args() -> Vec<String> {
    vec![
        "list-windows".into(),
        "--all".into(),
        "--format".into(),
        "%{window-id}|%{app-pid}|%{workspace}|%{workspace-is-focused}|%{monitor-appkit-nsscreen-screens-id}|%{window-title}".into(),
    ]
}

pub fn workspaces_args() -> Vec<String> {
    vec![
        "list-workspaces".into(),
        "--focused".into(),
        "--format".into(),
        "%{workspace}|%{monitor-appkit-nsscreen-screens-id}".into(),
    ]
}

pub fn eval_true_args() -> Vec<String> {
    vec!["eval".into(), "true".into()]
}

pub fn move_node_args(window_id: &str, workspace: &str) -> Vec<String> {
    vec![
        "move-node-to-workspace".into(),
        "--window-id".into(),
        window_id.to_string(),
        workspace.to_string(),
    ]
}

/// Split a `%{a}|%{b}|…` row into at most 6 fields, empty fields kept
/// (`split(separator:maxSplits:omittingEmptySubsequences: false)`).
pub fn parse_windows_table(rows: &str) -> Vec<Vec<String>> {
    rows.split('\n')
        .map(|line| {
            line.splitn(6, '|')
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        })
        .filter(|r| r.len() == 6)
        .collect()
}

/// `cur` + optional screen index from `workspace|screen`.
pub fn parse_workspace_field(ws: &str) -> (String, Option<i32>) {
    let mut parts = ws.splitn(2, '|');
    let cur = parts.next().unwrap_or("").to_string();
    let screen = parts.next().and_then(|s| s.parse::<i32>().ok());
    (cur, screen)
}

fn write_focus_file(path: &str, contents: &str) -> std::io::Result<()> {
    let p = std::path::Path::new(path);
    let tmp = p.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, p)
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct HotkeyPrep {
    pub log: String,
    pub workspace: String,
    pub screen: Option<i32>,
    pub cache_cleared: bool,
}

pub fn hotkey_prep(
    ipc: &dyn AeroCall,
    focus_file_path: &str,
    switcher_window_name: &str,
) -> HotkeyPrep {
    hotkey_prep_with_pid(
        ipc,
        focus_file_path,
        switcher_window_name,
        std::process::id() as i32,
    )
}

pub fn hotkey_prep_with_pid(
    ipc: &dyn AeroCall,
    focus_file_path: &str,
    switcher_window_name: &str,
    pid: i32,
) -> HotkeyPrep {
    let t0 = Instant::now();
    let focused_q = focused_args();
    let rows_q = rows_args();
    let eval_q = eval_true_args();

    // The focused-window query, the AeroSpace closed-windows cache clear and
    // the all-windows query run in parallel (Swift DispatchGroup).
    let (focused, cleared, rows) = std::thread::scope(|s| {
        let h_focused = s.spawn(|| ipc.call(&focused_q));
        let h_eval = s.spawn(|| ipc.call(&eval_q));
        let rows = ipc.call(&rows_q);
        let focused = h_focused.join().unwrap_or(None).unwrap_or_default();
        let cleared = h_eval.join().unwrap_or(None).is_some();
        (focused.trim().to_string(), cleared, rows.unwrap_or_default())
    });

    let table = parse_windows_table(&rows);
    let ws = if let Some(f) = table.iter().find(|r| r.get(3).map(String::as_str) == Some("true")) {
        format!("{}|{}", f[2], f[4])
    } else {
        ipc.call(&workspaces_args())
            .unwrap_or_default()
            .trim()
            .to_string()
    };

    if focused.split_whitespace().count() == 2 {
        let _ = write_focus_file(focus_file_path, &focused);
    } else {
        let _ = std::fs::remove_file(focus_file_path);
    }

    let (cur, screen) = parse_workspace_field(&ws);
    let me = pid.to_string();
    let mut moved: Vec<String> = Vec::new();
    for f in &table {
        if f[1] != me
            || cur.is_empty()
            || f[2] == cur
            || f[2] == "N"
            || f[5] == switcher_window_name
        {
            continue;
        }
        let _ = ipc.call(&move_node_args(&f[0], &cur));
        moved.push(f[0].clone());
    }

    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    let screen_str = screen
        .map(|s| s.to_string())
        .unwrap_or_else(|| "?".to_string());
    let moved_str = if moved.is_empty() {
        String::new()
    } else {
        format!(", moved {}", moved.join(","))
    };

    HotkeyPrep {
        log: format!("prep {ms:.1} ms (ws {cur}, screen {screen_str}{moved_str})"),
        workspace: cur,
        screen,
        cache_cleared: cleared,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    struct RecordingIpc {
        calls: Mutex<Vec<Vec<String>>>,
        responses: HashMap<String, String>,
    }

    impl RecordingIpc {
        fn new() -> Self {
            RecordingIpc {
                calls: Mutex::new(Vec::new()),
                responses: HashMap::new(),
            }
        }
        fn with(mut self, args: &[&str], out: &str) -> Self {
            self.responses.insert(args.join(" "), out.to_string());
            self
        }
        fn calls(&self) -> Vec<Vec<String>> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl AeroCall for RecordingIpc {
        fn call(&self, args: &[String]) -> Option<String> {
            self.calls.lock().unwrap().push(args.to_vec());
            self.responses.get(&args.join(" ")).cloned()
        }
    }

    #[test]
    fn hotkey_modes_content() {
        assert_eq!(
            HOTKEY_MODES,
            &[
                "window",
                "notes",
                "voice",
                "jira",
                "files",
                "confluence",
                "ai",
                "compare"
            ]
        );
        assert!(is_hotkey_mode("window"));
        assert!(is_hotkey_mode("compare"));
        assert!(!is_hotkey_mode("terminal"));
        assert!(!is_hotkey_mode("screenshot"));
    }

    #[test]
    fn aero_arguments_match_swift() {
        assert_eq!(
            focused_args(),
            vec!["list-windows", "--focused", "--format", "%{window-id} %{app-pid}"]
        );
        assert_eq!(
            rows_args(),
            vec![
                "list-windows",
                "--all",
                "--format",
                "%{window-id}|%{app-pid}|%{workspace}|%{workspace-is-focused}|%{monitor-appkit-nsscreen-screens-id}|%{window-title}"
            ]
        );
        assert_eq!(
            workspaces_args(),
            vec![
                "list-workspaces",
                "--focused",
                "--format",
                "%{workspace}|%{monitor-appkit-nsscreen-screens-id}"
            ]
        );
        assert_eq!(eval_true_args(), vec!["eval", "true"]);
        assert_eq!(
            move_node_args("42", "3"),
            vec!["move-node-to-workspace", "--window-id", "42", "3"]
        );
    }

    #[test]
    fn parse_table_keeps_empty_fields_and_six_columns() {
        let rows = "1|100|2|true|0|Title with | pipe\n2|100|3|false|1|\nshort|row\n";
        let t = parse_windows_table(rows);
        assert_eq!(t.len(), 2);
        assert_eq!(t[0][5], "Title with | pipe");
        assert_eq!(t[1][5], "");
        assert_eq!(t[0][3], "true");
    }

    #[test]
    fn parse_workspace_field_splits_and_parses() {
        assert_eq!(parse_workspace_field("3|0"), ("3".to_string(), Some(0)));
        assert_eq!(parse_workspace_field("3"), ("3".to_string(), None));
        assert_eq!(parse_workspace_field("3|"), ("3".to_string(), None));
        assert_eq!(parse_workspace_field(""), ("".to_string(), None));
    }

    #[test]
    fn prep_writes_focus_file_and_moves_our_other_windows() {
        let ipc = RecordingIpc::new()
            .with(
                &["list-windows", "--focused", "--format", "%{window-id} %{app-pid}"],
                "42 555\n",
            )
            .with(&["eval", "true"], "")
            .with(
                &[
                    "list-windows",
                    "--all",
                    "--format",
                    "%{window-id}|%{app-pid}|%{workspace}|%{workspace-is-focused}|%{monitor-appkit-nsscreen-screens-id}|%{window-title}",
                ],
                "42|555|2|true|0|main\n7|555|3|false|1|other\n8|999|3|false|1|foreign\n9|555|N|false|1|scratch\n",
            );

        let dir = std::env::temp_dir().join(format!("ws-rs-hotkey-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let focus = dir.join("kitchen-sink-focus");
        let focus_s = focus.to_str().unwrap();

        let prep = hotkey_prep_with_pid(&ipc, focus_s, "kitchen-sink", 555);

        assert_eq!(prep.workspace, "2");
        assert_eq!(prep.screen, Some(0));
        assert!(prep.cache_cleared);
        assert_eq!(std::fs::read_to_string(focus_s).unwrap(), "42 555");

        let calls = ipc.calls();
        assert!(calls.contains(&move_node_args("7", "2")));
        assert!(
            !calls.iter().any(|c| c.get(0).map(String::as_str) == Some("move-node-to-workspace")
                && c.get(2).map(String::as_str) == Some("8")),
            "a foreign pid must not move"
        );
        assert!(
            !calls.iter().any(|c| c.get(2).map(String::as_str) == Some("9")),
            "workspace N must not move"
        );
        assert!(
            !calls.iter().any(|c| c.get(2).map(String::as_str) == Some("42")),
            "the already-focused window must not move"
        );
        assert!(prep.log.starts_with("prep "));
        assert!(prep.log.contains("moved 7"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prep_removes_focus_file_when_nothing_focused() {
        let ipc = RecordingIpc::new()
            .with(
                &["list-windows", "--focused", "--format", "%{window-id} %{app-pid}"],
                "",
            )
            .with(
                &[
                    "list-windows",
                    "--all",
                    "--format",
                    "%{window-id}|%{app-pid}|%{workspace}|%{workspace-is-focused}|%{monitor-appkit-nsscreen-screens-id}|%{window-title}",
                ],
                "",
            )
            .with(
                &[
                    "list-workspaces",
                    "--focused",
                    "--format",
                    "%{workspace}|%{monitor-appkit-nsscreen-screens-id}",
                ],
                "1|0\n",
            );

        let dir = std::env::temp_dir().join(format!("ws-rs-hotkey2-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let focus = dir.join("kitchen-sink-focus");
        std::fs::write(&focus, "stale").unwrap();
        let focus_s = focus.to_str().unwrap();

        let prep = hotkey_prep_with_pid(&ipc, focus_s, "kitchen-sink", 555);

        assert!(!focus.exists(), "a stale focus file must be removed");
        assert_eq!(prep.workspace, "1");
        assert_eq!(prep.screen, Some(0));
        assert!(!prep.cache_cleared);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
