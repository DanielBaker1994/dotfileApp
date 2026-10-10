//! Port of `FileOps.swift` — facade over `pylib/file_ops.py`.
//!
//! The ops + undo stack live in python; the handle crosses the helper as an
//! integer. Callers pass an [`UndoStack`]; [`FileOps::shared`] is the browser's
//! global stack (compare sessions own their own).

use serde_json::{json, Value};
use std::sync::OnceLock;
use std::time::Duration;

pub type Change = (Option<String>, String);

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub changes: Vec<Change>,
    pub failed: Option<String>,
}

impl Outcome {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn paths(&self) -> Vec<String> {
        self.changes.iter().map(|c| c.1.clone()).collect()
    }

    pub fn from_json(json: &Value) -> Self {
        let changes = json
            .get("changes")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|c| {
                        let pair = c.as_array()?;
                        if pair.len() != 2 {
                            return None;
                        }
                        let to = pair[1].as_str()?.to_string();
                        let from = pair[0].as_str().map(str::to_string);
                        Some((from, to))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let failed = json
            .get("failed")
            .and_then(Value::as_str)
            .map(str::to_string);
        Outcome { changes, failed }
    }
}

fn fo_call(method: &str, params: Value) -> Option<Value> {
    crate::app::python_helper::PythonHelper::shared()
        .call(
            method,
            params,
            Duration::from_secs(300),
            Duration::from_secs(5),
        )
        .ok()
}

#[derive(Debug)]
pub struct UndoStack {
    handle: i64,
    limit: i64,
}

impl UndoStack {
    pub fn new(limit: i64) -> Self {
        let handle = fo_call("fileops.stack_new", json!({"limit": limit}))
            .and_then(|b| b.get("handle").and_then(Value::as_i64))
            .unwrap_or(-1);
        UndoStack { handle, limit }
    }

    pub fn handle(&self) -> i64 {
        self.handle
    }

    pub fn limit(&self) -> i64 {
        self.limit
    }

    pub fn can_undo(&self) -> bool {
        self.handle >= 0
            && fo_call("fileops.stack_state", json!({"handle": self.handle}))
                .and_then(|b| b.get("canUndo").and_then(Value::as_bool))
                .unwrap_or(false)
    }

    pub fn count(&self) -> i64 {
        if self.handle < 0 {
            return 0;
        }
        fo_call("fileops.stack_state", json!({"handle": self.handle}))
            .and_then(|b| b.get("count").and_then(Value::as_i64))
            .unwrap_or(0)
    }

    pub fn forget(&self) {
        if self.handle >= 0 {
            let _ = fo_call("fileops.stack_forget", json!({"handle": self.handle}));
        }
    }

    pub fn collapse(&self, since: i64, what: &str) {
        if self.handle >= 0 {
            let _ = fo_call(
                "fileops.stack_collapse",
                json!({"handle": self.handle, "since": since, "what": what}),
            );
        }
    }
}

impl Default for UndoStack {
    fn default() -> Self {
        Self::new(50)
    }
}

impl Drop for UndoStack {
    fn drop(&mut self) {
        if self.handle >= 0 {
            let _ = fo_call("fileops.stack_drop", json!({"handle": self.handle}));
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clash {
    Replace,
    KeepBoth,
    Skip,
}

impl Clash {
    pub fn as_str(self) -> &'static str {
        match self {
            Clash::Replace => "replace",
            Clash::KeepBoth => "keepBoth",
            Clash::Skip => "skip",
        }
    }
}

pub struct FileOps;

impl FileOps {
    pub fn shared() -> &'static UndoStack {
        static SHARED: OnceLock<UndoStack> = OnceLock::new();
        SHARED.get_or_init(|| UndoStack::new(50))
    }

    pub fn can_undo() -> bool {
        Self::shared().can_undo()
    }

    pub fn forget_undo() {
        Self::shared().forget();
    }

    pub fn transfer(paths: &[String], into: &str, move_: bool, undo: &UndoStack) -> Outcome {
        let params = json!({
            "paths": paths, "into": into, "move": move_, "stack": undo.handle()
        });
        outcome(fo_call("fileops.transfer", params))
    }

    pub fn duplicate(paths: &[String], undo: &UndoStack) -> Outcome {
        let params = json!({"paths": paths, "stack": undo.handle()});
        outcome(fo_call("fileops.duplicate", params))
    }

    pub fn trash(paths: &[String], undo: &UndoStack) -> Outcome {
        let params = json!({"paths": paths, "stack": undo.handle()});
        outcome(fo_call("fileops.trash", params))
    }

    pub fn create(name: &str, in_dir: &str, folder: bool, undo: &UndoStack) -> Outcome {
        let params = json!({
            "name": name, "dir": in_dir, "folder": folder, "stack": undo.handle()
        });
        outcome(fo_call("fileops.create", params))
    }

    pub fn record_rename(from: &str, to: &str, undo: &UndoStack) {
        let _ = fo_call(
            "fileops.record_rename",
            json!({"from": from, "to": to, "stack": undo.handle()}),
        );
    }

    pub fn place(
        items: &[(String, String)],
        move_: bool,
        clash: Clash,
        undo: &UndoStack,
    ) -> Outcome {
        let items: Vec<Value> = items.iter().map(|(src, dst)| json!([src, dst])).collect();
        let params = json!({
            "items": items, "move": move_, "clash": clash.as_str(), "stack": undo.handle()
        });
        outcome(fo_call("fileops.place", params))
    }

    pub fn undo(undo: &UndoStack) -> Option<(String, Outcome)> {
        let boxed = fo_call("fileops.undo", json!({"stack": undo.handle()}))?;
        let outcome_json = boxed.get("outcome")?;
        let what = boxed
            .get("what")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        Some((what, Outcome::from_json(outcome_json)))
    }
}

fn outcome(boxed: Option<Value>) -> Outcome {
    match boxed {
        Some(v) => Outcome::from_json(&v),
        None => Outcome::new(),
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
    fn parses_outcome() {
        let raw = json!({
            "changes": [["/a", "/b"], [null, "/c"]],
            "failed": "x: nope"
        });
        let o = Outcome::from_json(&raw);
        assert_eq!(
            o.changes,
            vec![
                (Some("/a".to_string()), "/b".to_string()),
                (None, "/c".to_string())
            ]
        );
        assert_eq!(o.paths(), vec!["/b".to_string(), "/c".to_string()]);
        assert_eq!(o.failed.as_deref(), Some("x: nope"));
    }

    #[test]
    fn ignores_malformed_changes() {
        let raw = json!({"changes": [["only-one"], ["a", "b", "c"]], "failed": null});
        let o = Outcome::from_json(&raw);
        assert!(o.changes.is_empty());
        assert!(o.failed.is_none());
    }

    #[test]
    fn clash_raw_values() {
        assert_eq!(Clash::Replace.as_str(), "replace");
        assert_eq!(Clash::KeepBoth.as_str(), "keepBoth");
        assert_eq!(Clash::Skip.as_str(), "skip");
    }

    #[test]
    fn stack_round_trip_when_helper_present() {
        if !helper_ready() {
            eprintln!("skipping: no python 3.11+ or pylib/helper not found");
            return;
        }
        let stack = UndoStack::new(5);
        assert!(stack.handle() >= 0);
        assert!(!stack.can_undo());
        assert_eq!(stack.count(), 0);
        stack.forget();
        stack.collapse(0, "noop");
        let _ = FileOps::duplicate(&[], &stack);
    }
}
