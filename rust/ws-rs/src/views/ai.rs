//! Port of `AIWindow.swift` — the AI rules view.
//!
//! The rules engine itself (`pylib/ai_format.py`) is already ported in
//! [`crate::engines::ai_format`]. This module builds the window MODEL around
//! it: the `[ai]` config, the rule tabs, the right-pane mode, the run /
//! streaming state, the keys and the copy model. The pasteboard write goes
//! through [`RichText::copy`] (HTML + RTF + plain text) and the streaming `fm`
//! child is spawned by [`AIWindowModel::spawn_part`]; the right-pane
//! `WKWebView` preview (`macos`) loads the ported `ai_format` HTML for the
//! Markdown / Outlook / Webex modes.

use std::cell::Cell;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::{json, Value};

use crate::app::registry::{PaletteCommand, RectI, Registry, SlotMember, SlotView};
use crate::engines::ai_format::{
    AnswerCleanup, AIRule, CodeGuard, PaneMode, PasteTarget, RichText, TokenBudget,
};

// macOS virtual key codes used by `AIWindow.handleKey`.
pub const KEY_ESC: u16 = 53;
pub const KEY_RETURN: u16 = 36;
pub const KEY_L: u16 = 37;
pub const KEY_SLASH: u16 = 44;
pub const KEY_KEYPAD_ENTER: u16 = 76;

// ---------------------------------------------------------------------------
// `[ai]` config (`aiSetting` / `aiNumber` / `aiEnabled`)
// ---------------------------------------------------------------------------

pub const DEFAULT_WIDTH: f64 = 900.0;
pub const DEFAULT_HEIGHT: f64 = 600.0;
pub const DEFAULT_SPLIT: f64 = 0.5;
pub const DEFAULT_SIDEBAR_WIDTH: f64 = 210.0;
pub const DEFAULT_CONTEXT_TOKENS: i64 = 4096;
pub const DEFAULT_COPY_TOAST: &str = "Copied to clipboard";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AIConfig {
    pub entries: HashMap<String, String>,
}

impl AIConfig {
    pub fn from_entries(entries: HashMap<String, String>) -> Self {
        AIConfig { entries }
    }

    pub fn string(&self, key: &str, fallback: &str) -> String {
        match self.entries.get(key) {
            Some(v) if !v.trim().is_empty() => v.trim().to_string(),
            _ => fallback.to_string(),
        }
    }

    pub fn number(&self, key: &str, fallback: f64) -> f64 {
        self.entries
            .get(key)
            .and_then(|v| v.trim().parse::<f64>().ok())
            .unwrap_or(fallback)
    }

    pub fn bool(&self, key: &str, fallback: bool) -> bool {
        self.entries
            .get(key)
            .and_then(|v| crate::views::confluence::tri_value(v))
            .unwrap_or(fallback)
    }

    /// `aiEnabled()` — `tri(...) == true`.
    pub fn enabled(&self) -> bool {
        self.entries
            .get("enabled")
            .and_then(|v| crate::views::confluence::tri_value(v))
            == Some(true)
    }

    pub fn in_palette(&self) -> bool {
        self.entries
            .get("in-palette")
            .and_then(|v| crate::views::confluence::tri_value(v))
            .unwrap_or(true)
    }

    pub fn label(&self) -> String {
        self.string("label", "AI View")
    }

    pub fn width(&self) -> f64 {
        self.number("width", DEFAULT_WIDTH)
    }

    pub fn height(&self) -> f64 {
        self.number("height", DEFAULT_HEIGHT)
    }

    pub fn split(&self) -> f64 {
        let f = self.number("split", DEFAULT_SPLIT);
        if !(0.2..=0.8).contains(&f) {
            DEFAULT_SPLIT
        } else {
            f
        }
    }

    pub fn sidebar_width(&self) -> f64 {
        self.number("sidebar-width", DEFAULT_SIDEBAR_WIDTH)
    }

    pub fn fm_bin(&self) -> String {
        expand_tilde(&self.string("fm-bin", "/usr/bin/fm"))
    }

    /// `aiPath("rules-dir", userDir + "/rules")`.
    pub fn rules_dir(&self) -> String {
        let fallback = format!("{}/rules", user_dir());
        expand_tilde(&self.string("rules-dir", &fallback))
    }

    pub fn context_tokens(&self) -> i64 {
        self.number("context-tokens", DEFAULT_CONTEXT_TOKENS as f64) as i64
    }

    pub fn font(&self) -> String {
        self.string("font", "")
    }

    /// `max(9, min(32, aiNumber("font-size", 14)))`.
    pub fn font_size(&self) -> f64 {
        self.number("font-size", 14.0).clamp(9.0, 32.0)
    }

    pub fn copy_toast(&self) -> String {
        self.string("copy-toast", DEFAULT_COPY_TOAST)
    }

    /// Parse the `[ai]` section out of a full `commands.toml` text (pure — the
    /// line codec, no python helper).
    pub fn from_config_text(text: &str) -> AIConfig {
        let lines = crate::engines::config_text::config_lines(text);
        let entries = crate::engines::config_text::config_section_entries(&lines, "ai");
        let mut map: HashMap<String, String> = HashMap::new();
        for (_, k, v) in entries {
            map.insert(k, v);
        }
        AIConfig::from_entries(map)
    }

    /// `[ai]` from `commands.toml` (falls back to the defaults when the config
    /// file is unreadable). Never touches the python helper.
    pub fn load() -> AIConfig {
        match crate::app::config::read_config_text() {
            Some(text) => AIConfig::from_config_text(&text),
            None => AIConfig::default(),
        }
    }
}

fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{home}/{rest}");
        }
    }
    path.to_string()
}

fn user_dir() -> String {
    crate::app::paths::Paths::from_env().user_dir().to_string()
}

/// `reloadRules()`'s directory listing: non-hidden `.md` files, ordered by
/// `localizedStandardCompare`.
pub fn rule_files(dir: &str) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || !name.to_lowercase().ends_with(".md") {
                None
            } else {
                Some(name)
            }
        })
        .collect();
    names.sort_by(|a, b| localized_standard_cmp(a, b));
    names
}

/// Build [`AIRule`]s from a directory listing alone — each rule's name is its
/// file stem. This deliberately skips `AIRule::load`, which would call the
/// python helper (`ai.rule_load`); the embeddable content must build with no
/// helper running.
pub fn rules_from_dir(dir: &str) -> Vec<AIRule> {
    rule_files(dir)
        .iter()
        .map(|n| {
            let stem = Path::new(n)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| n.clone());
            AIRule::new(&format!("{dir}/{n}"), &stem)
        })
        .collect()
}

/// `String.localizedStandardCompare` — Finder's case-insensitive natural sort
/// (digit runs compared as numbers).
pub fn localized_standard_cmp(a: &str, b: &str) -> Ordering {
    let ac: Vec<char> = a.chars().collect();
    let bc: Vec<char> = b.chars().collect();
    let mut i = 0;
    let mut j = 0;
    while i < ac.len() && j < bc.len() {
        let ca = ac[i];
        let cb = bc[j];
        if ca.is_ascii_digit() && cb.is_ascii_digit() {
            let mut ia = i;
            while ia < ac.len() && ac[ia].is_ascii_digit() {
                ia += 1;
            }
            let mut ib = j;
            while ib < bc.len() && bc[ib].is_ascii_digit() {
                ib += 1;
            }
            let na: String = ac[i..ia].iter().collect();
            let nb: String = bc[j..ib].iter().collect();
            let va = na.trim_start_matches('0');
            let vb = nb.trim_start_matches('0');
            match va.len().cmp(&vb.len()).then(va.cmp(vb)) {
                Ordering::Equal => {}
                other => return other,
            }
            i = ia;
            j = ib;
            continue;
        }
        let fa = ca.to_ascii_lowercase();
        let fb = cb.to_ascii_lowercase();
        match fa.cmp(&fb) {
            Ordering::Equal => {
                i += 1;
                j += 1;
            }
            other => return other,
        }
    }
    (ac.len() - i).cmp(&(bc.len() - j))
}

// ---------------------------------------------------------------------------
// Run / streaming state (`run()` → `runPart` → `finish`)
// ---------------------------------------------------------------------------

/// One step's outcome from [`AIRunState::finish_step`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StepOutcome {
    /// More parts / steps: the driver should run the process again.
    Continue,
    Done { missing: i64, warn: bool },
    Failed,
}

/// The per-run fields of `AIWindow` (`steps`, `parts`, `doneParts`,
/// `streamData`, `guardCode`, `answer`…). `finish_step` covers the decision
/// tree; the `fm` process spawn lives on [`AIWindowModel::spawn_part`] (the
/// state here is pure so it stays testable without a child).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AIRunState {
    pub steps: Vec<AIRule>,
    pub step_index: usize,
    pub step_input: String,
    pub parts: Vec<String>,
    pub part_index: usize,
    pub done_parts: Vec<String>,
    pub stream: String,
    pub step_notes: Vec<String>,
    pub guard_code: Option<CodeGuard>,
    pub answer: String,
    pub answer_input: String,
    pub answer_diff: bool,
    pub running: bool,
    pub run_gen: u64,
    pub started: f64,
}

impl AIRunState {
    /// `run()`'s setup + `startStep(0, …)`.
    pub fn start(&mut self, steps: Vec<AIRule>, input_text: &str, protect_code: bool) {
        if steps.is_empty() {
            return;
        }
        self.run_gen += 1;
        self.step_notes.clear();
        self.answer.clear();
        self.answer_input = input_text.to_string();
        self.answer_diff = steps[0].diff();
        self.guard_code = if protect_code {
            let g = CodeGuard::new(input_text);
            if g.codes.is_empty() {
                None
            } else {
                Some(g)
            }
        } else {
            None
        };
        self.running = true;
        self.started = now_secs();
        self.steps = steps;
        let guarded = self.guard_code.as_ref().map(|g| g.text.clone());
        let text = guarded.unwrap_or_else(|| input_text.to_string());
        self.start_step(0, &text);
    }

    /// `startStep(_:text:)` — prepare the input and split it into parts.
    pub fn start_step(&mut self, i: usize, text: &str) {
        if i >= self.steps.len() {
            return;
        }
        let rule = self.steps[i].clone();
        self.step_index = i;
        self.step_input = rule.prepare(text);
        let guarded = self.guard_code.is_some();
        let instructions = format!("{}{}", rule.instructions_for(guarded), rule.prompt);
        let budget = TokenBudget::part_budget(&instructions);
        self.parts = if rule.chunk() {
            TokenBudget::parts(&self.step_input, budget)
        } else {
            vec![self.step_input.clone()]
        };
        self.part_index = 0;
        self.done_parts.clear();
        self.stream.clear();
    }

    /// `outPipe` chunks → `chunk(gen:)`.
    pub fn push_chunk(&mut self, s: &str) {
        self.stream.push_str(s);
    }

    fn trimmed_stream(&self) -> String {
        self.stream.trim().to_string()
    }

    /// `stepSoFar()` — the joined parts, fence-unwrapped like the window.
    pub fn step_so_far(&self) -> String {
        let cur = self.trimmed_stream();
        let mut parts: Vec<String> = self.done_parts.clone();
        if !cur.is_empty() {
            parts.push(cur);
        }
        let joined = parts.join("\n\n");
        if self.guard_code.is_some() || !self.answer_input.starts_with("```") {
            AnswerCleanup::unwrap_fence(&joined)
        } else {
            joined
        }
    }

    /// `soFar()` — the code guard put back.
    pub fn so_far(&self) -> (String, i64) {
        let text = self.step_so_far();
        match &self.guard_code {
            Some(g) => g.restore(&text),
            None => (text, 0),
        }
    }

    /// `finish(gen:code:err:)` — advance parts / steps or finish the run.
    /// Returns [`StepOutcome::Continue`] when the driver should run again.
    pub fn finish_step(&mut self, code: i32, err: &str) -> StepOutcome {
        let rule = self.steps[self.step_index].clone();
        if code == 0 && self.part_index + 1 < self.parts.len() {
            self.done_parts.push(self.trimmed_stream());
            self.part_index += 1;
            self.stream.clear();
            return StepOutcome::Continue;
        }
        let mut code = code;
        let mut step_text = self.step_so_far();
        if code != 0 && self.step_index > 0 {
            // a later rule failed: keep the earlier step's answer, warn.
            let line = err
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .last()
                .map(str::to_string)
                .unwrap_or_else(|| format!("exit {code}"));
            self.step_notes
                .push(format!("\u{201c}{}\u{201d} failed ({})", rule.name, strip_ansi(&line)));
            step_text = self.step_input.clone();
            code = 0;
        } else if code == 0 {
            let (text, note) = rule.accept(&self.step_input, &step_text);
            step_text = text;
            if let Some(n) = note {
                self.step_notes.push(n);
            }
        }
        self.stream.clear();
        if code == 0 && self.step_index + 1 < self.steps.len() {
            let next = self.step_index + 1;
            self.start_step(next, &step_text);
            return StepOutcome::Continue;
        }
        let missing = match &self.guard_code {
            Some(g) => g.restore(&step_text).1,
            None => 0,
        };
        self.answer = match &self.guard_code {
            Some(g) => g.restore(&step_text).0,
            None => step_text,
        };
        self.running = false;
        if code != 0 {
            StepOutcome::Failed
        } else {
            StepOutcome::Done {
                missing,
                warn: !self.step_notes.is_empty(),
            }
        }
    }

    /// `cancelRun(quiet:)`.
    pub fn cancel(&mut self) {
        self.run_gen += 1;
        self.stream.clear();
        self.running = false;
    }
}

/// One event pushed by a `runPart` reader thread into the model's queue. The
/// reader threads never touch the model; they only append here and the UI/poll
/// side drains with [`AIWindowModel::poll_stream`]. Every event carries the
/// `run_gen` it belongs to so a cancelled / superseded run's tail is ignored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamEvent {
    /// A decoded stdout chunk (`outPipe` → `chunk(gen:)`).
    Chunk { gen: u64, data: String },
    /// A stderr chunk (accumulated for the failure message).
    Stderr { gen: u64, data: String },
    /// stdout hit EOF (all chunks for this run are already queued).
    StdoutEof { gen: u64 },
    /// stderr hit EOF.
    StderrEof { gen: u64 },
}

/// The shared queue type: the reader-thread boundary. `Child` stays with the
/// model so [`AIWindowModel::cancel_run`] can kill it; the threads only hold
/// this `Arc` and their own pipe handle.
pub type StreamQueue = Arc<Mutex<Vec<StreamEvent>>>;

fn push_event(q: &StreamQueue, ev: StreamEvent) {
    if let Ok(mut v) = q.lock() {
        v.push(ev);
    }
}

/// Decode the largest valid UTF-8 prefix of `pending`, leaving any trailing
/// incomplete sequence for the next read (an invalid byte is folded lossily).
fn drain_utf8(pending: &mut Vec<u8>) -> String {
    let mut cut = pending.len();
    if let Err(e) = std::str::from_utf8(pending) {
        if e.error_len().is_none() {
            cut = e.valid_up_to();
        }
    }
    let text = String::from_utf8_lossy(&pending[..cut]).into_owned();
    pending.drain(..cut);
    text
}

/// Read a pipe to EOF, queuing chunks (stdout) or stderr text, then an EOF
/// marker. Mirrors the `availableData` loops in `runPart` but off the main
/// thread — nothing here blocks the caller.
fn pump<R: Read>(mut pipe: R, gen: u64, q: &StreamQueue, is_stdout: bool) {
    let mut buf = [0u8; 8192];
    let mut pending: Vec<u8> = Vec::new();
    loop {
        match pipe.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if is_stdout {
                    pending.extend_from_slice(&buf[..n]);
                    let text = drain_utf8(&mut pending);
                    if !text.is_empty() {
                        push_event(q, StreamEvent::Chunk { gen, data: text });
                    }
                } else {
                    let text = String::from_utf8_lossy(&buf[..n]).into_owned();
                    if !text.is_empty() {
                        push_event(q, StreamEvent::Stderr { gen, data: text });
                    }
                }
            }
            Err(_) => break,
        }
    }
    if is_stdout {
        if !pending.is_empty() {
            let text = String::from_utf8_lossy(&pending).into_owned();
            if !text.is_empty() {
                push_event(q, StreamEvent::Chunk { gen, data: text });
            }
        }
        push_event(q, StreamEvent::StdoutEof { gen });
    } else {
        push_event(q, StreamEvent::StderrEof { gen });
    }
}

/// The last non-blank, ANSI-stripped stderr line (Swift's `err.split(...).last`).
fn last_error_line(err: &str, code: i32) -> String {
    err.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .last()
        .map(strip_ansi)
        .unwrap_or_else(|| format!("exit {code}"))
}

// ---------------------------------------------------------------------------
// Char diff (`CharDiff.changes(CharDiff.diff(_:_:))`)
//
// `AIWindow.finish` calls the Python `compare.char_diff` helper synchronously
// on the completion path (the `REVIEW-architecture` blocking-call debt). The
// streaming poll must not block, so the same token-LCS grouping is ported
// inline from `pylib/compare_text.py` (and `CompareText.swift`'s `CharDiff`).
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DiffOpKind {
    Same,
    Del,
    Ins,
}

/// `char_tokens` — word / whitespace / punctuation runs (an apostrophe after a
/// word letter stays with that word).
fn char_tokens(s: &str) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_kind = 0u8;
    for (i, &ch) in chars.iter().enumerate() {
        let is_word = ch.is_alphabetic()
            || ch.is_numeric()
            || ((ch == '\'' || ch == '\u{2019}')
                && cur_kind == 1
                && i + 1 < chars.len()
                && chars[i + 1].is_alphabetic());
        let k = if is_word {
            1u8
        } else if ch.is_whitespace() {
            2
        } else {
            3
        };
        if k == 3 {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            cur_kind = 0;
            out.push(ch.to_string());
            continue;
        }
        if k != cur_kind {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            cur_kind = k;
        }
        cur.push(ch);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// `char_lines`.
fn char_lines(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in s.chars() {
        cur.push(ch);
        if ch == '\n' {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// `_char_raw_ops` — common prefix/suffix trim then an LCS alignment.
fn char_raw_ops(a: &str, b: &str) -> Vec<(DiffOpKind, String)> {
    let mut x = char_tokens(a);
    let mut y = char_tokens(b);
    if x.len().saturating_mul(y.len()) > 6_000_000 {
        x = char_lines(a);
        y = char_lines(b);
    }
    let n = x.len();
    let m = y.len();
    let mut pre = 0;
    while pre < n && pre < m && x[pre] == y[pre] {
        pre += 1;
    }
    let mut suf = 0;
    while suf < n - pre && suf < m - pre && x[n - 1 - suf] == y[m - 1 - suf] {
        suf += 1;
    }
    let xs = &x[pre..n - suf];
    let ys = &y[pre..m - suf];
    let mut raw: Vec<(DiffOpKind, String)> = x[..pre]
        .iter()
        .map(|t| (DiffOpKind::Same, t.clone()))
        .collect();
    let r = xs.len();
    let c = ys.len();
    if r > 0 || c > 0 {
        let w = c + 1;
        let mut l = vec![0usize; (r + 1) * w];
        let idx = |i: usize, j: usize| i * w + j;
        if r > 0 && c > 0 {
            for i in (0..r).rev() {
                for j in (0..c).rev() {
                    l[idx(i, j)] = if xs[i] == ys[j] {
                        l[idx(i + 1, j + 1)] + 1
                    } else {
                        l[idx(i + 1, j)].max(l[idx(i, j + 1)])
                    };
                }
            }
        }
        let (mut i, mut j) = (0usize, 0usize);
        while i < r || j < c {
            if i < r && j < c && xs[i] == ys[j] {
                raw.push((DiffOpKind::Same, xs[i].clone()));
                i += 1;
                j += 1;
            } else if j < c && (i == r || l[idx(i, j + 1)] >= l[idx(i + 1, j)]) {
                raw.push((DiffOpKind::Ins, ys[j].clone()));
                j += 1;
            } else {
                raw.push((DiffOpKind::Del, xs[i].clone()));
                i += 1;
            }
        }
    }
    for t in &x[n - suf..] {
        raw.push((DiffOpKind::Same, t.clone()));
    }
    raw
}

fn flush_group(
    out: &mut Vec<(DiffOpKind, String)>,
    dels: &mut String,
    ins: &mut String,
) {
    let all_ws = |s: &str| s.chars().all(|c| c.is_whitespace());
    if all_ws(dels) && all_ws(ins) {
        if !ins.is_empty() {
            if let Some(last) = out.last_mut() {
                if last.0 == DiffOpKind::Same {
                    last.1.push_str(ins);
                } else {
                    out.push((DiffOpKind::Same, ins.clone()));
                }
            } else {
                out.push((DiffOpKind::Same, ins.clone()));
            }
        }
    } else {
        if !dels.is_empty() {
            out.push((DiffOpKind::Del, dels.clone()));
        }
        if !ins.is_empty() {
            out.push((DiffOpKind::Ins, ins.clone()));
        }
    }
    dels.clear();
    ins.clear();
}

/// `_char_group`.
fn char_group(raw: Vec<(DiffOpKind, String)>) -> Vec<(DiffOpKind, String)> {
    let mut ops = raw;
    let mut k = 1;
    while k + 1 < ops.len() {
        let splits = ops[k].0 == DiffOpKind::Same
            && !ops[k].1.is_empty()
            && ops[k].1.chars().all(|c| c == ' ')
            && ops[k - 1].0 != DiffOpKind::Same
            && ops[k + 1].0 != DiffOpKind::Same;
        if splits {
            let t = ops[k].1.clone();
            ops.splice(k..k + 1, [(DiffOpKind::Del, t.clone()), (DiffOpKind::Ins, t)]);
            k += 2;
        } else {
            k += 1;
        }
    }
    let mut out: Vec<(DiffOpKind, String)> = Vec::new();
    let mut dels = String::new();
    let mut ins = String::new();
    for (kind, text) in ops {
        match kind {
            DiffOpKind::Del => dels.push_str(&text),
            DiffOpKind::Ins => ins.push_str(&text),
            DiffOpKind::Same => {
                flush_group(&mut out, &mut dels, &mut ins);
                if let Some(last) = out.last_mut() {
                    if last.0 == DiffOpKind::Same {
                        last.1.push_str(&text);
                    } else {
                        out.push((DiffOpKind::Same, text));
                    }
                } else {
                    out.push((DiffOpKind::Same, text));
                }
            }
        }
    }
    flush_group(&mut out, &mut dels, &mut ins);
    out
}

/// `CharDiff.changes(CharDiff.diff(a, b))` — the number of change runs.
fn diff_change_count(a: &str, b: &str) -> usize {
    let ops = char_group(char_raw_ops(a, b));
    let mut n = 0;
    let mut in_change = false;
    for (kind, _) in ops {
        if kind == DiffOpKind::Same {
            in_change = false;
        } else if !in_change {
            n += 1;
            in_change = true;
        }
    }
    n
}

/// `stripANSI` — drop `ESC [ … <letter>` sequences.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                while let Some(&n) = chars.peek() {
                    chars.next();
                    if n.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Preview HTML (`previewPage` / `showPreviewMessage`)
// ---------------------------------------------------------------------------

/// The dim color `previewPage` passes to CSS. The Swift build reads the live
/// `PopupColors.dim`; the Rust model holds no theme, so a neutral dim matches
/// the transparent-background look.
pub const PREVIEW_DIM: &str = "#88919e";

/// `previewPage(_:_:)` — the fragment inside the Outlook / Webex paste chrome.
pub fn preview_page(fragment: &str, target: PasteTarget) -> String {
    let dim = PREVIEW_DIM;
    let (note, card, who) = match target {
        PasteTarget::Outlook => (
            "Pastes as rich text: tables, code and lists keep their formatting.",
            "background:#ffffff;border-radius:6px;padding:16px 18px;box-shadow:0 1px 3px rgba(0,0,0,.35)",
            "",
        ),
        PasteTarget::Webex => (
            "Webex has no tables: they paste as aligned text. Markdown also works if you paste as plain text.",
            "background:#f4f5f7;border-radius:14px;padding:12px 14px;box-shadow:0 1px 3px rgba(0,0,0,.35)",
            "<div style=\"font:600 12px -apple-system;color:#555;margin-bottom:6px\">You \u{00b7} now</div>",
        ),
    };
    format!(
        "<html><head><meta charset=\"utf-8\"><style>\n\
html,body{{margin:0;background:transparent}}\n\
.cap{{font:600 10.5px -apple-system;letter-spacing:.06em;color:{dim};margin:8px 12px 6px}}\n\
.note{{font:11px -apple-system;color:{dim};margin:8px 12px 12px}}\n\
.card{{margin:0 10px;{card}}}\n\
a{{color:#0f6cbd}}\n\
</style></head><body>\n\
<div class=\"cap\">{title} \u{00b7} AS IT WILL PASTE</div>\n\
<div class=\"card\">{who}{fragment}</div>\n\
<div class=\"note\">{note}</div>\n\
</body></html>\n",
        title = target.title().to_uppercase(),
    )
}

/// `showPreviewMessage(_:)` — escape `&` and `<` (Swift escapes those two only).
pub fn preview_message(message: &str) -> String {
    let esc = message.replace('&', "&amp;").replace('<', "&lt;");
    format!(
        "<html><body style=\"margin:0;background:transparent;font:13px -apple-system;color:{PREVIEW_DIM}\">\n\
<div style=\"padding:14px 16px\">{esc}</div></body></html>\n"
    )
}

// ---------------------------------------------------------------------------
// Keys (`handleKey`)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AIKeyAction {
    /// Esc while a run is in flight (`cancelRun()`).
    Stop,
    /// Ctrl/Cmd+Return (or keypad Enter) — `run()`.
    Run,
    /// Cmd+L — `focusInput()`.
    FocusInput,
    /// Cmd+/ — `showShortcuts()`.
    ShowShortcuts,
    /// Esc with no run: `slot.escapeAtTop(.ai)` when the host has the hook.
    EscapeAtTop,
    /// Not consumed (the `webEditKey` fallthrough).
    Pass,
}

/// `AIWindow.handleKey`, as a pure function.
pub fn route_ai_key(key: KeyInputLike, running: bool) -> AIKeyAction {
    match key.key_code {
        KEY_ESC => {
            if running {
                AIKeyAction::Stop
            } else {
                AIKeyAction::EscapeAtTop
            }
        }
        KEY_RETURN | KEY_KEYPAD_ENTER if key.ctrl || key.cmd => AIKeyAction::Run,
        KEY_L if key.cmd => AIKeyAction::FocusInput,
        KEY_SLASH if key.cmd => AIKeyAction::ShowShortcuts,
        _ => AIKeyAction::Pass,
    }
}

/// A tiny key shape so [`route_ai_key`] stays testable without the popup
/// module's `KeyInput` (which is `Copy` + `Eq` and could be used directly, but
/// keeping the router dependency-light is deliberate).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyInputLike {
    pub key_code: u16,
    pub cmd: bool,
    pub ctrl: bool,
}

impl KeyInputLike {
    pub fn new(key_code: u16) -> Self {
        KeyInputLike { key_code, cmd: false, ctrl: false }
    }
    pub fn cmd(mut self) -> Self {
        self.cmd = true;
        self
    }
    pub fn ctrl(mut self) -> Self {
        self.ctrl = true;
        self
    }
}

// ---------------------------------------------------------------------------
// Copy model (`copyAnswer` / `copyMarkdown`)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CopyPlan {
    Nothing,
    Markdown(String),
    /// The pasteboard write is AppKit (`RichText.copy`).
    Rich,
}

// ---------------------------------------------------------------------------
// The window model
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AITone {
    Dim,
    Success,
    Warning,
    Danger,
}

#[derive(Debug)]
pub struct AIWindowModel {
    pub config: AIConfig,
    pub rules: Vec<AIRule>,
    pub selected: usize,
    pub mode: PaneMode,
    pub target: PasteTarget,
    pub run: AIRunState,
    pub input: String,
    pub status: String,
    pub status_tone: AITone,
    pub available: Option<bool>,
    pub unavailable_why: String,
    pub rendered: HashMap<String, String>,
    #[cfg(target_os = "macos")]
    preview: Option<macos::AIPreview>,
    /// The live `fm respond --stream` child (killed by `cancel_run` / `Drop`).
    process: Option<Child>,
    /// Reader-thread → UI event queue (see [`StreamEvent`]).
    stream: StreamQueue,
    /// stderr accumulated for the run in flight (drained on completion).
    pending_err: String,
    /// stdout / stderr pipe EOF flags for the run in flight.
    stdout_eof: bool,
    stderr_eof: bool,
    shown: Cell<bool>,
    key: Cell<bool>,
    frame: Cell<Option<RectI>>,
    level: Cell<i64>,
}

impl Default for AIWindowModel {
    fn default() -> Self {
        Self::new(AIConfig::default())
    }
}

impl AIWindowModel {
    pub fn new(config: AIConfig) -> Self {
        AIWindowModel {
            config,
            rules: Vec::new(),
            selected: 0,
            mode: PaneMode::Diff,
            target: PasteTarget::Outlook,
            run: AIRunState::default(),
            input: String::new(),
            status: String::new(),
            status_tone: AITone::Dim,
            available: None,
            unavailable_why: String::new(),
            rendered: HashMap::new(),
            #[cfg(target_os = "macos")]
            preview: None,
            process: None,
            stream: Arc::new(Mutex::new(Vec::new())),
            pending_err: String::new(),
            stdout_eof: false,
            stderr_eof: false,
            shown: Cell::new(false),
            key: Cell::new(false),
            frame: Cell::new(None),
            level: Cell::new(0),
        }
    }

    pub fn rules_dir(&self) -> String {
        self.config.rules_dir()
    }

    /// `var whereText`.
    pub fn where_text(&self) -> String {
        self.rule().map(|r| r.name.clone()).unwrap_or_default()
    }

    pub fn rule(&self) -> Option<&AIRule> {
        self.rules.get(self.selected)
    }

    /// `modes` — Diff only when the rule wants a diff.
    pub fn modes(&self) -> Vec<PaneMode> {
        PaneMode::all(self.rule().map(|r| r.diff()).unwrap_or(true))
    }

    /// `setMode(_:)`.
    pub fn set_mode(&mut self, m: PaneMode) {
        let modes = self.modes();
        self.mode = if modes.contains(&m) { m } else { PaneMode::Markdown };
        if let Some(t) = self.mode.target() {
            self.target = t;
        }
        self.render_answer();
    }

    /// `updateCopyTitle()`.
    pub fn copy_title(&self) -> String {
        format!("\u{29c9} Copy for {}", self.target.title())
    }

    /// `reloadRules()` — needs a directory; the window passes `rulesDir`.
    pub fn reload_rules(&mut self, dir: &str) {
        let keep = self.rule().map(|r| r.file());
        let names = rule_files(dir);
        self.rules = names
            .iter()
            .map(|n| AIRule::load(&format!("{dir}/{n}")))
            .collect();
        self.selected = keep
            .and_then(|k| self.rules.iter().position(|r| r.file() == k))
            .unwrap_or(0);
        self.apply_rule_modes();
    }

    /// `select(_:)`.
    pub fn select(&mut self, i: usize) {
        if i < self.rules.len() {
            self.selected = i;
            self.apply_rule_modes();
        }
    }

    /// `applyRule(loadInput:)`'s mode half: keep the mode in the allowed set.
    pub fn apply_rule_modes(&mut self) {
        if !self.modes().contains(&self.mode) {
            self.mode = PaneMode::Markdown;
        }
    }

    pub fn set_status(&mut self, s: impl Into<String>, tone: AITone) {
        self.status = s.into();
        self.status_tone = tone;
    }

    /// `checkAvailable()` — record the `fm available` result.
    pub fn set_available(&mut self, ok: bool, why: &str) {
        self.available = Some(ok);
        self.unavailable_why = if ok { String::new() } else { why.to_string() };
    }

    /// `run()`'s validation + state setup (no process). Use [`Self::start_run`]
    /// to also spawn the first `fm` child.
    pub fn begin_run(&mut self, input_text: &str) -> Result<(), String> {
        let Some(picked) = self.rule().cloned() else {
            return Err("no rules — press + to make one".to_string());
        };
        let steps = AIRule::chain(&picked);
        let text = input_text.trim();
        if text.is_empty() {
            self.set_status("Type something on the left first", AITone::Warning);
            return Err("empty input".to_string());
        }
        if self.available == Some(false) {
            let why = self.unavailable_why.clone();
            self.set_status(why, AITone::Danger);
            return Err("Apple Intelligence unavailable".to_string());
        }
        let protect = steps[0].protect_code();
        self.input = input_text.to_string();
        self.run.start(steps, text, protect);
        self.set_status("Asking the on-device model\u{2026}", AITone::Dim);
        Ok(())
    }

    /// `run()` end-to-end: validate + set up ([`Self::begin_run`]) then spawn
    /// the first `fm respond --stream` child ([`Self::spawn_part`]). The caller
    /// drives the stream with [`Self::poll_stream`] until `run.running` clears.
    /// A spawn failure is reported in `status` (danger), not as an `Err`, just
    /// like `run()`'s `finish(gen:code:-1)` path.
    pub fn start_run(&mut self, input_text: &str) -> Result<(), String> {
        self.begin_run(input_text)?;
        self.spawn_part();
        Ok(())
    }

    /// `runPart(_:)` — spawn `fm respond --stream` and stream its output.
    ///
    /// The rule/part come from the current [`AIRunState`] (the caller has run
    /// `begin_run` / `start_step`). argv is `fmBin` + `r.arguments(guarded:)`
    /// and stdin is `r.wrap(parts[partIndex])` — see `pylib/ai_format.py`
    /// `rule_arguments` / `rule_wrap`. Spawn failures set a danger status (no
    /// panic); stdout/stderr are pumped by detached reader threads into
    /// `self.stream`, which [`Self::poll_stream`] drains.
    pub fn spawn_part(&mut self) {
        if let Err(err) = self.spawn_process() {
            // `finish(gen:code:-1, err:)`'s failure tail, without a child.
            self.run.running = false;
            self.set_status(format!("fm failed: {}", strip_ansi(&err)), AITone::Danger);
        }
    }

    /// Do the actual `Process`/pipes/threads work of `runPart(_:)`. Returns the
    /// `couldn't start …` message on a spawn error.
    fn spawn_process(&mut self) -> Result<(), String> {
        let Some(rule) = self.run.steps.get(self.run.step_index).cloned() else {
            return Err("no active rule".to_string());
        };
        let Some(part) = self.run.parts.get(self.run.part_index).cloned() else {
            return Err("no part to send".to_string());
        };
        let fm = self.config.fm_bin();
        let guarded = self.run.guard_code.is_some();
        let args = rule.arguments(guarded);

        let n = self.run.parts.len();
        let step_note = if self.run.steps.len() > 1 {
            format!(
                " {} ({} of {})",
                rule.name,
                self.run.step_index + 1,
                self.run.steps.len()
            )
        } else {
            String::new()
        };
        let part_note = if n > 1 {
            format!(" part {} of {n}", self.run.part_index + 1)
        } else {
            String::new()
        };
        self.set_status(
            format!("Asking the on-device model\u{2026}{step_note}{part_note}"),
            AITone::Dim,
        );

        self.pending_err.clear();
        self.stdout_eof = false;
        self.stderr_eof = false;

        let mut child = Command::new(&fm)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("couldn't start {fm}: {e}"))?;

        let gen = self.run.run_gen;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        // Feed the part on a utility thread (Swift's global-queue write) so a
        // full pipe can't stall the caller; dropping the handle closes stdin.
        let payload = rule.wrap(&part);
        thread::spawn(move || {
            if let Some(mut si) = stdin {
                let _ = si.write_all(payload.as_bytes());
            }
        });

        if let Some(out) = stdout {
            let q = Arc::clone(&self.stream);
            thread::spawn(move || pump(out, gen, &q, true));
        }
        if let Some(err) = stderr {
            let q = Arc::clone(&self.stream);
            thread::spawn(move || pump(err, gen, &q, false));
        }

        self.process = Some(child);
        Ok(())
    }

    /// Drain the reader-thread queue and advance the run. Never blocks: it
    /// applies chunks, then uses `try_wait` plus the pipe EOF markers to detect
    /// completion, exactly like the `runPart` completion path (`finish`).
    pub fn poll_stream(&mut self) {
        let gen = self.run.run_gen;
        let events = match self.stream.lock() {
            Ok(mut q) => std::mem::take(&mut *q),
            Err(_) => Vec::new(),
        };
        for ev in events {
            match ev {
                StreamEvent::Chunk { gen: g, data } if g == gen => self.run.push_chunk(&data),
                StreamEvent::Stderr { gen: g, data } if g == gen => {
                    self.pending_err.push_str(&data);
                }
                StreamEvent::StdoutEof { gen: g } if g == gen => self.stdout_eof = true,
                StreamEvent::StderrEof { gen: g } if g == gen => self.stderr_eof = true,
                // a cancelled / superseded run's tail is ignored.
                _ => {}
            }
        }

        let exit = match self.process.as_mut() {
            Some(child) => match child.try_wait() {
                Ok(Some(status)) => Some(status.code().unwrap_or(-1)),
                Ok(None) => None,
                Err(_) => Some(-1),
            },
            None => None,
        };
        let Some(code) = exit else {
            return;
        };
        // Wait for both pipes to drain before finishing so trailing output and
        // stderr aren't lost to the exit race.
        if !(self.stdout_eof && self.stderr_eof) {
            return;
        }
        self.process = None;
        let err = std::mem::take(&mut self.pending_err);
        self.finish_part(code, &err);
    }

    /// Advance to the next part / step, or finish the run with a status.
    fn finish_part(&mut self, code: i32, err: &str) {
        match self.run.finish_step(code, err) {
            StepOutcome::Continue => self.spawn_part(),
            StepOutcome::Failed => {
                self.set_status(
                    format!("fm failed: {}", last_error_line(err, code)),
                    AITone::Danger,
                );
            }
            StepOutcome::Done { missing, .. } => self.complete_status(missing),
        }
    }

    /// The success tail of `finish(gen:code:err:)`: notes / missing-code /
    /// diff-change / plain "Done" status.
    fn complete_status(&mut self, missing: i64) {
        let secs = format!("{:.1} s", (now_secs() - self.run.started).max(0.0));
        let parts_note = if self.run.parts.len() > 1 {
            format!(" \u{00b7} {} parts", self.run.parts.len())
        } else {
            String::new()
        };
        if !self.run.step_notes.is_empty() {
            self.set_status(
                format!("{} \u{00b7} {secs}", self.run.step_notes.join(" \u{00b7} ")),
                AITone::Warning,
            );
        } else if missing > 0 {
            let block = if missing == 1 { "code block" } else { "code blocks" };
            self.set_status(
                format!("The model dropped {missing} {block} \u{2014} check before sending{parts_note}"),
                AITone::Warning,
            );
        } else if self.run.answer_diff {
            let n = diff_change_count(&self.run.answer_input, &self.run.answer);
            let tone = if n == 0 { AITone::Success } else { AITone::Dim };
            let head = if n == 0 {
                "No changes \u{2713}".to_string()
            } else {
                format!("{n} change{}", if n == 1 { "" } else { "s" })
            };
            self.set_status(format!("{head} \u{00b7} {secs}{parts_note}"), tone);
        } else {
            self.set_status(format!("Done \u{00b7} {secs}{parts_note}"), AITone::Dim);
        }
    }

    /// SIGKILL the in-flight child (if any) and reap it off the main thread.
    fn kill_process(&mut self) {
        if let Some(mut child) = self.process.take() {
            let _ = child.kill();
            thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }

    /// `cancelRun(quiet:)`.
    pub fn cancel_run(&mut self) {
        self.run.cancel();
        self.kill_process();
        self.pending_err.clear();
        self.stdout_eof = false;
        self.stderr_eof = false;
        self.set_status("Stopped", AITone::Dim);
    }

    /// `copyAnswer()` / `copyMarkdown()`'s decision.
    pub fn copy_plan(&self, markdown_only: bool) -> CopyPlan {
        if self.run.answer.is_empty() {
            return CopyPlan::Nothing;
        }
        if markdown_only || !RichText::available() {
            let md = RichText::markdown(&self.run.answer, self.target);
            return CopyPlan::Markdown(md);
        }
        CopyPlan::Rich
    }

    /// `renderAnswer()` — swap the native output for the web preview on the
    /// Outlook / Webex modes (Diff and Markdown stay native).
    pub fn render_answer(&mut self) {
        self.rendered.clear();
        #[cfg(target_os = "macos")]
        {
            if self.mode.target().is_some() {
                self.show_preview_macos();
            } else if let Some(p) = &self.preview {
                p.set_hidden(true);
            }
        }
    }

    /// `showPreview()` — load `RichText.html` into the right-pane `WKWebView`.
    pub fn show_preview(&mut self) {
        #[cfg(target_os = "macos")]
        self.show_preview_macos();
    }

    #[cfg(target_os = "macos")]
    fn show_preview_macos(&mut self) {
        use objc2::MainThreadMarker;
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let Some(target) = self.mode.target() else {
            if let Some(p) = &self.preview {
                p.set_hidden(true);
            }
            return;
        };
        if self.preview.is_none() {
            self.preview = Some(macos::AIPreview::new(mtm));
        }
        let Some(p) = self.preview.as_ref() else {
            return;
        };
        p.set_hidden(false);
        if self.run.answer.is_empty() {
            let msg = if self.run.running {
                "Waiting for the model\u{2026}"
            } else {
                "Run a rule \u{2014} the answer shows here as it will look pasted into your target."
            };
            p.load_html(&preview_message(msg));
            return;
        }
        if !RichText::available() {
            p.load_html(&preview_message(&format!(
                "pandoc not found at {} \u{2014} brew install pandoc. Copy puts the Markdown on the clipboard meanwhile.",
                RichText::pandoc_bin()
            )));
            return;
        }
        match RichText::html(&self.run.answer, target) {
            Some(h) => p.load_html(&preview_page(&h, target)),
            None => p.load_html(&preview_message("pandoc couldn't convert this text.")),
        }
    }

    /// `handleKey(_:)`.
    pub fn handle_key(&mut self, key: KeyInputLike) -> AIKeyAction {
        let action = route_ai_key(key, self.run.running);
        match action {
            AIKeyAction::Stop => self.cancel_run(),
            AIKeyAction::Run => {
                let input = self.input.clone();
                let _ = self.begin_run(&input);
            }
            _ => {}
        }
        action
    }

    /// `do:ai:*` hooks.
    pub fn handle_do(&mut self, action: &str) -> Option<Value> {
        let rest = action.strip_prefix("ai:")?;
        match rest {
            "state" => Some(self.test_state()),
            "run" => {
                let input = self.input.clone();
                let _ = self.begin_run(&input);
                Some(self.test_state())
            }
            "stop" => {
                self.cancel_run();
                Some(self.test_state())
            }
            "apply-modes" => {
                self.apply_rule_modes();
                Some(self.test_state())
            }
            _ => {
                if let Some(v) = rest.strip_prefix("select:") {
                    if let Ok(i) = v.parse::<usize>() {
                        self.select(i);
                        return Some(self.test_state());
                    }
                    return None;
                }
                if let Some(v) = rest.strip_prefix("mode:") {
                    self.set_mode(match v {
                        "diff" => PaneMode::Diff,
                        "markdown" => PaneMode::Markdown,
                        "outlook" => PaneMode::Outlook,
                        "webex" => PaneMode::Webex,
                        _ => return None,
                    });
                    return Some(self.test_state());
                }
                if let Some(v) = rest.strip_prefix("input:") {
                    self.input = v.to_string();
                    return Some(self.test_state());
                }
                if let Some(v) = rest.strip_prefix("answer:") {
                    self.run.answer = v.to_string();
                    self.run.answer_input = self.input.clone();
                    return Some(self.test_state());
                }
                None
            }
        }
    }

    pub fn test_state(&self) -> Value {
        json!({
            "shown": self.shown.get(),
            "key": self.key.get(),
            "level": self.level.get(),
            "frame": self.frame.get(),
            "where": self.where_text(),
            "selected": self.selected,
            "mode": self.mode.as_str(),
            "modes": self.modes().iter().map(|m| m.as_str()).collect::<Vec<_>>(),
            "target": self.target.key(),
            "running": self.run.running,
            "answer": self.run.answer,
            "answerDiff": self.run.answer_diff,
            "steps": self.run.steps.len(),
            "stepIndex": self.run.step_index,
            "part": self.run.part_index,
            "parts": self.run.parts.len(),
            "available": self.available,
            "status": self.status,
            "statusTone": match self.status_tone {
                AITone::Dim => "dim",
                AITone::Success => "success",
                AITone::Warning => "warning",
                AITone::Danger => "danger",
            },
            "copyTitle": self.copy_title(),
            "input": self.input,
            "rules": self.rules.iter().map(|r| json!({
                "file": r.file(), "name": r.name, "path": r.path, "diff": r.diff(),
            })).collect::<Vec<_>>(),
        })
    }
}

impl Drop for AIWindowModel {
    fn drop(&mut self) {
        // Kill on drop: never leave a detached `fm` streaming at teardown.
        if let Some(mut child) = self.process.take() {
            let _ = child.kill();
        }
    }
}

impl SlotMember for AIWindowModel {
    fn view(&self) -> SlotView {
        SlotView::Ai
    }
    fn shown(&self) -> bool {
        self.shown.get()
    }
    fn is_key(&self) -> bool {
        self.key.get()
    }
    fn frame(&self) -> Option<RectI> {
        self.frame.get()
    }
    fn slot_show(&self, frame: Option<RectI>) {
        self.shown.set(true);
        if let Some(f) = frame {
            self.frame.set(Some(f));
        }
    }
    fn slot_park(&self, _stop_voice: bool) {
        self.shown.set(false);
        self.key.set(false);
    }
    fn test_state(&self) -> Value {
        AIWindowModel::test_state(self)
    }
}

/// Register the AI palette entry (only while enabled, mirroring
/// `paletteCommands()`).
pub fn register(reg: &mut Registry, enabled: bool, listed: bool) {
    if enabled && listed {
        reg.add_palette(PaletteCommand::new("ai", "AI View", "views"));
    }
}

/// `AIWindow` — host the right-pane `WKWebView` preview.
pub fn build_window() {
    #[cfg(target_os = "macos")]
    macos::build_ai_window();
}

/// The embeddable AI content view for the shared host window: the rules popup +
/// editable input on the left, the live [`macos::AIPreview`] web view on the
/// right. Builds from model values and the rules directory only — no python
/// helper and no `fm` child. `None` off the main thread.
#[cfg(target_os = "macos")]
pub fn build_content(
    mtm: objc2::MainThreadMarker,
) -> Option<objc2::rc::Retained<objc2_app_kit::NSView>> {
    macos::build_content(mtm)
}

/// The right pane's `WKWebView` (Markdown / Outlook / Webex) and its link
/// handling.
#[cfg(target_os = "macos")]
pub mod macos {
    use super::{
        preview_message, rules_from_dir, AIConfig, AITone, AIWindowModel, CopyPlan, RichText,
    };
    use crate::ui::theme::{PopupColors, Rgba};
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, ProtocolObject};
    use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSAutoresizingMaskOptions, NSButton, NSColor, NSFont, NSFontAttributeName,
        NSForegroundColorAttributeName, NSPasteboard, NSPasteboardTypeString, NSPopUpButton,
        NSScrollView, NSTextField, NSTextView, NSStringDrawing, NSView, NSWorkspace,
    };
    use objc2_foundation::{
        ns_string, NSAttributedStringKey, NSDictionary, NSObject, NSObjectProtocol, NSPoint,
        NSRect, NSSize, NSURL, NSString, NSTimer,
    };
    use objc2_web_kit::{
        WKScriptMessage, WKScriptMessageHandler, WKUserContentController, WKUserScript,
        WKUserScriptInjectionTime, WKWebView, WKWebViewConfiguration,
    };
    use std::cell::RefCell;

    fn as_any<T: objc2::Message + ?Sized>(obj: &T) -> &AnyObject {
        unsafe { &*(obj as *const T as *const AnyObject) }
    }

    fn open_external(url: &str) {
        if let Some(ns) = NSURL::URLWithString(&NSString::from_str(url)) {
            NSWorkspace::sharedWorkspace().openURL(&ns);
        }
    }

    pub struct LinkHandlerIvars;

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSAILinkHandler"]
        #[ivars = LinkHandlerIvars]
        pub struct LinkHandler;

        impl LinkHandler {
            #[unsafe(method(userContentController:didReceiveScriptMessage:))]
            fn did_receive(
                &self,
                _controller: &WKUserContentController,
                message: &WKScriptMessage,
            ) {
                let body = unsafe { message.body() };
                if let Some(s) = as_any(&*body).downcast_ref::<NSString>() {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s.to_string()) {
                        if let Some(href) = v.get("href").and_then(|h| h.as_str()) {
                            open_external(href);
                        }
                    }
                }
            }
        }

        unsafe impl NSObjectProtocol for LinkHandler {}
        unsafe impl WKScriptMessageHandler for LinkHandler {}
    );

    impl LinkHandler {
        pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
            let this = LinkHandler::alloc(mtm).set_ivars(LinkHandlerIvars);
            unsafe { msg_send![super(this), init] }
        }
    }

    /// The right-pane web view. `copy` targets the loaded HTML; the native
    /// NSTextView half of `renderAnswer()` stays in the AppKit port.
    pub struct AIPreview {
        pub web: Retained<WKWebView>,
        pub link: Retained<LinkHandler>,
    }

    impl std::fmt::Debug for AIPreview {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("AIPreview")
        }
    }

    impl AIPreview {
        pub fn new(mtm: MainThreadMarker) -> Self {
            let config = unsafe { WKWebViewConfiguration::new(mtm) };
            let ucc = unsafe { config.userContentController() };
            let link = LinkHandler::new(mtm);
            unsafe {
                ucc.addScriptMessageHandler_name(ProtocolObject::from_ref(&*link), ns_string!("ws"));
            }
            let src = NSString::from_str(super::super::confluence::link_user_script());
            let user_script = unsafe {
                WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
                    WKUserScript::alloc(mtm),
                    &src,
                    WKUserScriptInjectionTime::AtDocumentEnd,
                    true,
                )
            };
            unsafe { ucc.addUserScript(&user_script) };
            let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(320.0, 240.0));
            let web = unsafe {
                WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), frame, &config)
            };
            AIPreview { web, link }
        }

        pub fn set_hidden(&self, hidden: bool) {
            let _: () = unsafe { msg_send![&*self.web, setHidden: hidden] };
        }

        /// `web.loadHTMLString(page, baseURL: nil)`.
        pub fn load_html(&self, html: &str) {
            unsafe {
                self.web
                    .loadHTMLString_baseURL(&NSString::from_str(html), None);
            }
        }
    }

    thread_local! {
        static LIVE_WINDOWS: std::cell::RefCell<Vec<(Retained<crate::ui::card::CardNSWindow>, AIPreview)>>
            = const { std::cell::RefCell::new(Vec::new()) };
    }

    /// `AIWindow`'s AppKit build: a themed card hosting the preview pane (the
    /// input / rule sidebar are separate surfaces).
    pub fn build_ai_window() {
        use crate::ui::card::{create_card_window, CardConfig};
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let preview = AIPreview::new(mtm);
        let config = CardConfig {
            title: "AI".to_string(),
            min_size: (480.0, 360.0),
            ..Default::default()
        };
        let window = create_card_window(mtm, &config);
        window.setContentView(Some(&preview.web));
        window.center();
        window.makeKeyAndOrderFront(None);
        LIVE_WINDOWS.with(|w| w.borrow_mut().push((window, preview)));
    }

    // -- embeddable content view (`build_content`) -------------------------

    pub struct AIFlippedIvars;

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSAIFlippedView"]
        #[ivars = AIFlippedIvars]
        pub struct AIFlippedView;

        impl AIFlippedView {
            #[unsafe(method(isFlipped))]
            fn is_flipped(&self) -> bool {
                true
            }
        }

        unsafe impl NSObjectProtocol for AIFlippedView {}
    );

    impl AIFlippedView {
        fn new(mtm: MainThreadMarker) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(AIFlippedIvars);
            unsafe {
                msg_send![
                    super(this),
                    initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0))
                ]
            }
        }
    }

    /// An editable `NSTextView` that paints a dim placeholder while it is empty
    /// (mirrors `AITextView`'s `placeholder` / `placeholderColor`).
    pub struct PlaceholderTextViewIvars {
        pub placeholder: RefCell<String>,
        pub color: RefCell<Rgba>,
    }

    define_class!(
        #[unsafe(super(NSTextView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSAIPlaceholderTextView"]
        #[ivars = PlaceholderTextViewIvars]
        pub struct PlaceholderTextView;

        impl PlaceholderTextView {
            #[unsafe(method(drawRect:))]
            fn draw_rect(&self, dirty: NSRect) {
                let _: () = unsafe { msg_send![super(self), drawRect: dirty] };
                if !self.string().to_string().is_empty() {
                    return;
                }
                let placeholder = self.ivars().placeholder.borrow().clone();
                if placeholder.is_empty() {
                    return;
                }
                let inset = self.textContainerInset();
                let font = self.font().unwrap_or_else(|| NSFont::systemFontOfSize(13.0));
                let color = self.ivars().color.borrow().to_nscolor();
                let attrs = attr_dict(&font, &color);
                let text = NSString::from_str(&placeholder);
                let p = NSPoint::new(inset.width, inset.height);
                unsafe { text.drawAtPoint_withAttributes(p, Some(&attrs)) };
            }
        }

        unsafe impl NSObjectProtocol for PlaceholderTextView {}
    );

    impl PlaceholderTextView {
        fn new(mtm: MainThreadMarker, placeholder: &str, color: Rgba) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(PlaceholderTextViewIvars {
                placeholder: RefCell::new(placeholder.to_string()),
                color: RefCell::new(color),
            });
            unsafe {
                msg_send![
                    super(this),
                    initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0))
                ]
            }
        }
    }

    /// `[.font: font, .foregroundColor: color]` as an attribute dictionary.
    fn attr_dict(
        font: &NSFont,
        color: &NSColor,
    ) -> Retained<NSDictionary<NSAttributedStringKey, AnyObject>> {
        let font_obj: &AnyObject = unsafe { &*(font as *const NSFont as *const AnyObject) };
        let color_obj: &AnyObject = unsafe { &*(color as *const NSColor as *const AnyObject) };
        let keys: [&NSAttributedStringKey; 2] =
            unsafe { [NSFontAttributeName, NSForegroundColorAttributeName] };
        let objs: [&AnyObject; 2] = [font_obj, color_obj];
        NSDictionary::from_slices(&keys, &objs)
    }

    fn label(mtm: MainThreadMarker, s: &str, size: f64, color: Rgba) -> Retained<NSTextField> {
        let l = NSTextField::labelWithString(&NSString::from_str(s), mtm);
        l.setFont(Some(&NSFont::systemFontOfSize(size)));
        l.setTextColor(Some(&color.to_nscolor()));
        l
    }

    fn set_background(view: &NSView, color: Rgba) {
        view.setWantsLayer(true);
        if let Some(layer) = view.layer() {
            layer.setBackgroundColor(Some(&color.to_nscolor().CGColor()));
        }
    }

    fn popup_tone(t: AITone) -> crate::ui::theme::PopupTone {
        use crate::ui::theme::PopupTone;
        match t {
            AITone::Dim => PopupTone::Dim,
            AITone::Success => PopupTone::Success,
            AITone::Warning => PopupTone::Warning,
            AITone::Danger => PopupTone::Danger,
        }
    }

    pub struct AIHandlerIvars {
        pub model: RefCell<AIWindowModel>,
        pub input: Retained<NSTextView>,
        pub status: Retained<NSTextField>,
        pub popup: Retained<NSPopUpButton>,
        pub colors: PopupColors,
        pub preview: AIPreview,
        /// The repeating main-thread drain armed while an `fm` run is live
        /// (mirrors the Swift reader thread's `DispatchQueue.main.async`).
        pub poll_timer: RefCell<Option<Retained<NSTimer>>>,
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSAIHandler"]
        #[ivars = AIHandlerIvars]
        pub struct AIHandler;

        impl AIHandler {
            /// `runClicked` — `process != nil ? cancelRun() : run()`. A fresh
            /// run validates + spawns the first `fm` child ([`AIWindowModel::start_run`])
            /// and arms the main-thread poll; a live run is cancelled instead
            /// (the button doubles as Stop).
            #[unsafe(method(run:))]
            fn run(&self, _sender: Option<&AnyObject>) {
                if MainThreadMarker::new().is_none() {
                    return;
                }
                let text = self.ivars().input.string().to_string();
                let (status, tone, arm) = {
                    let mut m = self.ivars().model.borrow_mut();
                    m.input = text.clone();
                    if m.run.running {
                        m.cancel_run();
                        (m.status.clone(), m.status_tone, false)
                    } else {
                        let _ = m.start_run(&text);
                        (m.status.clone(), m.status_tone, m.run.running)
                    }
                };
                if arm {
                    self.ensure_poll_timer();
                } else {
                    self.stop_poll_timer();
                }
                self.set_status(&status, tone);
            }

            /// The main-thread drain for a live run: apply streamed chunks and,
            /// once the run stops (done / failed / cancelled), invalidate the
            /// timer so it doesn't keep firing.
            #[unsafe(method(pollStream:))]
            fn poll_stream_tick(&self, _timer: &NSTimer) {
                let done = {
                    let mut m = self.ivars().model.borrow_mut();
                    m.poll_stream();
                    !m.run.running
                };
                if done {
                    self.stop_poll_timer();
                }
            }

            /// `copyAnswer()` — the Copy button: render the target's HTML and
            /// write HTML + RTF + plain Markdown in one pasteboard item through
            /// [`RichText::copy`], falling back to a plain Markdown string when
            /// pandoc is unavailable (Swift's `copyMarkdown()` path).
            #[unsafe(method(copyAnswer:))]
            fn copy_answer(&self, _sender: Option<&AnyObject>) {
                if MainThreadMarker::new().is_none() {
                    return;
                }
                let (plan, answer, target, toast) = {
                    let m = self.ivars().model.borrow();
                    (
                        m.copy_plan(false),
                        m.run.answer.clone(),
                        m.target,
                        m.config.copy_toast(),
                    )
                };
                match plan {
                    CopyPlan::Nothing => self.set_status(
                        "Nothing to copy yet \u{2014} run a rule first",
                        AITone::Dim,
                    ),
                    CopyPlan::Markdown(md) => {
                        let pb = NSPasteboard::generalPasteboard();
                        pb.clearContents();
                        let ok = pb.setString_forType(
                            &NSString::from_str(&md),
                            unsafe { NSPasteboardTypeString },
                        );
                        if ok {
                            self.set_status(&toast, AITone::Success);
                        }
                    }
                    CopyPlan::Rich => {
                        let fragment = RichText::html(&answer, target);
                        RichText::copy(&answer, fragment.as_deref(), target);
                        self.set_status(&toast, AITone::Success);
                    }
                }
            }

            /// Paste the clipboard into the input field.
            #[unsafe(method(pasteInput:))]
            fn paste_input(&self, _sender: Option<&AnyObject>) {
                let pb = NSPasteboard::generalPasteboard();
                if let Some(s) = pb.stringForType(unsafe { NSPasteboardTypeString }) {
                    let text = s.to_string();
                    self.ivars().input.setString(&NSString::from_str(&text));
                    self.ivars().model.borrow_mut().input = text;
                }
            }

            /// The rules popup's selection changed (`select(_:)`).
            #[unsafe(method(chooseRule:))]
            fn choose_rule(&self, _sender: Option<&AnyObject>) {
                let i = self.ivars().popup.indexOfSelectedItem();
                if i >= 0 {
                    self.ivars().model.borrow_mut().select(i as usize);
                }
            }
        }

        unsafe impl NSObjectProtocol for AIHandler {}
    );

    impl AIHandler {
        fn set_status(&self, s: &str, tone: AITone) {
            let status = &self.ivars().status;
            status.setStringValue(&NSString::from_str(s));
            status.setTextColor(Some(&self.ivars().colors.tone(popup_tone(tone)).to_nscolor()));
        }

        /// Arm the 30 ms main-thread poll that drains the `fm` stream (the
        /// Rust stand-in for the Swift reader thread's `DispatchQueue.main`
        /// chunk/finish delivery). Idempotent while a run is live.
        fn ensure_poll_timer(&self) {
            if self.ivars().poll_timer.borrow().is_some() {
                return;
            }
            let timer = unsafe {
                NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                    0.03,
                    as_any(self),
                    objc2::sel!(pollStream:),
                    None,
                    true,
                )
            };
            *self.ivars().poll_timer.borrow_mut() = Some(timer);
        }

        fn stop_poll_timer(&self) {
            if let Some(t) = self.ivars().poll_timer.borrow_mut().take() {
                t.invalidate();
            }
        }
    }

    thread_local! {
        static LIVE_HANDLERS: std::cell::RefCell<Vec<Retained<AIHandler>>>
            = const { std::cell::RefCell::new(Vec::new()) };
    }

    /// Build the embeddable AI content tree (see [`super::build_content`]).
    ///
    /// Left: the rules `NSPopUpButton` (names from the directory listing), the
    /// editable input `NSTextView` (with a placeholder), Run / Copy / Paste.
    /// Right: the live [`AIPreview`] `WKWebView` loaded with the empty-state
    /// `preview_message("")`. Everything is built from model values and the
    /// rules directory; nothing here spawns the python helper or `fm`.
    pub fn build_content(mtm: MainThreadMarker) -> Option<Retained<NSView>> {
        let colors = crate::ui::theme::PopupThemeDefaults::colors();
        let cfg = AIConfig::load();
        let mut model = AIWindowModel::new(cfg.clone());
        model.rules = rules_from_dir(&model.rules_dir());
        model.apply_rule_modes();

        let w = cfg.width().max(560.0);
        let h = cfg.height().max(360.0);
        let split = cfg.split();
        let pad = 12.0;

        let root = AIFlippedView::new(mtm);
        root.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w, h)));
        root.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        set_background(&root, colors.mantle());

        // --- top bar: rules popup + action buttons ---------------------------
        let rules_label = label(mtm, "Rules", 11.0, colors.dim);
        rules_label.setFrame(NSRect::new(NSPoint::new(pad, pad + 4.0), NSSize::new(40.0, 18.0)));
        root.addSubview(&rules_label);

        let popup = NSPopUpButton::new(mtm);
        popup.setFrame(NSRect::new(NSPoint::new(pad + 44.0, pad), NSSize::new(240.0, 26.0)));
        if model.rules.is_empty() {
            popup.addItemWithTitle(&NSString::from_str("No rules"));
        } else {
            for r in &model.rules {
                popup.addItemWithTitle(&NSString::from_str(&r.name));
            }
            popup.selectItemAtIndex(0);
        }
        root.addSubview(&popup);

        // --- panes -----------------------------------------------------------
        let title_y = pad + 34.0;
        let body_top = title_y + 18.0;
        let foot_y = h - pad - 16.0;
        let body_h = (foot_y - body_top - 8.0).max(80.0);
        let gap = 10.0;
        let avail = (w - pad * 2.0 - gap).max(1.0);
        let left_w = (avail * split).round().max(1.0);
        let right_w = (avail - left_w).max(1.0);

        let input_title = label(mtm, "Input", 11.0, colors.dim);
        input_title.setFrame(NSRect::new(NSPoint::new(pad, title_y), NSSize::new(left_w, 14.0)));
        root.addSubview(&input_title);

        let preview_title = label(mtm, "Preview", 11.0, colors.dim);
        preview_title.setFrame(NSRect::new(
            NSPoint::new(pad + left_w + gap, title_y),
            NSSize::new(right_w, 14.0),
        ));
        root.addSubview(&preview_title);

        let in_scroll = NSScrollView::new(mtm);
        in_scroll.setFrame(NSRect::new(NSPoint::new(pad, body_top), NSSize::new(left_w, body_h)));
        in_scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        in_scroll.setHasVerticalScroller(true);
        in_scroll.setAutohidesScrollers(true);
        in_scroll.setDrawsBackground(true);
        in_scroll.setBackgroundColor(&colors.crust().to_nscolor());

        let input = PlaceholderTextView::new(
            mtm,
            "Paste or type the text to transform, then Run\u{2026}",
            colors.dim,
        );
        input.setEditable(true);
        input.setSelectable(true);
        input.setRichText(false);
        input.setAllowsUndo(true);
        input.setDrawsBackground(false);
        input.setFont(Some(&NSFont::systemFontOfSize(13.0)));
        input.setTextColor(Some(&colors.text.to_nscolor()));
        input.setInsertionPointColor(Some(&colors.accent_on().to_nscolor()));
        input.setTextContainerInset(NSSize::new(8.0, 8.0));
        input.setVerticallyResizable(true);
        input.setHorizontallyResizable(false);
        input.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        if let Some(container) = unsafe { input.textContainer() } {
            container.setWidthTracksTextView(true);
        }
        input.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(left_w, body_h)));
        in_scroll.setDocumentView(Some(&input));
        root.addSubview(&in_scroll);

        let preview = AIPreview::new(mtm);
        preview.web.setFrame(NSRect::new(
            NSPoint::new(pad + left_w + gap, body_top),
            NSSize::new(right_w, body_h),
        ));
        preview.web.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        preview.load_html(&preview_message(""));
        preview.set_hidden(false);
        root.addSubview(&preview.web);

        let status = label(mtm, "", 11.0, colors.dim);
        status.setFrame(NSRect::new(NSPoint::new(pad, foot_y), NSSize::new(w - pad * 2.0, 16.0)));
        status.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        root.addSubview(&status);

        // --- handler (the buttons need it as their target) -------------------
        let input_view: Retained<NSTextView> = input.clone().into_super();
        let handler = AIHandler::alloc(mtm).set_ivars(AIHandlerIvars {
            model: RefCell::new(model),
            input: input_view,
            status: status.clone(),
            popup: popup.clone(),
            colors,
            preview,
            poll_timer: RefCell::new(None),
        });
        let handler: Retained<AIHandler> = unsafe { msg_send![super(handler), init] };
        LIVE_HANDLERS.with(|v| v.borrow_mut().push(handler.clone()));

        unsafe {
            popup.setTarget(Some(as_any(&*handler)));
            popup.setAction(Some(objc2::sel!(chooseRule:)));
        }

        let run_b = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str("Run"),
                Some(as_any(&*handler)),
                Some(objc2::sel!(run:)),
                mtm,
            )
        };
        let copy_b = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str("Copy"),
                Some(as_any(&*handler)),
                Some(objc2::sel!(copyAnswer:)),
                mtm,
            )
        };
        let paste_b = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str("Paste"),
                Some(as_any(&*handler)),
                Some(objc2::sel!(pasteInput:)),
                mtm,
            )
        };
        let bw = 64.0;
        let bh = 26.0;
        paste_b.setFrame(NSRect::new(
            NSPoint::new(w - pad - bw * 3.0 - 16.0, pad),
            NSSize::new(bw, bh),
        ));
        copy_b.setFrame(NSRect::new(
            NSPoint::new(w - pad - bw * 2.0 - 8.0, pad),
            NSSize::new(bw, bh),
        ));
        run_b.setFrame(NSRect::new(NSPoint::new(w - pad - bw, pad), NSSize::new(bw, bh)));
        for b in [&paste_b, &copy_b, &run_b] {
            b.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
            root.addSubview(b);
        }

        Some(root.into_super())
    }
}

fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ws-ai-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn config_parse_defaults_and_overrides() {
        let empty = AIConfig::default();
        assert_eq!(empty.width(), DEFAULT_WIDTH);
        assert_eq!(empty.height(), DEFAULT_HEIGHT);
        assert_eq!(empty.split(), DEFAULT_SPLIT);
        assert_eq!(empty.sidebar_width(), DEFAULT_SIDEBAR_WIDTH);
        assert_eq!(empty.fm_bin(), "/usr/bin/fm");
        assert_eq!(empty.context_tokens(), DEFAULT_CONTEXT_TOKENS);
        assert_eq!(empty.copy_toast(), DEFAULT_COPY_TOAST);
        assert_eq!(empty.font_size(), 14.0);
        assert!(!empty.enabled());
        assert!(empty.in_palette());
        assert_eq!(empty.label(), "AI View");

        let c = AIConfig::from_entries(entries(&[
            ("enabled", "yes"),
            ("width", "1234"),
            ("height", "777"),
            ("split", "0.6"),
            ("sidebar-width", "180"),
            ("fm-bin", "/usr/local/bin/fm"),
            ("context-tokens", "8192"),
            ("font", "Menlo"),
            ("font-size", "40"),
            ("copy-toast", "Copied {}"),
            ("label", "Writer"),
            ("in-palette", "false"),
        ]));
        assert!(c.enabled());
        assert_eq!(c.width(), 1234.0);
        assert_eq!(c.height(), 777.0);
        assert_eq!(c.split(), 0.6);
        assert_eq!(c.sidebar_width(), 180.0);
        assert_eq!(c.fm_bin(), "/usr/local/bin/fm");
        assert_eq!(c.context_tokens(), 8192);
        assert_eq!(c.font(), "Menlo");
        assert_eq!(c.font_size(), 32.0, "clamped to 32");
        assert_eq!(c.copy_toast(), "Copied {}");
        assert_eq!(c.label(), "Writer");
        assert!(!c.in_palette());

        // a garbage split resets to the default.
        assert_eq!(AIConfig::from_entries(entries(&[("split", "5")])).split(), DEFAULT_SPLIT);
        // a tiny font clamps up to 9.
        assert_eq!(AIConfig::from_entries(entries(&[("font-size", "2")])).font_size(), 9.0);
    }

    #[test]
    fn rule_files_sorted_and_filtered() {
        let dir = temp_dir("rules");
        for name in ["b.md", "A.md", "a10.md", "a2.md", ".hidden.md", "c.txt", "notes.MD"] {
            std::fs::write(dir.join(name), "---\n---\n").unwrap();
        }
        let names = rule_files(dir.to_str().unwrap());
        assert_eq!(names, vec!["A.md", "a2.md", "a10.md", "b.md", "notes.MD"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn localized_standard_cmp_is_natural() {
        use std::cmp::Ordering::{Equal, Greater, Less};
        assert_eq!(localized_standard_cmp("A.md", "a.md"), Equal, "case-insensitive");
        assert_eq!(localized_standard_cmp("a2.md", "a10.md"), Less);
        assert_eq!(localized_standard_cmp("a10.md", "a2.md"), Greater);
        assert_eq!(localized_standard_cmp("a.md", "a2.md"), Less, "'.' < '2'");
        assert_eq!(localized_standard_cmp("a", "ab"), Less);
    }

    #[test]
    fn reload_rules_reads_directory() {
        let dir = temp_dir("reload");
        std::fs::write(dir.join("a.md"), "fix it").unwrap();
        std::fs::write(dir.join("b.md"), "format it").unwrap();
        let mut m = AIWindowModel::default();
        m.reload_rules(dir.to_str().unwrap());
        let files: Vec<String> = m.rules.iter().map(|r| r.file()).collect();
        assert_eq!(files, vec!["a.md", "b.md"]);
        assert_eq!(m.where_text(), "a", "the name falls back to the file stem");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_from_config_text_reads_only_the_ai_section() {
        let text = "[app]\nfoo = 1\n\n[ai]\nenabled = yes\nwidth = 800\nrules-dir = ~/myrules\n\n[files]\nwidth = 12\n";
        let c = AIConfig::from_config_text(text);
        assert!(c.enabled(), "yes is truthy");
        assert_eq!(c.width(), 800.0);
        assert_eq!(c.string("rules-dir", ""), "~/myrules");
        assert_eq!(c.height(), DEFAULT_HEIGHT, "keys from other sections don't leak");
    }

    #[test]
    fn rules_from_dir_uses_file_stems_without_the_helper() {
        let dir = temp_dir("rules-from-dir");
        std::fs::write(dir.join("b.md"), "").unwrap();
        std::fs::write(dir.join("a.md"), "").unwrap();
        std::fs::write(dir.join("skip.txt"), "").unwrap();
        let rules = rules_from_dir(dir.to_str().unwrap());
        let files: Vec<String> = rules.iter().map(|r| r.file()).collect();
        assert_eq!(files, vec!["a.md", "b.md"], "listing order, only .md");
        let names: Vec<String> = rules.iter().map(|r| r.name.clone()).collect();
        assert_eq!(names, vec!["a", "b"], "no `ai.rule_load` → the file stem");
        assert_eq!(rules[0].path, format!("{}/a.md", dir.to_str().unwrap()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn key_routing_matches_handle_key() {
        // Esc stops a run, else asks the host to escapeAtTop.
        assert_eq!(route_ai_key(KeyInputLike::new(KEY_ESC), true), AIKeyAction::Stop);
        assert_eq!(
            route_ai_key(KeyInputLike::new(KEY_ESC), false),
            AIKeyAction::EscapeAtTop
        );
        // Ctrl/Cmd+Return and keypad Enter run.
        assert_eq!(route_ai_key(KeyInputLike::new(KEY_RETURN).ctrl(), false), AIKeyAction::Run);
        assert_eq!(route_ai_key(KeyInputLike::new(KEY_RETURN).cmd(), false), AIKeyAction::Run);
        assert_eq!(
            route_ai_key(KeyInputLike::new(KEY_KEYPAD_ENTER).cmd(), false),
            AIKeyAction::Run
        );
        // a bare Return is not consumed.
        assert_eq!(route_ai_key(KeyInputLike::new(KEY_RETURN), false), AIKeyAction::Pass);
        // Cmd+L focuses the input; Cmd+/ shows the shortcuts card.
        assert_eq!(route_ai_key(KeyInputLike::new(KEY_L).cmd(), false), AIKeyAction::FocusInput);
        assert_eq!(
            route_ai_key(KeyInputLike::new(KEY_SLASH).cmd(), false),
            AIKeyAction::ShowShortcuts
        );
        // plain L / / are not consumed.
        assert_eq!(route_ai_key(KeyInputLike::new(KEY_L), false), AIKeyAction::Pass);
        assert_eq!(route_ai_key(KeyInputLike::new(KEY_SLASH), false), AIKeyAction::Pass);
    }

    #[test]
    fn mode_set_clamps_and_tracks_target() {
        let mut m = AIWindowModel::default();
        m.rules = vec![AIRule::new("/tmp/r.md", "r")]; // plain → no Diff
        assert_eq!(
            m.modes(),
            vec![PaneMode::Markdown, PaneMode::Outlook, PaneMode::Webex],
            "plain rules drop Diff"
        );
        m.set_mode(PaneMode::Diff);
        assert_eq!(m.mode, PaneMode::Markdown, "Diff is not allowed → Markdown");
        m.set_mode(PaneMode::Outlook);
        assert_eq!(m.mode, PaneMode::Outlook);
        assert_eq!(m.target, PasteTarget::Outlook);
        m.set_mode(PaneMode::Webex);
        assert_eq!(m.target, PasteTarget::Webex);
        assert_eq!(m.copy_title(), "\u{29c9} Copy for Webex");

        // a diff rule keeps Diff and lets it be selected.
        let mut diff = AIRule::new("/tmp/d.md", "d");
        diff.output = "diff".to_string();
        m.rules = vec![diff];
        assert!(m.modes().contains(&PaneMode::Diff));
        m.set_mode(PaneMode::Diff);
        assert_eq!(m.mode, PaneMode::Diff);
    }

    #[test]
    fn run_state_single_step_done() {
        let mut run = AIRunState::default();
        let rule = AIRule::new("/tmp/r.md", "r");
        run.start(vec![rule], "hello", true);
        assert!(run.running);
        assert_eq!(run.step_input, "hello", "prepare() falls back to the text");
        assert_eq!(run.parts, vec!["hello".to_string()], "plain rule = one part");
        run.push_chunk("wor");
        run.push_chunk("ld");
        assert_eq!(run.step_so_far(), "world");
        assert_eq!(run.finish_step(0, ""), StepOutcome::Done { missing: 0, warn: false });
        assert_eq!(run.answer, "world");
        assert!(!run.running);
    }

    #[test]
    fn run_state_multi_part_joins_with_blank_lines() {
        let mut run = AIRunState::default();
        let rule = AIRule::new("/tmp/r.md", "r");
        run.start(vec![rule], "hello", false);
        // force two parts (the helper may return one; simulate the split).
        run.parts = vec!["p1".to_string(), "p2".to_string()];
        run.part_index = 0;
        run.stream = "first".to_string();
        assert_eq!(run.finish_step(0, ""), StepOutcome::Continue, "advance part");
        assert_eq!(run.part_index, 1);
        assert_eq!(run.done_parts, vec!["first".to_string()]);
        assert!(run.running);
        assert_eq!(run.step_so_far(), "first");
        run.stream = "second".to_string();
        assert_eq!(run.finish_step(0, ""), StepOutcome::Done { missing: 0, warn: false });
        assert_eq!(run.answer, "first\n\nsecond");
        assert!(!run.running);
    }

    #[test]
    fn run_state_chain_advances_steps() {
        let mut run = AIRunState::default();
        let a = AIRule::new("/tmp/a.md", "a");
        let b = AIRule::new("/tmp/b.md", "b");
        run.start(vec![a, b], "x", false);
        run.push_chunk("A-out");
        assert_eq!(run.finish_step(0, ""), StepOutcome::Continue, "start step 2");
        assert_eq!(run.step_index, 1);
        assert_eq!(run.step_input, "A-out", "the next rule gets the answer");
        assert!(run.running);
        run.push_chunk("B-out");
        assert_eq!(run.finish_step(0, ""), StepOutcome::Done { missing: 0, warn: false });
        assert_eq!(run.answer, "B-out");
    }

    #[test]
    fn run_state_cancel_and_strip_ansi() {
        let mut run = AIRunState::default();
        run.start(vec![AIRule::new("/tmp/r.md", "r")], "hi", false);
        let gen = run.run_gen;
        run.cancel();
        assert!(!run.running);
        assert_eq!(run.run_gen, gen + 1);
        assert_eq!(strip_ansi("\u{1b}[31mred\u{1b}[0m"), "red");
        assert_eq!(strip_ansi("plain"), "plain");
    }

    #[test]
    fn begin_run_validates_and_fills_state() {
        let mut m = AIWindowModel::default();
        assert!(m.begin_run("hi").is_err(), "no rules");

        m.rules = vec![AIRule::new("/tmp/r.md", "r")];
        assert!(m.begin_run("   ").is_err(), "empty input is rejected");
        assert_eq!(m.status_tone, AITone::Warning);

        assert!(m.begin_run("hello").is_ok());
        assert!(m.run.running);
        assert_eq!(m.input, "hello");
        assert!(m.status.starts_with("Asking the on-device model"));

        // Apple Intelligence unavailable short-circuits.
        let mut off = AIWindowModel::default();
        off.rules = vec![AIRule::new("/tmp/r.md", "r")];
        off.set_available(false, "fm not found");
        assert!(off.begin_run("hi").is_err());
        assert_eq!(off.status, "fm not found");
        assert_eq!(off.status_tone, AITone::Danger);
    }

    #[cfg(unix)]
    #[test]
    fn start_run_validates_before_spawning() {
        let dir = temp_dir("start-run");
        let script = fake_fm(&dir, "cat");
        let mut m = model_with_fm(script.to_str().unwrap());

        // empty input: no child, error, warning tone.
        assert!(m.start_run("   ").is_err());
        assert!(m.process.is_none(), "validation failure must not spawn");
        assert!(!m.run.running);

        // valid input: the child is spawned and the run stays live for poll.
        assert!(m.start_run("hello").is_ok());
        assert!(m.process.is_some(), "the fm child is spawned");
        assert!(m.run.running);
        m.cancel_run();
        assert!(m.process.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn start_run_reports_spawn_failure_in_status() {
        let mut m = model_with_fm("/nonexistent/definitely-not-fm");
        // a spawn failure is surfaced in the status, not as an Err.
        assert!(m.start_run("hi").is_ok());
        assert!(!m.run.running);
        assert_eq!(m.status_tone, AITone::Danger);
        assert!(m.status.starts_with("fm failed: couldn't start"));
        assert!(m.process.is_none());
    }

    #[test]
    fn copy_plan_model() {
        let mut m = AIWindowModel::default();
        assert_eq!(m.copy_plan(false), CopyPlan::Nothing);
        m.run.answer = "# Title".to_string();
        // markdown_only always produces the Markdown paste.
        assert_eq!(
            m.copy_plan(true),
            CopyPlan::Markdown(RichText::markdown("# Title", PasteTarget::Outlook))
        );
    }

    #[test]
    fn copy_plan_rich_only_when_pandoc_available() {
        let mut m = AIWindowModel::default();
        m.run.answer = "# Title".to_string();
        let plan = m.copy_plan(false);
        if RichText::available() {
            assert_eq!(plan, CopyPlan::Rich, "pandoc present → HTML+RTF+text path");
        } else {
            assert_eq!(
                plan,
                CopyPlan::Markdown(RichText::markdown("# Title", PasteTarget::Outlook)),
                "no pandoc → plain Markdown fallback"
            );
        }
    }

    #[test]
    fn do_hooks_and_state() {
        let mut m = AIWindowModel::default();
        m.rules = vec![AIRule::new("/tmp/r.md", "r")];
        m.apply_rule_modes();
        let st = m.handle_do("ai:state").unwrap();
        assert_eq!(st["where"], "r");
        assert_eq!(st["mode"], "markdown", "plain rule defaults to Markdown");

        m.handle_do("ai:mode:outlook");
        assert_eq!(m.mode, PaneMode::Outlook);
        assert_eq!(m.target, PasteTarget::Outlook);

        m.handle_do("ai:input:hello world");
        let st = m.handle_do("ai:run").unwrap();
        assert_eq!(st["running"], true);
        assert_eq!(st["input"], "hello world");

        m.handle_do("ai:answer:the answer");
        assert_eq!(m.run.answer, "the answer");

        let st = m.handle_do("ai:stop").unwrap();
        assert_eq!(st["running"], false);

        assert!(m.handle_do("ai:mode:nope").is_none());
        assert!(m.handle_do("ai:select:x").is_none());
        assert!(m.handle_do("other:thing").is_none());
    }

    #[test]
    fn preview_page_wraps_fragment_for_target() {
        let o = preview_page("<p>hi</p>", PasteTarget::Outlook);
        assert!(o.contains("OUTLOOK \u{00b7} AS IT WILL PASTE"));
        assert!(o.contains("<p>hi</p>"));
        assert!(o.contains("background:#ffffff"), "outlook card");
        assert!(o.contains("Pastes as rich text"));
        assert!(!o.contains("You \u{00b7} now"), "no chat header for Outlook");

        let w = preview_page("<p>hi</p>", PasteTarget::Webex);
        assert!(w.contains("WEBEX \u{00b7} AS IT WILL PASTE"));
        assert!(w.contains("background:#f4f5f7"), "webex card");
        assert!(w.contains("You \u{00b7} now"));
        assert!(w.contains("Webex has no tables"));
    }

    #[test]
    fn preview_message_escapes_html() {
        let m = preview_message("a < b & c");
        assert!(m.contains("a &lt; b &amp; c"));
        assert!(!m.contains("a < b"));
    }

    #[test]
    fn slot_view_identity() {
        use crate::app::registry::SlotMember;
        let m = AIWindowModel::default();
        assert_eq!(m.view(), SlotView::Ai);
        assert!(!m.shown());
        m.slot_show(None);
        assert!(m.shown());
        m.slot_park(false);
        assert!(!m.shown());
    }

    // -- streaming (`spawn_part` / `poll_stream`) ---------------------------

    fn model_with_fm(fm: &str) -> AIWindowModel {
        let mut cfg = AIConfig::default();
        cfg.entries.insert("fm-bin".to_string(), fm.to_string());
        let mut m = AIWindowModel::new(cfg);
        m.rules = vec![AIRule::new("/tmp/r.md", "r")];
        m
    }

    /// A fake `fm`: ignores its argv and echoes stdin (so the test doesn't
    /// depend on the helper-supplied `respond --stream` argv, nor on `fm`).
    #[cfg(unix)]
    fn fake_fm(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let script = dir.join("fake-fm.sh");
        std::fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    #[test]
    fn poll_stream_drains_chunks_and_ignores_stale_generation() {
        let mut m = AIWindowModel::default();
        m.rules = vec![AIRule::new("/tmp/r.md", "r")];
        assert!(m.begin_run("x").is_ok());
        let gen = m.run.run_gen;
        {
            let mut q = m.stream.lock().unwrap();
            q.push(StreamEvent::Chunk { gen, data: "ab".to_string() });
            q.push(StreamEvent::Chunk { gen: gen + 1, data: "STALE".to_string() });
            q.push(StreamEvent::Chunk { gen, data: "cd".to_string() });
        }
        m.poll_stream();
        assert_eq!(m.run.step_so_far(), "abcd", "matching chunks apply, stale dropped");
        assert!(m.run.running, "no child → poll cannot finish the run");
    }

    #[cfg(unix)]
    #[test]
    fn spawn_part_streams_to_completion() {
        let dir = temp_dir("stream");
        let script = fake_fm(&dir, "cat");
        let mut m = model_with_fm(script.to_str().unwrap());
        assert!(m.begin_run("hello world").is_ok());
        assert!(m.run.running);
        m.spawn_part();
        assert!(m.process.is_some(), "the child is stored so it can be killed");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while m.run.running && std::time::Instant::now() < deadline {
            m.poll_stream();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!m.run.running, "the run completes on its own (no blocking wait)");
        assert!(m.process.is_none(), "the child is released on completion");
        assert!(
            m.run.answer.contains("hello world"),
            "streamed answer: {:?}",
            m.run.answer
        );
        assert_eq!(m.status_tone, AITone::Dim);
        assert!(m.status.starts_with("Done"), "status: {:?}", m.status);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn spawn_failure_sets_danger_status() {
        let mut m = model_with_fm("/nonexistent/definitely-not-fm");
        assert!(m.begin_run("hi").is_ok());
        assert!(m.run.running);
        m.spawn_part();
        assert!(!m.run.running, "a failed spawn must not leave the run live");
        assert_eq!(m.status_tone, AITone::Danger);
        assert!(
            m.status.starts_with("fm failed: couldn't start"),
            "status: {:?}",
            m.status
        );
        assert!(m.process.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn cancel_run_kills_the_child() {
        let dir = temp_dir("cancel");
        let script = fake_fm(&dir, "sleep 30");
        let mut m = model_with_fm(script.to_str().unwrap());
        assert!(m.begin_run("hi").is_ok());
        m.spawn_part();
        assert!(m.process.is_some());
        m.cancel_run();
        assert!(!m.run.running);
        assert!(m.process.is_none(), "cancel drops the child handle");
        assert_eq!(m.status, "Stopped");
        assert_eq!(m.status_tone, AITone::Dim);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn diff_change_count_matches_char_diff() {
        assert_eq!(diff_change_count("hello world", "hello world"), 0);
        // whitespace-only edits are not changes (the group flush folds them).
        assert_eq!(diff_change_count("a b", "a  b"), 0);
        // one insertion / one word swap → one change run.
        assert_eq!(diff_change_count("hello world", "hello brave world"), 1);
        assert_eq!(diff_change_count("The cat sat", "The dog sat"), 1);
        assert_eq!(diff_change_count("abc", "axc"), 1);
        // two edits separated by unchanged text → two change runs.
        assert_eq!(diff_change_count("one two three", "ONE two THREE"), 2);
    }

    #[test]
    fn last_error_line_uses_final_nonblank_line() {
        assert_eq!(last_error_line("warn\n\nboom\n", 2), "boom");
        assert_eq!(last_error_line("\u{1b}[31mfail\u{1b}[0m", 7), "fail");
        assert_eq!(last_error_line("", 3), "exit 3");
    }

    #[test]
    fn drain_utf8_holds_incomplete_sequences() {
        // "é" = 0xC3 0xA9; the first byte alone is incomplete.
        let mut pending = vec![b'a', 0xC3];
        assert_eq!(drain_utf8(&mut pending), "a");
        assert_eq!(pending, vec![0xC3]);
        pending.push(0xA9);
        assert_eq!(drain_utf8(&mut pending), "\u{e9}");
        assert!(pending.is_empty());
    }
}
