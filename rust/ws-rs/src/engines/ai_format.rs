//! Port of `AIFormat.swift` — facade over `pylib/ai_format.py`.
//!
//! Everything here forwards to the `ai.*` helper methods. The AppKit-only
//! pieces of `RichText` (`rtf` / `copy`) are implemented on macOS behind
//! `#[cfg(target_os = "macos")]` and fall back to no-ops elsewhere.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasteTarget {
    Outlook,
    Webex,
}

impl PasteTarget {
    pub fn title(self) -> &'static str {
        match self {
            PasteTarget::Outlook => "Outlook",
            PasteTarget::Webex => "Webex",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            PasteTarget::Outlook => "outlook",
            PasteTarget::Webex => "webex",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneMode {
    Diff,
    Markdown,
    Outlook,
    Webex,
}

impl PaneMode {
    pub fn as_str(self) -> &'static str {
        match self {
            PaneMode::Diff => "diff",
            PaneMode::Markdown => "markdown",
            PaneMode::Outlook => "outlook",
            PaneMode::Webex => "webex",
        }
    }

    pub fn title(self) -> String {
        let raw = self.as_str();
        let mut chars = raw.chars();
        match chars.next() {
            Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
            None => String::new(),
        }
    }

    pub fn target(self) -> Option<PasteTarget> {
        match self {
            PaneMode::Outlook => Some(PasteTarget::Outlook),
            PaneMode::Webex => Some(PasteTarget::Webex),
            _ => None,
        }
    }

    pub fn all(diff: bool) -> Vec<PaneMode> {
        let mut out = Vec::new();
        if diff {
            out.push(PaneMode::Diff);
        }
        out.push(PaneMode::Markdown);
        out.push(PaneMode::Outlook);
        out.push(PaneMode::Webex);
        out
    }
}

fn ai_call(method: &str, params: Value, timeout_secs: u64) -> Option<Value> {
    crate::app::python_helper::PythonHelper::shared()
        .call(
            method,
            params,
            Duration::from_secs(timeout_secs),
            Duration::from_secs(5),
        )
        .ok()
}

fn ai_box(method: &str, params: Value, timeout_secs: u64) -> Option<Value> {
    ai_call(method, params, timeout_secs).filter(Value::is_object)
}

fn ai_string(method: &str, params: Value, key: &str, timeout_secs: u64) -> Option<String> {
    ai_box(method, params, timeout_secs)?
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn read_commands_text() -> Option<String> {
    let path = std::env::var("WS_COMMANDS_CONF")
        .ok()
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".config/kitchen-sink/commands.toml"))
        })?;
    std::fs::read_to_string(path).ok()
}

// `aiSetting` / `aiNumber` from AIWindow.swift: the first `[ai]` entry with
// this key, read through the codec's `config.section_entries`.
pub fn ai_section_value(key: &str) -> Option<String> {
    let text = read_commands_text()?;
    let params = json!({"text": text, "section": "ai"});
    ai_box("config.section_entries", params, 10)?
        .get("entries")?
        .as_array()?
        .iter()
        .find_map(|e| {
            if e.get("key").and_then(Value::as_str) == Some(key) {
                e.get("value").and_then(Value::as_str).map(str::to_string)
            } else {
                None
            }
        })
}

pub fn ai_setting(key: &str, fallback: &str) -> String {
    let value = ai_section_value(key).map(|s| s.trim().to_string()).unwrap_or_default();
    if value.is_empty() {
        fallback.to_string()
    } else {
        value
    }
}

pub fn ai_number(key: &str, fallback: f64) -> f64 {
    ai_section_value(key)
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(fallback)
}

fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{home}/{rest}");
        }
    }
    path.to_string()
}

fn is_executable(path: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && (m.permissions().mode() & 0o111) != 0)
        .unwrap_or(false)
}

pub struct RichText;

impl RichText {
    pub fn pandoc_bin() -> String {
        expand_tilde(&ai_setting("pandoc-bin", "/opt/homebrew/bin/pandoc"))
    }

    pub fn available() -> bool {
        is_executable(&Self::pandoc_bin())
    }

    pub fn tables_as_text(md: &str) -> String {
        ai_string("ai.tables_as_text", json!({"md": md}), "text", 60)
            .unwrap_or_else(|| md.to_string())
    }

    pub fn markdown(md: &str, target: PasteTarget) -> String {
        ai_string(
            "ai.markdown",
            json!({"md": md, "target": target.key()}),
            "text",
            60,
        )
        .unwrap_or_else(|| md.to_string())
    }

    pub fn pandoc_html(md: &str, highlight: bool) -> Option<String> {
        if !Self::available() {
            return None;
        }
        let params = json!({"md": md, "highlight": highlight, "pandoc": Self::pandoc_bin()});
        ai_string("ai.pandoc_html", params, "html", 60)
    }

    pub fn styled(fragment: &str, target: PasteTarget) -> String {
        ai_string(
            "ai.styled",
            json!({"fragment": fragment, "target": target.key()}),
            "html",
            60,
        )
        .unwrap_or_else(|| fragment.to_string())
    }

    pub fn html(md: &str, target: PasteTarget) -> Option<String> {
        if !Self::available() {
            return None;
        }
        let params = json!({"md": md, "target": target.key(), "pandoc": Self::pandoc_bin()});
        ai_string("ai.html", params, "html", 60)
    }

    pub fn document(fragment: &str) -> String {
        format!("<html><head><meta charset=\"utf-8\"></head><body>{fragment}</body></html>")
    }

    /// Mirrors Swift `RichText.rtf(_:)`: parse [`Self::document`] as an HTML
    /// `NSAttributedString`, then serialize it to RTF data. `None` on any
    /// failure (including off-macOS).
    pub fn rtf(fragment: &str) -> Option<Vec<u8>> {
        #[cfg(target_os = "macos")]
        {
            Self::rtf_appkit(fragment)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = fragment;
            None
        }
    }

    #[cfg(target_os = "macos")]
    fn rtf_appkit(fragment: &str) -> Option<Vec<u8>> {
        use objc2::runtime::AnyObject;
        use objc2::AnyThread;
        use objc2_app_kit::{
            NSAttributedStringDocumentFormats, NSCharacterEncodingDocumentOption,
            NSDocumentTypeDocumentAttribute, NSDocumentTypeDocumentOption, NSHTMLTextDocumentType,
            NSRTFTextDocumentType, NSAttributedStringDocumentAttributeKey,
            NSAttributedStringDocumentReadingOptionKey,
        };
        use objc2_foundation::{
            NSAttributedString, NSData, NSDictionary, NSNumber, NSRange, NSString,
        };

        let doc = Self::document(fragment);
        let data = NSData::with_bytes(doc.as_bytes());

        // Reading options: `[.documentType: .html, .characterEncoding: UTF-8]`
        // (`NSUTF8StringEncoding == 4`).
        let html_type: &AnyObject =
            unsafe { &*(NSHTMLTextDocumentType as *const NSString as *const AnyObject) };
        let encoding: objc2::rc::Retained<AnyObject> = NSNumber::new_u64(4).into();
        let read_keys: [&NSAttributedStringDocumentReadingOptionKey; 2] =
            unsafe { [NSDocumentTypeDocumentOption, NSCharacterEncodingDocumentOption] };
        let read_objs: [&AnyObject; 2] = [html_type, &encoding];
        let read_options: objc2::rc::Retained<
            NSDictionary<NSAttributedStringDocumentReadingOptionKey, AnyObject>,
        > = NSDictionary::from_slices(&read_keys, &read_objs);

        let mut attrs: Option<
            objc2::rc::Retained<NSDictionary<NSAttributedStringDocumentAttributeKey, AnyObject>>,
        > = None;
        let attr = unsafe {
            NSAttributedString::initWithData_options_documentAttributes_error(
                NSAttributedString::alloc(),
                &data,
                &read_options,
                Some(&mut attrs),
            )
        }
        .ok()?;

        // Write options: `[.documentType: .rtf]`.
        let rtf_type: &AnyObject =
            unsafe { &*(NSRTFTextDocumentType as *const NSString as *const AnyObject) };
        let write_keys: [&NSAttributedStringDocumentAttributeKey; 1] =
            unsafe { [NSDocumentTypeDocumentAttribute] };
        let write_objs: [&AnyObject; 1] = [rtf_type];
        let write_options: objc2::rc::Retained<
            NSDictionary<NSAttributedStringDocumentAttributeKey, AnyObject>,
        > = NSDictionary::from_slices(&write_keys, &write_objs);

        let range = NSRange::new(0, attr.length());
        let out =
            unsafe { attr.dataFromRange_documentAttributes_error(range, &write_options) }.ok()?;
        Some(out.to_vec())
    }

    /// Mirrors Swift `RichText.copy(markdown:fragment:for:)`: one pasteboard
    /// item carrying the HTML fragment, its RTF rendering, and the target's
    /// Markdown as plain text. A no-op off macOS.
    pub fn copy(markdown: &str, fragment: Option<&str>, target: PasteTarget) {
        #[cfg(target_os = "macos")]
        {
            Self::copy_appkit(markdown, fragment, target);
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (markdown, fragment, target);
        }
    }

    #[cfg(target_os = "macos")]
    fn copy_appkit(markdown: &str, fragment: Option<&str>, target: PasteTarget) {
        use objc2::rc::Retained;
        use objc2::runtime::ProtocolObject;
        use objc2_app_kit::{
            NSPasteboard, NSPasteboardItem, NSPasteboardTypeHTML, NSPasteboardTypeRTF,
            NSPasteboardTypeString, NSPasteboardWriting,
        };
        use objc2_foundation::{NSArray, NSData, NSString};

        let pb = NSPasteboard::generalPasteboard();
        pb.clearContents();
        let item = NSPasteboardItem::new();
        if let Some(f) = fragment {
            let html = NSString::from_str(&Self::document(f));
            let _ = item.setString_forType(&html, unsafe { NSPasteboardTypeHTML });
            if let Some(rtf) = Self::rtf(f) {
                let _ = item.setData_forType(
                    &NSData::with_bytes(&rtf),
                    unsafe { NSPasteboardTypeRTF },
                );
            }
        }
        let text = NSString::from_str(&Self::markdown(markdown, target));
        let _ = item.setString_forType(&text, unsafe { NSPasteboardTypeString });

        let writable: Retained<ProtocolObject<dyn NSPasteboardWriting>> =
            ProtocolObject::from_retained(item);
        let objects = NSArray::from_retained_slice(&[writable]);
        pb.writeObjects(&objects);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeGuard {
    pub text: String,
    pub codes: Vec<String>,
}

impl CodeGuard {
    pub fn new(s: &str) -> Self {
        match ai_box("ai.code_guard", json!({"text": s}), 60) {
            Some(boxed) => {
                let text = boxed
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| s.to_string());
                let codes = boxed
                    .get("codes")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                CodeGuard { text, codes }
            }
            None => CodeGuard {
                text: s.to_string(),
                codes: Vec::new(),
            },
        }
    }

    pub fn token(n: i64) -> String {
        format!("[[CODE{n}]]")
    }

    pub fn restore(&self, s: &str) -> (String, i64) {
        let params = json!({"codes": self.codes, "s": s});
        match ai_box("ai.code_restore", params, 60) {
            Some(boxed) => (
                boxed
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| s.to_string()),
                boxed.get("missing").and_then(Value::as_i64).unwrap_or(0),
            ),
            None => (s.to_string(), 0),
        }
    }
}

pub struct TokenBudget;

impl TokenBudget {
    pub fn context() -> i64 {
        std::cmp::max(512, ai_number("context-tokens", 4096.0) as i64)
    }

    pub fn estimate(s: &str) -> i64 {
        if let Some(n) = ai_box("ai.estimate", json!({"s": s}), 60)
            .and_then(|b| b.get("tokens").and_then(Value::as_i64))
        {
            return n;
        }
        (s.as_bytes().len() as f64 / 3.2).ceil() as i64
    }

    pub fn part_budget(instructions: &str) -> i64 {
        let params = json!({"instructions": instructions, "context": Self::context()});
        ai_box("ai.part_budget", params, 60)
            .and_then(|b| b.get("budget").and_then(Value::as_i64))
            .unwrap_or(200)
    }

    pub fn parts(s: &str, budget: i64) -> Vec<String> {
        let params = json!({"s": s, "budget": budget});
        ai_box("ai.parts", params, 60)
            .and_then(|b| {
                b.get("parts").and_then(Value::as_array).map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
            })
            .unwrap_or_else(|| vec![s.to_string()])
    }
}

pub struct Reflow;

impl Reflow {
    pub fn instructions(s: &str) -> String {
        ai_string("ai.reflow", json!({"s": s}), "text", 60).unwrap_or_else(|| s.to_string())
    }
}

pub struct AnswerCleanup;

impl AnswerCleanup {
    pub fn unwrap_fence(s: &str) -> String {
        ai_string("ai.unwrap_fence", json!({"s": s}), "text", 60).unwrap_or_else(|| s.to_string())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AIRule {
    pub path: String,
    pub name: String,
    pub output: String,
    pub placeholder: String,
    pub flags: Vec<String>,
    pub instructions: String,
    pub warnings: Vec<String>,
    pub protect_code_set: Option<bool>,
    pub chunk_set: Option<bool>,
    pub prompt: String,
    pub then: String,
    pub keep_words: bool,
    pub csv_tables: bool,
}

fn str_vec(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

impl AIRule {
    pub fn new(path: &str, name: &str) -> Self {
        AIRule {
            path: path.to_string(),
            name: name.to_string(),
            output: "plain".to_string(),
            placeholder: String::new(),
            flags: Vec::new(),
            instructions: String::new(),
            warnings: Vec::new(),
            protect_code_set: None,
            chunk_set: None,
            prompt: String::new(),
            then: String::new(),
            keep_words: false,
            csv_tables: false,
        }
    }

    pub fn file(&self) -> String {
        Path::new(&self.path)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    pub fn diff(&self) -> bool {
        self.output == "diff"
    }

    pub fn protect_code(&self) -> bool {
        self.protect_code_set.unwrap_or(true)
    }

    pub fn chunk(&self) -> bool {
        self.chunk_set.unwrap_or_else(|| self.diff())
    }

    pub fn to_json(&self) -> Value {
        json!({
            "path": self.path,
            "name": self.name,
            "output": self.output,
            "placeholder": self.placeholder,
            "flags": self.flags,
            "instructions": self.instructions,
            "warnings": self.warnings,
            "protectCodeSet": self.protect_code_set,
            "chunkSet": self.chunk_set,
            "prompt": self.prompt,
            "then": self.then,
            "keepWords": self.keep_words,
            "csvTables": self.csv_tables,
        })
    }

    pub fn from_json(json: &Value) -> Self {
        AIRule {
            path: json.get("path").and_then(Value::as_str).unwrap_or("").to_string(),
            name: json.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
            output: json
                .get("output")
                .and_then(Value::as_str)
                .unwrap_or("plain")
                .to_string(),
            placeholder: json
                .get("placeholder")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            flags: str_vec(json.get("flags")),
            instructions: json
                .get("instructions")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            warnings: str_vec(json.get("warnings")),
            protect_code_set: json.get("protectCodeSet").and_then(Value::as_bool),
            chunk_set: json.get("chunkSet").and_then(Value::as_bool),
            prompt: json.get("prompt").and_then(Value::as_str).unwrap_or("").to_string(),
            then: json.get("then").and_then(Value::as_str).unwrap_or("").to_string(),
            keep_words: json.get("keepWords").and_then(Value::as_bool).unwrap_or(false),
            csv_tables: json.get("csvTables").and_then(Value::as_bool).unwrap_or(false),
        }
    }

    pub fn load(path: &str) -> AIRule {
        let params = json!({"path": path});
        match ai_box("ai.rule_load", params, 60).and_then(|b| b.get("rule").cloned()) {
            Some(rule) => AIRule::from_json(&rule),
            None => {
                let name = Path::new(path)
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                AIRule::new(path, &name)
            }
        }
    }

    pub fn chain(first: &AIRule) -> Vec<AIRule> {
        let params = json!({"path": first.path});
        match ai_box("ai.rule_chain", params, 60)
            .and_then(|b| b.get("rules").and_then(Value::as_array).map(|a| a.to_vec()))
        {
            Some(rules) => rules.iter().map(AIRule::from_json).collect(),
            None => vec![first.clone()],
        }
    }

    // Swift exposes a method `instructions(guarded:)` beside the stored
    // property `instructions`; Rust can't share the name, hence `_for`.
    pub fn instructions_for(&self, guarded: bool) -> String {
        let params = json!({"rule": self.to_json(), "guarded": guarded});
        ai_string("ai.rule_instructions", params, "text", 60)
            .unwrap_or_else(|| self.instructions.clone())
    }

    pub fn wrap(&self, text: &str) -> String {
        let params = json!({"rule": self.to_json(), "text": text});
        ai_string("ai.rule_wrap", params, "text", 60).unwrap_or_else(|| text.to_string())
    }

    pub fn prepare(&self, text: &str) -> String {
        let params = json!({"rule": self.to_json(), "text": text});
        ai_string("ai.rule_prepare", params, "text", 60).unwrap_or_else(|| text.to_string())
    }

    pub fn accept(&self, input: &str, answer: &str) -> (String, Option<String>) {
        let params = json!({"rule": self.to_json(), "input": input, "answer": answer});
        match ai_box("ai.rule_accept", params, 60) {
            Some(boxed) => (
                boxed
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| answer.to_string()),
                boxed.get("note").and_then(Value::as_str).map(str::to_string),
            ),
            None => (answer.to_string(), None),
        }
    }

    pub fn arguments(&self, guarded: bool) -> Vec<String> {
        let params = json!({"rule": self.to_json(), "guarded": guarded});
        ai_box("ai.rule_arguments", params, 60)
            .and_then(|b| {
                b.get("args").and_then(Value::as_array).map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
            })
            .unwrap_or_default()
    }

    pub fn preview(&self) -> String {
        let params = json!({"rule": self.to_json()});
        ai_string("ai.rule_preview", params, "text", 60).unwrap_or_default()
    }

    pub fn runnable(&self, input: &str) -> String {
        let params = json!({"rule": self.to_json(), "input": input});
        ai_string("ai.rule_runnable", params, "text", 60).unwrap_or_default()
    }
}

pub struct CSVTables;

impl CSVTables {
    pub fn cells(line: &str) -> Option<Vec<String>> {
        ai_box("ai.csv_cells", json!({"line": line}), 60)?
            .get("cells")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
    }

    pub fn convert(s: &str) -> String {
        ai_string("ai.csv_convert", json!({"s": s}), "text", 60).unwrap_or_else(|| s.to_string())
    }
}

pub struct WordGuard;

impl WordGuard {
    pub fn words(s: &str) -> Vec<String> {
        ai_box("ai.words", json!({"s": s}), 60)
            .and_then(|b| {
                b.get("words").and_then(Value::as_array).map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
            })
            .unwrap_or_default()
    }

    pub fn check(input: &str, output: &str) -> (bool, Vec<String>, Vec<String>) {
        let params = json!({"input": input, "output": output});
        match ai_box("ai.word_check", params, 60) {
            Some(boxed) => (
                boxed.get("ok").and_then(Value::as_bool).unwrap_or(true),
                str_vec(boxed.get("added")),
                str_vec(boxed.get("dropped")),
            ),
            None => (true, Vec::new(), Vec::new()),
        }
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
    fn pane_mode_shape() {
        assert_eq!(PaneMode::Diff.as_str(), "diff");
        assert_eq!(PaneMode::Markdown.title(), "Markdown");
        assert_eq!(PaneMode::Outlook.target(), Some(PasteTarget::Outlook));
        assert_eq!(PaneMode::Markdown.target(), None);
        let all = PaneMode::all(true);
        assert_eq!(all.len(), 4);
        assert_eq!(all[0], PaneMode::Diff);
        assert_eq!(PaneMode::all(false), vec![PaneMode::Markdown, PaneMode::Outlook, PaneMode::Webex]);
        assert_eq!(PasteTarget::Webex.key(), "webex");
    }

    #[test]
    fn code_guard_token_and_document() {
        assert_eq!(CodeGuard::token(7), "[[CODE7]]");
        assert_eq!(
            RichText::document("<b>x</b>"),
            "<html><head><meta charset=\"utf-8\"></head><body><b>x</b></body></html>"
        );
    }

    #[test]
    fn airule_json_round_trip_and_defaults() {
        let rule = AIRule::from_json(&json!({
            "path": "/r/a.md", "name": "a", "output": "diff",
            "flags": ["--greedy"], "protectCodeSet": false, "keepWords": true
        }));
        assert_eq!(rule.file(), "a.md");
        assert!(rule.diff());
        assert!(!rule.protect_code());
        assert!(rule.chunk());
        assert!(rule.keep_words);
        assert_eq!(rule.to_json().get("name").unwrap(), "a");

        let plain = AIRule::new("/r/b.md", "b");
        assert!(plain.protect_code());
        assert!(!plain.chunk());
    }

    #[test]
    fn estimate_short_ascii() {
        // ceil(4 / 3.2) == 2 via the helper or its fallback.
        assert_eq!(TokenBudget::estimate("abcd"), 2);
    }

    #[test]
    fn code_guard_round_trip_when_helper_present() {
        if !helper_ready() {
            eprintln!("skipping: no python 3.11+ or pylib/helper not found");
            return;
        }
        let guarded = CodeGuard::new("hi `x` there");
        assert!(guarded.text.contains(&CodeGuard::token(1)));
        assert_eq!(guarded.codes, vec!["`x`".to_string()]);
        let (restored, missing) = guarded.restore(&guarded.text);
        assert_eq!(restored, "hi `x` there");
        assert_eq!(missing, 0);

        let (ok, added, dropped) = WordGuard::check("the cat sat", "the cat sat");
        assert!(ok);
        assert!(added.is_empty() && dropped.is_empty());
    }

    /// `RichText::rtf` builds an RTF document from the HTML wrapper. The
    /// AppKit text system is main-thread-bound, so skip (rather than hazard an
    /// off-thread call) when libtest's worker thread is not the main thread.
    #[cfg(target_os = "macos")]
    #[test]
    fn rtf_produces_rtf_document() {
        if objc2::MainThreadMarker::new().is_none() {
            eprintln!("skipping: not on the main thread");
            return;
        }
        match RichText::rtf("<b>hi</b>") {
            Some(data) => assert!(
                data.starts_with(b"{\\rtf"),
                "expected an RTF header, got {:?}",
                &data[..data.len().min(16)]
            ),
            None => eprintln!("skipping: NSAttributedString HTML->RTF unavailable"),
        }
    }
}
