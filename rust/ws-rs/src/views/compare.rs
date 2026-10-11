//! Compare family — port of `CompareText.swift`, `CompareFolder.swift`,
//! `ComparePane.swift`, `CompareWindow.swift` and `CompareFolderView.swift`.
//!
//! The text engine lives in `pylib/compare_text.py` and the folder engine in
//! `pylib/compare_folder.py`, both behind HANDLE-based helper calls
//! (`compare.*` / `folder.*`); the Swift files (and this port) are thin
//! mirrors over those handles. The AppKit drawing layer (`ComparePaneView`,
//! `CompareThumbnail`, `CompareEditor`, `FolderTreeView`) is real over the
//! ported models — see `macos` and [`build_content`].
//!
//! A [`HelperBackend`] seam lets the handle wrappers be unit-tested with a
//! stubbed worker; [`PythonBackend`] is the real `PythonHelper`.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Debug;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::app::python_helper::PythonHelper;
use crate::engines::file_ops::{Clash, FileOps, UndoStack};

// ---------------------------------------------------------------------------
// Helper backend seam
// ---------------------------------------------------------------------------

/// The worker call surface the handle wrappers need. `PythonBackend` forwards
/// to [`PythonHelper::shared`]; tests inject a recording stub.
pub trait HelperBackend: Send + Sync + Debug {
    fn call(&self, method: &str, params: Value, timeout_secs: u64) -> Result<Value, String>;
}

/// The real worker (`PythonHelper.shared().call`).
#[derive(Debug, Default, Clone, Copy)]
pub struct PythonBackend;

impl HelperBackend for PythonBackend {
    fn call(&self, method: &str, params: Value, timeout_secs: u64) -> Result<Value, String> {
        PythonHelper::shared()
            .call(
                method,
                params,
                Duration::from_secs(timeout_secs),
                Duration::from_secs(5),
            )
            .map_err(|e| e.0)
    }
}

fn real_backend() -> Arc<dyn HelperBackend> {
    static BACKEND: OnceLock<Arc<dyn HelperBackend>> = OnceLock::new();
    BACKEND.get_or_init(|| Arc::new(PythonBackend)).clone()
}

// ---------------------------------------------------------------------------
// Base64 (compare.decode / compare.encode transport) — std-only.
// ---------------------------------------------------------------------------

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn b64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64[(n >> 18) as usize & 0x3f] as char);
        out.push(B64[(n >> 12) as usize & 0x3f] as char);
        out.push(if chunk.len() > 1 { B64[(n >> 6) as usize & 0x3f] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64[n as usize & 0x3f] as char } else { '=' });
    }
    out
}

fn b64_val(c: u8) -> Option<u32> {
    match c {
        b'A'..=b'Z' => Some((c - b'A') as u32),
        b'a'..=b'z' => Some((c - b'a' + 26) as u32),
        b'0'..=b'9' => Some((c - b'0' + 52) as u32),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let bytes: Vec<u8> = s.bytes().filter(|c| !c.is_ascii_whitespace()).collect();
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.chunks(4) {
        if chunk.len() < 2 {
            return None;
        }
        let mut n = 0u32;
        let mut pad = 0;
        for &c in chunk {
            if c == b'=' {
                pad += 1;
                n <<= 6;
            } else {
                n = (n << 6) | b64_val(c)?;
            }
        }
        match chunk.len() {
            4 => {
                out.push((n >> 16) as u8);
                if pad < 2 {
                    out.push((n >> 8) as u8);
                }
                if pad < 1 {
                    out.push(n as u8);
                }
            }
            3 => {
                out.push((n >> 10) as u8);
                out.push((n >> 2) as u8);
            }
            2 => out.push((n >> 4) as u8),
            _ => return None,
        }
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// CompareText.swift
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum CompareSide {
    Left,
    Right,
}

impl CompareSide {
    pub fn other(self) -> Self {
        match self {
            CompareSide::Left => CompareSide::Right,
            CompareSide::Right => CompareSide::Left,
        }
    }
    pub fn raw(self) -> &'static str {
        match self {
            CompareSide::Left => "left",
            CompareSide::Right => "right",
        }
    }
    pub fn from_raw(s: &str) -> Option<Self> {
        match s {
            "left" => Some(CompareSide::Left),
            "right" => Some(CompareSide::Right),
            _ => None,
        }
    }
    pub fn index(self) -> usize {
        match self {
            CompareSide::Left => 0,
            CompareSide::Right => 1,
        }
    }
}

/// The wire codes python emits (`compare.decode` / snapshots): `utf8`,
/// `utf8BOM`, `utf16LE`, `utf16BE`, `latin1`. (Swift's rawValues are display
/// labels, so its `init(json:)` silently collapsed every side to UTF-8; the
/// port keeps the engine's codes.)
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum TextEncodingKind {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
    Latin1,
}

impl TextEncodingKind {
    pub fn wire(self) -> &'static str {
        match self {
            TextEncodingKind::Utf8 => "utf8",
            TextEncodingKind::Utf8Bom => "utf8BOM",
            TextEncodingKind::Utf16Le => "utf16LE",
            TextEncodingKind::Utf16Be => "utf16BE",
            TextEncodingKind::Latin1 => "latin1",
        }
    }
    pub fn from_wire(s: &str) -> Option<Self> {
        Some(match s {
            "utf8" => TextEncodingKind::Utf8,
            "utf8BOM" => TextEncodingKind::Utf8Bom,
            "utf16LE" => TextEncodingKind::Utf16Le,
            "utf16BE" => TextEncodingKind::Utf16Be,
            "latin1" => TextEncodingKind::Latin1,
            _ => return None,
        })
    }
    pub fn label(self) -> &'static str {
        match self {
            TextEncodingKind::Utf8 => "UTF-8",
            TextEncodingKind::Utf8Bom => "UTF-8 BOM",
            TextEncodingKind::Utf16Le => "UTF-16 LE",
            TextEncodingKind::Utf16Be => "UTF-16 BE",
            TextEncodingKind::Latin1 => "Latin-1",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize, Deserialize)]
#[repr(u8)]
pub enum Eol {
    None = 0,
    Lf = 1,
    Crlf = 2,
    Cr = 3,
}

impl Eol {
    pub fn from_code(v: i64) -> Eol {
        match v {
            0 => Eol::None,
            1 => Eol::Lf,
            2 => Eol::Crlf,
            3 => Eol::Cr,
            _ => Eol::Lf,
        }
    }
    pub fn from_u8(v: u8) -> Eol {
        Self::from_code(v as i64)
    }
    pub fn code(self) -> i64 {
        self as u8 as i64
    }
    pub fn label(self) -> &'static str {
        match self {
            Eol::None => "none",
            Eol::Lf => "LF",
            Eol::Crlf => "CRLF",
            Eol::Cr => "CR",
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Eol::None => "",
            Eol::Lf => "\n",
            Eol::Crlf => "\r\n",
            Eol::Cr => "\r",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Importance {
    pub leading_ws: bool,
    pub trailing_ws: bool,
    pub embedded_ws: bool,
    pub ignore_case: bool,
    pub line_endings: bool,
    pub blank_lines: bool,
}

impl Default for Importance {
    fn default() -> Self {
        Importance {
            leading_ws: true,
            trailing_ws: true,
            embedded_ws: false,
            ignore_case: false,
            line_endings: true,
            blank_lines: false,
        }
    }
}

impl Importance {
    pub fn new(
        leading_ws: bool,
        trailing_ws: bool,
        embedded_ws: bool,
        ignore_case: bool,
        line_endings: bool,
        blank_lines: bool,
    ) -> Self {
        Importance { leading_ws, trailing_ws, embedded_ws, ignore_case, line_endings, blank_lines }
    }

    pub fn exact() -> Self {
        Importance {
            leading_ws: false,
            trailing_ws: false,
            embedded_ws: false,
            ignore_case: false,
            line_endings: false,
            blank_lines: false,
        }
    }

    pub fn json(&self) -> Value {
        json!({
            "leadingWS": self.leading_ws,
            "trailingWS": self.trailing_ws,
            "embeddedWS": self.embedded_ws,
            "ignoreCase": self.ignore_case,
            "lineEndings": self.line_endings,
            "blankLines": self.blank_lines,
        })
    }

    pub fn from_json(v: &Value) -> Self {
        let g = |k: &str, d: bool| v.get(k).and_then(Value::as_bool).unwrap_or(d);
        Importance {
            leading_ws: g("leadingWS", true),
            trailing_ws: g("trailingWS", true),
            embedded_ws: g("embeddedWS", false),
            ignore_case: g("ignoreCase", false),
            line_endings: g("lineEndings", true),
            blank_lines: g("blankLines", false),
        }
    }

    pub fn cache_key(&self) -> String {
        format!(
            "{}{}{}{}{}{}",
            self.leading_ws as u8,
            self.trailing_ws as u8,
            self.embedded_ws as u8,
            self.ignore_case as u8,
            self.line_endings as u8,
            self.blank_lines as u8
        )
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TextSide {
    pub lines: Vec<String>,
    pub eols: Vec<Eol>,
    pub encoding: TextEncodingKind,
}

impl Default for TextSide {
    fn default() -> Self {
        TextSide { lines: Vec::new(), eols: Vec::new(), encoding: TextEncodingKind::Utf8 }
    }
}

impl TextSide {
    pub fn from_json(j: &Value) -> Self {
        let lines = j
            .get("lines")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default();
        let eols = j
            .get("eols")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(|e| Eol::from_code(e.as_i64().unwrap_or(1))).collect())
            .unwrap_or_default();
        let encoding = j
            .get("encoding")
            .and_then(Value::as_str)
            .and_then(TextEncodingKind::from_wire)
            .unwrap_or(TextEncodingKind::Utf8);
        TextSide { lines, eols, encoding }
    }

    pub fn json(&self) -> Value {
        json!({
            "lines": self.lines,
            "eols": self.eols.iter().map(|e| e.code()).collect::<Vec<_>>(),
            "encoding": self.encoding.wire(),
        })
    }

    pub fn text(&self) -> String {
        let mut out = String::with_capacity(self.lines.iter().map(|l| l.len() + 2).sum());
        for (i, line) in self.lines.iter().enumerate() {
            out.push_str(line);
            out.push_str(self.eols.get(i).copied().unwrap_or(Eol::None).as_str());
        }
        out
    }

    pub fn eol_label(&self) -> String {
        let kinds: BTreeSet<u8> =
            self.eols.iter().filter(|e| **e != Eol::None).map(|e| *e as u8).collect();
        if kinds.len() > 1 {
            return "mixed".to_string();
        }
        if let Some(k) = kinds.iter().next() {
            return Eol::from_u8(*k).label().to_string();
        }
        let mut counts = [0usize; 4];
        for e in self.eols.iter().take(5000) {
            counts[*e as usize] += 1;
        }
        let mut best = 1usize;
        for i in 1..4 {
            if counts[i] > counts[best] {
                best = i;
            }
        }
        let e = if counts[best] == 0 { Eol::Lf } else { Eol::from_u8(best as u8) };
        e.label().to_string()
    }

    /// `compare.decode` — `None` when the bytes are binary. Uses the worker.
    pub fn decode(backend: &dyn HelperBackend, data: &[u8]) -> Option<TextSide> {
        let boxed = backend
            .call("compare.decode", json!({"data": b64_encode(data)}), 60)
            .ok()?;
        if boxed.get("binary").and_then(Value::as_bool) == Some(true) {
            return None;
        }
        Some(TextSide::from_json(boxed.get("side")?))
    }

    /// `compare.encode` — byte-exact in the side's own encoding, `None` when
    /// Latin-1 cannot hold an edit.
    pub fn encoded(&self, backend: &dyn HelperBackend) -> Option<Vec<u8>> {
        let boxed = backend
            .call("compare.encode", json!({"side": self.json()}), 60)
            .ok()?;
        match boxed.get("data") {
            Some(Value::String(s)) => b64_decode(s),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[repr(u8)]
pub enum RowKind {
    Same = 0,
    Changed = 1,
    LeftOnly = 2,
    RightOnly = 3,
}

impl RowKind {
    pub fn from_code(v: u8) -> RowKind {
        match v & 3 {
            1 => RowKind::Changed,
            2 => RowKind::LeftOnly,
            3 => RowKind::RightOnly,
            _ => RowKind::Same,
        }
    }
    pub fn raw(self) -> &'static str {
        match self {
            RowKind::Same => "same",
            RowKind::Changed => "changed",
            RowKind::LeftOnly => "leftOnly",
            RowKind::RightOnly => "rightOnly",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CompareRow {
    pub l: i32,
    pub r: i32,
    pub kind: RowKind,
    pub important: bool,
}

impl CompareRow {
    pub fn line(&self, side: CompareSide) -> i32 {
        match side {
            CompareSide::Left => self.l,
            CompareSide::Right => self.r,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CompareSection {
    pub rows: std::ops::Range<usize>,
    pub important: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum CompareFilter {
    All,
    Diffs,
    Same,
    Context,
}

impl CompareFilter {
    pub const ALL: [CompareFilter; 4] =
        [CompareFilter::All, CompareFilter::Diffs, CompareFilter::Same, CompareFilter::Context];

    pub fn raw(self) -> &'static str {
        match self {
            CompareFilter::All => "all",
            CompareFilter::Diffs => "diffs",
            CompareFilter::Same => "same",
            CompareFilter::Context => "context",
        }
    }
    pub fn from_raw(s: &str) -> Option<Self> {
        Some(match s {
            "all" => CompareFilter::All,
            "diffs" => CompareFilter::Diffs,
            "same" => CompareFilter::Same,
            "context" => CompareFilter::Context,
            _ => return None,
        })
    }
    pub fn title(self) -> &'static str {
        match self {
            CompareFilter::All => "All",
            CompareFilter::Diffs => "Diffs",
            CompareFilter::Same => "Same",
            CompareFilter::Context => "Context",
        }
    }
}

/// `TextCompare` — a thin mirror over the python model handle.
#[derive(Clone)]
pub struct TextCompare {
    pub left: TextSide,
    pub right: TextSide,
    pub importance: Importance,
    pub ignore_unimportant: bool,
    rows: Vec<CompareRow>,
    sections: Vec<CompareSection>,
    anchors: Vec<(usize, usize)>,
    undo_counts: [usize; 2],
    redo_counts: [usize; 2],
    handle: i64,
    backend: Arc<dyn HelperBackend>,
}

impl Debug for TextCompare {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextCompare")
            .field("handle", &self.handle)
            .field("rows", &self.rows.len())
            .field("sections", &self.sections.len())
            .finish()
    }
}

impl Default for TextCompare {
    fn default() -> Self {
        TextCompare {
            left: TextSide::default(),
            right: TextSide::default(),
            importance: Importance::default(),
            ignore_unimportant: false,
            rows: Vec::new(),
            sections: Vec::new(),
            anchors: Vec::new(),
            undo_counts: [0, 0],
            redo_counts: [0, 0],
            handle: -1,
            backend: real_backend(),
        }
    }
}

impl TextCompare {
    /// `init(left:right:importance:ignoreUnimportant:)` — creates the handle
    /// immediately through the real worker.
    pub fn new(
        left: TextSide,
        right: TextSide,
        importance: Importance,
        ignore_unimportant: bool,
    ) -> Self {
        Self::with_backend(real_backend(), left, right, importance, ignore_unimportant)
    }

    pub fn with_backend(
        backend: Arc<dyn HelperBackend>,
        left: TextSide,
        right: TextSide,
        importance: Importance,
        ignore_unimportant: bool,
    ) -> Self {
        let mut t = TextCompare {
            left,
            right,
            importance,
            ignore_unimportant,
            rows: Vec::new(),
            sections: Vec::new(),
            anchors: Vec::new(),
            undo_counts: [0, 0],
            redo_counts: [0, 0],
            handle: -1,
            backend,
        };
        t.ensure_handle();
        t
    }

    pub fn backend(&self) -> &dyn HelperBackend {
        self.backend.as_ref()
    }

    pub fn handle(&self) -> i64 {
        self.handle
    }
    pub fn rows(&self) -> &[CompareRow] {
        &self.rows
    }
    pub fn sections(&self) -> &[CompareSection] {
        &self.sections
    }
    pub fn anchors(&self) -> &[(usize, usize)] {
        &self.anchors
    }

    pub fn side(&self, s: CompareSide) -> &TextSide {
        match s {
            CompareSide::Left => &self.left,
            CompareSide::Right => &self.right,
        }
    }

    fn ensure_handle(&mut self) {
        if self.handle >= 0 {
            return;
        }
        let params = json!({
            "left": self.left.json(),
            "right": self.right.json(),
            "importance": self.importance.json(),
            "ignoreUnimportant": self.ignore_unimportant,
        });
        if let Ok(snap) = self.backend.call("compare.new", params, 120) {
            self.apply_snapshot(&snap);
        }
    }

    fn call(&mut self, method: &str, params: Value) -> bool {
        self.ensure_handle();
        if self.handle < 0 {
            return false;
        }
        let mut obj = params;
        if !obj.is_object() {
            obj = json!({});
        }
        obj.as_object_mut().unwrap().insert("handle".to_string(), json!(self.handle));
        match self.backend.call(method, obj, 120) {
            Ok(snap) => {
                let changed = snap.get("changed").and_then(Value::as_bool).unwrap_or(true);
                self.apply_snapshot(&snap);
                changed
            }
            Err(_) => false,
        }
    }

    fn apply_snapshot(&mut self, snap: &Value) {
        if let Some(h) = snap.get("handle").and_then(Value::as_i64) {
            self.handle = h;
        }
        if let Some(j) = snap.get("left") {
            self.left = TextSide::from_json(j);
        }
        if let Some(j) = snap.get("right") {
            self.right = TextSide::from_json(j);
        }
        if let Some(rows) = snap.get("rows").and_then(Value::as_array) {
            self.rows = rows
                .iter()
                .filter_map(|r| {
                    let r = r.as_array()?;
                    if r.len() < 3 {
                        return None;
                    }
                    let l = r[0].as_i64()? as i32;
                    let rr = r[1].as_i64()? as i32;
                    let flags = r[2].as_i64()? as u8;
                    Some(CompareRow {
                        l,
                        r: rr,
                        kind: RowKind::from_code(flags),
                        important: flags & 4 != 0,
                    })
                })
                .collect();
        }
        if let Some(secs) = snap.get("sections").and_then(Value::as_array) {
            self.sections = secs
                .iter()
                .filter_map(|s| {
                    let s = s.as_array()?;
                    if s.len() < 3 {
                        return None;
                    }
                    let lo = s[0].as_i64()? as usize;
                    let hi = s[1].as_i64()? as usize;
                    Some(CompareSection { rows: lo..hi, important: s[2].as_i64()? != 0 })
                })
                .collect();
        }
        if let Some(anchors) = snap.get("anchors").and_then(Value::as_array) {
            self.anchors = anchors
                .iter()
                .filter_map(|a| {
                    let a = a.as_array()?;
                    if a.len() != 2 {
                        return None;
                    }
                    Some((a[0].as_i64()? as usize, a[1].as_i64()? as usize))
                })
                .collect();
        }
        if let Some(u) = snap.get("undo").and_then(Value::as_array) {
            if u.len() == 2 {
                self.undo_counts =
                    [u[0].as_i64().unwrap_or(0) as usize, u[1].as_i64().unwrap_or(0) as usize];
            }
        }
        if let Some(r) = snap.get("redo").and_then(Value::as_array) {
            if r.len() == 2 {
                self.redo_counts =
                    [r[0].as_i64().unwrap_or(0) as usize, r[1].as_i64().unwrap_or(0) as usize];
            }
        }
        if let Some(j) = snap.get("importance") {
            if j.is_object() {
                self.importance = Importance::from_json(j);
            }
        }
        if let Some(on) = snap.get("ignoreUnimportant").and_then(Value::as_bool) {
            self.ignore_unimportant = on;
        }
    }

    pub fn recompute(&mut self) {
        self.call("compare.recompute", json!({}));
    }

    pub fn replace(&mut self, side: CompareSide, range: std::ops::Range<usize>, lines: &[String]) {
        self.call(
            "compare.replace",
            json!({
                "side": side.raw(),
                "start": range.start,
                "count": range.len(),
                "lines": lines,
            }),
        );
    }

    pub fn replace_with_eols(
        &mut self,
        side: CompareSide,
        range: std::ops::Range<usize>,
        lines: &[String],
        eols: &[Eol],
    ) {
        self.call(
            "compare.replace",
            json!({
                "side": side.raw(),
                "start": range.start,
                "count": range.len(),
                "lines": lines,
                "eols": eols.iter().map(|e| e.code()).collect::<Vec<_>>(),
            }),
        );
    }

    pub fn undo_count(&self, s: CompareSide) -> usize {
        self.undo_counts[s.index()]
    }
    pub fn has_undo(&self, s: CompareSide) -> bool {
        self.undo_count(s) > 0
    }
    pub fn can_undo(&self) -> bool {
        self.undo_counts[0] + self.undo_counts[1] > 0
    }
    pub fn can_redo(&self) -> bool {
        self.redo_counts[0] + self.redo_counts[1] > 0
    }

    pub fn undo(&mut self, side: Option<CompareSide>) -> bool {
        if let Some(s) = side {
            if !self.has_undo(s) {
                return false;
            }
        }
        if !self.can_undo() {
            return false;
        }
        let params = match side {
            Some(s) => json!({"side": s.raw()}),
            None => json!({}),
        };
        self.call("compare.undo", params);
        true
    }

    pub fn redo(&mut self) -> bool {
        if !self.can_redo() {
            return false;
        }
        self.call("compare.redo", json!({}));
        true
    }

    pub fn copy_rows(&mut self, range: std::ops::Range<usize>, from: CompareSide) -> bool {
        self.call("compare.copy_rows", json!({"lo": range.start, "hi": range.end, "from": from.raw()}))
    }

    pub fn copy_section(&mut self, index: usize, from: CompareSide) -> bool {
        self.call("compare.copy_section", json!({"index": index, "from": from.raw()}))
    }

    pub fn align(&mut self, left: usize, right: usize) {
        self.call("compare.align", json!({"l": left, "r": right}));
    }

    pub fn clear_alignment(&mut self, row: Option<usize>) {
        let params = match row {
            Some(r) => json!({"row": r}),
            None => json!({}),
        };
        self.call("compare.clear_alignment", params);
    }

    pub fn set_importance(&mut self, imp: Importance) {
        self.importance = imp;
        self.call("compare.set_importance", json!({"importance": imp.json()}));
    }

    pub fn set_ignore_unimportant(&mut self, on: bool) {
        self.ignore_unimportant = on;
        self.call("compare.set_ignore_unimportant", json!({"on": on}));
    }

    pub fn swap_sides(&mut self) {
        self.call("compare.swap_sides", json!({}));
    }

    pub fn set_side(&mut self, s: CompareSide, t: TextSide) {
        self.call("compare.set_side", json!({"side": s.raw(), "text": t.json()}));
    }

    pub fn trim_trailing_whitespace(&mut self, s: CompareSide) -> usize {
        self.ensure_handle();
        if self.handle < 0 {
            return 0;
        }
        match self.backend.call("compare.trim", json!({"handle": self.handle, "side": s.raw()}), 120) {
            Ok(snap) => {
                let n = snap.get("count").and_then(Value::as_i64).unwrap_or(0) as usize;
                self.apply_snapshot(&snap);
                n
            }
            Err(_) => 0,
        }
    }

    pub fn convert_line_endings(&mut self, s: CompareSide, to: Eol) -> usize {
        self.ensure_handle();
        if self.handle < 0 || to == Eol::None {
            return 0;
        }
        match self.backend.call(
            "compare.convert",
            json!({"handle": self.handle, "side": s.raw(), "eol": to.code()}),
            120,
        ) {
            Ok(snap) => {
                let n = snap.get("count").and_then(Value::as_i64).unwrap_or(0) as usize;
                self.apply_snapshot(&snap);
                n
            }
            Err(_) => 0,
        }
    }

    pub fn is_diff(&self, r: &CompareRow) -> bool {
        is_diff_row(r, self.ignore_unimportant)
    }

    pub fn important_count(&self) -> usize {
        self.sections.iter().filter(|s| s.important).count()
    }
    pub fn unimportant_count(&self) -> usize {
        self.sections.len() - self.important_count()
    }
    pub fn identical_text(&self) -> bool {
        self.rows.iter().all(|r| r.kind == RowKind::Same)
    }

    pub fn section_at(&self, row: usize) -> Option<usize> {
        if self.sections.is_empty() {
            return None;
        }
        let (mut lo, mut hi) = (0i64, self.sections.len() as i64 - 1);
        while lo <= hi {
            let mid = (lo + hi) / 2;
            let s = &self.sections[mid as usize].rows;
            if row < s.start {
                hi = mid - 1;
            } else if row >= s.end {
                lo = mid + 1;
            } else {
                return Some(mid as usize);
            }
        }
        None
    }

    pub fn next_section(&self, after: usize) -> Option<usize> {
        self.sections.iter().position(|s| s.rows.start > after)
    }

    pub fn prev_section(&self, before: usize) -> Option<usize> {
        self.sections.iter().rposition(|s| s.rows.start < before)
    }

    pub fn line_index(&self, side: CompareSide, at_row: usize) -> usize {
        let mut i = at_row;
        while i < self.rows.len() {
            let v = self.rows[i].line(side);
            if v >= 0 {
                return v as usize;
            }
            i += 1;
        }
        self.side(side).lines.len()
    }

    pub fn line_range(
        &self,
        side: CompareSide,
        rows: std::ops::Range<usize>,
    ) -> std::ops::Range<usize> {
        let start = self.line_index(side, rows.start);
        let end = if rows.end >= self.rows.len() {
            self.side(side).lines.len()
        } else {
            self.line_index(side, rows.end)
        };
        start..start.max(end)
    }

    pub fn visible_rows(&self, f: CompareFilter, context: usize) -> Option<Vec<usize>> {
        match f {
            CompareFilter::All => None,
            CompareFilter::Diffs => Some(
                self.rows.iter().enumerate().filter(|(_, r)| self.is_diff(r)).map(|(i, _)| i).collect(),
            ),
            CompareFilter::Same => Some(
                self.rows.iter().enumerate().filter(|(_, r)| !self.is_diff(r)).map(|(i, _)| i).collect(),
            ),
            CompareFilter::Context => {
                let mut keep = vec![false; self.rows.len()];
                for s in &self.sections {
                    let lo = s.rows.start.saturating_sub(context);
                    let hi = (s.rows.end + context).min(self.rows.len());
                    for k in keep.iter_mut().take(hi).skip(lo) {
                        *k = true;
                    }
                }
                Some(keep.iter().enumerate().filter(|(_, k)| **k).map(|(i, _)| i).collect())
            }
        }
    }

    pub fn is_anchor(&self, row: usize) -> bool {
        if row >= self.rows.len() {
            return false;
        }
        let rr = self.rows[row];
        self.anchors.iter().any(|(l, r)| *l == rr.l as usize && *r == rr.r as usize)
    }
}

/// `ByteCountFormatter.string(fromByteCount:countStyle: .file)` — decimal
/// units: "Zero KB", "1 byte", "999 bytes", "12 KB", "1.5 MB", "1.25 GB".
pub fn byte_count_file(n: u64) -> String {
    match n {
        0 => "Zero KB".to_string(),
        1 => "1 byte".to_string(),
        2..=999 => format!("{n} bytes"),
        1_000..=999_499 => format!("{} KB", ((n as f64) / 1e3).round() as u64),
        _ if n < 999_950_000 => trim_zero(format!("{:.1}", n as f64 / 1e6)) + " MB",
        _ if n < 999_995_000_000 => trim_zero(format!("{:.2}", n as f64 / 1e9)) + " GB",
        _ => trim_zero(format!("{:.2}", n as f64 / 1e12)) + " TB",
    }
}

/// "1.0" → "1", "1.50" → "1.5" (ByteCountFormatter drops trailing zeros).
fn trim_zero(s: String) -> String {
    if !s.contains('.') {
        return s;
    }
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// `BinaryCompare` — a 10-line byte compare on in-memory `Data` (no helper hop).
pub struct BinaryCompare;

impl BinaryCompare {
    /// First differing byte index. `None` when equal; `Some(min_len)` when one
    /// is a prefix of the other (mirrors `compare_text.binary_first_difference`).
    pub fn first_difference(a: &[u8], b: &[u8]) -> Option<usize> {
        let n = a.len().min(b.len());
        for i in 0..n {
            if a[i] != b[i] {
                return Some(i);
            }
        }
        if a.len() == b.len() {
            None
        } else {
            Some(n)
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CharOpKind {
    Same,
    Del,
    Ins,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CharOp {
    pub kind: CharOpKind,
    pub text: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Mark {
    pub location: usize,
    pub length: usize,
    pub important: bool,
}

/// `CharDiff` — helper-backed char marks / ops (used by drawing).
pub struct CharDiff;

impl CharDiff {
    pub fn marks(
        backend: &dyn HelperBackend,
        a: &str,
        b: &str,
        imp: &Importance,
    ) -> (Vec<Mark>, Vec<Mark>) {
        let Ok(boxed) =
            backend.call("compare.marks", json!({"a": a, "b": b, "importance": imp.json()}), 30)
        else {
            return (Vec::new(), Vec::new());
        };
        (parse_marks(boxed.get("left")), parse_marks(boxed.get("right")))
    }

    pub fn diff(backend: &dyn HelperBackend, a: &str, b: &str) -> Vec<CharOp> {
        let Ok(boxed) = backend.call("compare.char_diff", json!({"a": a, "b": b}), 30) else {
            return Vec::new();
        };
        boxed
            .get("ops")
            .and_then(Value::as_array)
            .map(|ops| {
                ops.iter()
                    .filter_map(|o| {
                        let o = o.as_array()?;
                        if o.len() != 2 {
                            return None;
                        }
                        let kind = match o[0].as_i64()? {
                            0 => CharOpKind::Same,
                            1 => CharOpKind::Del,
                            _ => CharOpKind::Ins,
                        };
                        Some(CharOp { kind, text: o[1].as_str()?.to_string() })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn changes(ops: &[CharOp]) -> usize {
        let mut n = 0;
        let mut in_change = false;
        for o in ops {
            if o.kind == CharOpKind::Same {
                in_change = false;
            } else if !in_change {
                n += 1;
                in_change = true;
            }
        }
        n
    }
}

fn parse_marks(v: Option<&Value>) -> Vec<Mark> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|m| {
                    let loc = m.get("location").and_then(Value::as_i64)?;
                    let len = m.get("length").and_then(Value::as_i64)?;
                    Some(Mark {
                        location: loc as usize,
                        length: len as usize,
                        important: m.get("important").and_then(Value::as_bool).unwrap_or(false),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// CompareFolder.swift
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum FolderStatus {
    Same,
    Different,
    Unimportant,
    LeftOnly,
    RightOnly,
    Unknown,
    Error,
}

impl FolderStatus {
    pub fn raw(self) -> &'static str {
        match self {
            FolderStatus::Same => "same",
            FolderStatus::Different => "different",
            FolderStatus::Unimportant => "unimportant",
            FolderStatus::LeftOnly => "leftOnly",
            FolderStatus::RightOnly => "rightOnly",
            FolderStatus::Unknown => "unknown",
            FolderStatus::Error => "error",
        }
    }
    pub fn from_raw(s: &str) -> Self {
        match s {
            "same" => FolderStatus::Same,
            "different" => FolderStatus::Different,
            "unimportant" => FolderStatus::Unimportant,
            "leftOnly" => FolderStatus::LeftOnly,
            "rightOnly" => FolderStatus::RightOnly,
            "unknown" => FolderStatus::Unknown,
            "error" => FolderStatus::Error,
            _ => FolderStatus::Same,
        }
    }
    pub fn is_orphan(self) -> bool {
        matches!(self, FolderStatus::LeftOnly | FolderStatus::RightOnly)
    }
    pub fn is_diff(self) -> bool {
        self != FolderStatus::Same
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum FolderNewer {
    None,
    Left,
    Right,
}

impl FolderNewer {
    pub fn raw(self) -> &'static str {
        match self {
            FolderNewer::None => "none",
            FolderNewer::Left => "left",
            FolderNewer::Right => "right",
        }
    }
    pub fn from_raw(s: &str) -> Self {
        match s {
            "left" => FolderNewer::Left,
            "right" => FolderNewer::Right,
            _ => FolderNewer::None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum FolderFilter {
    All,
    Diffs,
    Same,
    Orphans,
    LeftNewer,
    RightNewer,
}

impl FolderFilter {
    pub const ALL: [FolderFilter; 6] = [
        FolderFilter::All,
        FolderFilter::Diffs,
        FolderFilter::Same,
        FolderFilter::Orphans,
        FolderFilter::LeftNewer,
        FolderFilter::RightNewer,
    ];

    pub fn raw(self) -> &'static str {
        match self {
            FolderFilter::All => "all",
            FolderFilter::Diffs => "diffs",
            FolderFilter::Same => "same",
            FolderFilter::Orphans => "orphans",
            FolderFilter::LeftNewer => "leftNewer",
            FolderFilter::RightNewer => "rightNewer",
        }
    }
    pub fn from_raw(s: &str) -> Option<Self> {
        Some(match s {
            "all" => FolderFilter::All,
            "diffs" => FolderFilter::Diffs,
            "same" => FolderFilter::Same,
            "orphans" => FolderFilter::Orphans,
            "leftNewer" => FolderFilter::LeftNewer,
            "rightNewer" => FolderFilter::RightNewer,
            _ => return None,
        })
    }
    pub fn title(self) -> &'static str {
        match self {
            FolderFilter::All => "All",
            FolderFilter::Diffs => "Differences",
            FolderFilter::Same => "Same",
            FolderFilter::Orphans => "Orphans",
            FolderFilter::LeftNewer => "Left newer",
            FolderFilter::RightNewer => "Right newer",
        }
    }
}

impl Default for FolderFilter {
    fn default() -> Self {
        FolderFilter::All
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct FolderSideInfo {
    pub name: String,
    pub is_dir: bool,
    pub is_link: bool,
    pub link: Option<String>,
    pub size: i64,
    pub mtime: f64,
    pub target_size: Option<i64>,
    pub target_mtime: f64,
}

impl Default for FolderSideInfo {
    fn default() -> Self {
        FolderSideInfo {
            name: String::new(),
            is_dir: false,
            is_link: false,
            link: None,
            size: 0,
            mtime: 0.0,
            target_size: None,
            target_mtime: 0.0,
        }
    }
}

impl FolderSideInfo {
    pub fn from_json(j: &Value) -> Self {
        FolderSideInfo {
            name: j.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
            is_dir: j.get("isDir").and_then(Value::as_bool).unwrap_or(false),
            is_link: j.get("isLink").and_then(Value::as_bool).unwrap_or(false),
            link: j.get("link").and_then(Value::as_str).map(str::to_string),
            size: j.get("size").and_then(Value::as_i64).unwrap_or(0),
            mtime: j.get("mtime").and_then(Value::as_f64).unwrap_or(0.0),
            target_size: j.get("targetSize").and_then(Value::as_i64),
            target_mtime: j.get("targetMtime").and_then(Value::as_f64).unwrap_or(0.0),
        }
    }

    pub fn json(&self) -> Value {
        json!({
            "name": self.name,
            "isDir": self.is_dir,
            "isLink": self.is_link,
            "link": self.link,
            "size": self.size,
            "mtime": self.mtime,
            "targetSize": self.target_size,
            "targetMtime": self.target_mtime,
        })
    }

    /// A regular file's (size, mtime); a link compares by its target.
    pub fn as_file(&self) -> Option<(i64, f64)> {
        if self.is_link {
            return self.target_size.map(|s| (s, self.target_mtime));
        }
        if self.is_dir {
            None
        } else {
            Some((self.size, self.mtime))
        }
    }
}

#[derive(Clone, Debug)]
pub struct FolderNode {
    pub id: usize,
    pub key: String,
    pub rel: String,
    pub name: String,
    pub left: Option<FolderSideInfo>,
    pub right: Option<FolderSideInfo>,
    pub status: FolderStatus,
    pub same_by_metadata: bool,
    pub newer: FolderNewer,
    pub children: Vec<usize>,
    pub parent: Option<usize>,
    pub expanded: bool,
    pub depth: usize,
    pub diff_below: i64,
    pub important_below: i64,
    pub unknown_below: i64,
}

impl FolderNode {
    pub fn new(id: usize, key: &str, rel: &str, name: &str) -> Self {
        FolderNode {
            id,
            key: key.to_string(),
            rel: rel.to_string(),
            name: name.to_string(),
            left: None,
            right: None,
            status: FolderStatus::Same,
            same_by_metadata: false,
            newer: FolderNewer::None,
            children: Vec::new(),
            parent: None,
            expanded: false,
            depth: 0,
            diff_below: 0,
            important_below: 0,
            unknown_below: 0,
        }
    }

    pub fn is_dir(&self) -> bool {
        self.left.as_ref().map(|l| l.is_dir).unwrap_or(false)
            || self.right.as_ref().map(|r| r.is_dir).unwrap_or(false)
    }

    pub fn kind_mismatch(&self) -> bool {
        match (&self.left, &self.right) {
            (Some(l), Some(r)) => l.is_dir != r.is_dir,
            _ => false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct FolderOptions {
    pub time_tolerance: f64,
    pub content: String,
    pub hidden: bool,
    pub exclude: Vec<String>,
    pub importance: Importance,
    pub use_gitignore: bool,
    pub ignore_file: String,
    pub recheck: f64,
}

impl Default for FolderOptions {
    fn default() -> Self {
        FolderOptions {
            time_tolerance: 2.0,
            content: "auto".to_string(),
            hidden: true,
            exclude: Vec::new(),
            importance: Importance::default(),
            use_gitignore: false,
            ignore_file: String::new(),
            recheck: 30.0,
        }
    }
}

impl FolderOptions {
    pub fn json(&self) -> Value {
        json!({
            "timeTolerance": self.time_tolerance,
            "content": self.content,
            "hidden": self.hidden,
            "exclude": self.exclude,
            "useGitignore": self.use_gitignore,
            "ignoreFile": self.ignore_file,
            "recheck": self.recheck,
        })
    }
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct FolderTreeCounts {
    pub different: i64,
    pub unimportant: i64,
    pub left_only: i64,
    pub right_only: i64,
    pub same: i64,
    pub same_by_metadata: i64,
    pub unknown: i64,
    pub error: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FolderRow {
    pub id: usize,
    pub depth: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct FolderView {
    pub filter: FolderFilter,
    pub flatten: bool,
    pub name_filter: String,
}

/// `FolderTree` — a mirror over the python tree handle (nodes stored by id).
pub struct FolderTree {
    pub left_root: String,
    pub right_root: String,
    pub handle: i64,
    pub roots: Vec<usize>,
    nodes: Vec<FolderNode>,
    pub case_insensitive: bool,
    pub truncated: bool,
    pub errors: Vec<String>,
    backend: Arc<dyn HelperBackend>,
}

impl Debug for FolderTree {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FolderTree")
            .field("handle", &self.handle)
            .field("nodes", &self.nodes.len())
            .field("caseInsensitive", &self.case_insensitive)
            .finish()
    }
}

impl FolderTree {
    pub fn new(backend: Arc<dyn HelperBackend>, left: &str, right: &str, handle: i64) -> Self {
        FolderTree {
            left_root: left.to_string(),
            right_root: right.to_string(),
            handle,
            roots: Vec::new(),
            nodes: Vec::new(),
            case_insensitive: true,
            truncated: false,
            errors: Vec::new(),
            backend,
        }
    }

    pub fn backend(&self) -> &dyn HelperBackend {
        self.backend.as_ref()
    }

    pub fn handle(&self) -> i64 {
        self.handle
    }

    fn call(&self, method: &str, params: Value) -> Option<Value> {
        self.backend.call(method, params, 600).ok()
    }

    pub fn all(&self) -> &[FolderNode] {
        &self.nodes
    }
    pub fn nodes_iter_mut(&mut self) -> std::slice::IterMut<'_, FolderNode> {
        self.nodes.iter_mut()
    }
    pub fn adopt(&mut self, n: FolderNode) {
        self.nodes.push(n);
    }
    pub fn node(&self, id: usize) -> Option<&FolderNode> {
        self.nodes.get(id).filter(|n| n.id == id)
    }
    pub fn node_mut(&mut self, id: usize) -> Option<&mut FolderNode> {
        self.nodes.get_mut(id).filter(|n| n.id == id)
    }

    pub fn path(&self, id: usize, side: CompareSide) -> String {
        self.call("folder.path", json!({"handle": self.handle, "id": id, "side": side.raw()}))
            .and_then(|b| b.get("path").and_then(Value::as_str).map(str::to_string))
            .unwrap_or_else(|| self.node(id).map(|n| n.rel.clone()).unwrap_or_default())
    }

    pub fn pending(&self) -> Vec<usize> {
        self.call("folder.pending", json!({"handle": self.handle}))
            .and_then(|b| b.get("ids").and_then(Value::as_array).cloned())
            .map(|ids| {
                ids.iter()
                    .filter_map(|v| v.as_i64())
                    .map(|i| i as usize)
                    .filter(|i| self.node(*i).is_some())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn counts(&self) -> FolderTreeCounts {
        let Some(d) = self.call("folder.counts", json!({"handle": self.handle})) else {
            return FolderTreeCounts::default();
        };
        let g = |k: &str| d.get(k).and_then(Value::as_i64).unwrap_or(0);
        FolderTreeCounts {
            different: g("different"),
            unimportant: g("unimportant"),
            left_only: g("leftOnly"),
            right_only: g("rightOnly"),
            same: g("same"),
            same_by_metadata: g("sameByMetadata"),
            unknown: g("unknown"),
            error: g("error"),
        }
    }

    pub fn rows(&self, view: &FolderView) -> Vec<FolderRow> {
        let expanded: Vec<usize> = self.nodes.iter().filter(|n| n.expanded).map(|n| n.id).collect();
        let Some(b) = self.call(
            "folder.rows",
            json!({
                "handle": self.handle,
                "filter": view.filter.raw(),
                "nameFilter": view.name_filter,
                "flatten": view.flatten,
                "expanded": expanded,
            }),
        ) else {
            return Vec::new();
        };
        b.get("rows")
            .and_then(Value::as_array)
            .map(|raw| {
                raw.iter()
                    .filter_map(|r| {
                        let id = r.get("id").and_then(Value::as_i64)? as usize;
                        if self.node(id).is_none() {
                            return None;
                        }
                        Some(FolderRow {
                            id,
                            depth: r.get("depth").and_then(Value::as_i64).unwrap_or(0) as usize,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn expand_all(&mut self, open: bool) {
        for n in self.nodes.iter_mut() {
            if n.is_dir() {
                n.expanded = open;
            }
        }
    }

    pub fn settle(&mut self) {
        let statuses: Vec<Value> = self
            .nodes
            .iter()
            .map(|n| json!([n.id, n.status.raw(), n.same_by_metadata]))
            .collect();
        if let Some(b) =
            self.call("folder.settle", json!({"handle": self.handle, "statuses": statuses}))
        {
            self.apply_statuses(b.get("statuses"));
        }
    }

    pub fn apply_statuses(&mut self, list: Option<&Value>) {
        let Some(entries) = list.and_then(Value::as_array) else { return };
        for entry in entries {
            let Some(e) = entry.as_array() else { continue };
            if e.len() < 4 {
                continue;
            }
            let Some(id) = e[0].as_i64().map(|v| v as usize) else { continue };
            let Some(n) = self.node_mut(id) else { continue };
            n.status = FolderStatus::from_raw(e[1].as_str().unwrap_or("same"));
            n.same_by_metadata = e[2].as_bool().unwrap_or(false);
            n.newer = FolderNewer::from_raw(e[3].as_str().unwrap_or("none"));
        }
    }
}

impl Drop for FolderTree {
    fn drop(&mut self) {
        if self.handle >= 0 {
            let _ = self.backend.call("folder.drop", json!({"handle": self.handle}), 60);
        }
    }
}

pub struct FolderScan;

impl FolderScan {
    /// Swift reads `URLResourceValues.volumeSupportsCaseSensitiveNames`; the
    /// std-only probe asks whether the basename resolves under a case flip.
    pub fn is_case_insensitive(path: &str) -> bool {
        let p = Path::new(path);
        let (Some(parent), Some(name)) = (p.parent(), p.file_name()) else {
            return true;
        };
        let name = name.to_string_lossy().to_string();
        let upper = name.to_ascii_uppercase();
        let lower = name.to_ascii_lowercase();
        if upper == lower {
            return true;
        }
        let flipped = if name == lower { upper } else { lower };
        std::fs::metadata(parent.join(flipped)).is_ok()
    }

    pub fn run(
        backend: Arc<dyn HelperBackend>,
        left: &str,
        right: &str,
        options: &FolderOptions,
    ) -> FolderTree {
        Self::run_with(backend, left, right, options, &mut |_| {}, &|| false)
    }

    pub fn run_with(
        backend: Arc<dyn HelperBackend>,
        left: &str,
        right: &str,
        options: &FolderOptions,
        progress: &mut dyn FnMut(usize),
        cancelled: &dyn Fn() -> bool,
    ) -> FolderTree {
        let ci = Self::is_case_insensitive(left);
        let empty = || FolderTree::new(backend.clone(), left, right, -1);
        let Some(start) = backend
            .call(
                "folder.scan_start",
                json!({"left": left, "right": right, "options": options.json(), "caseInsensitive": ci}),
                600,
            )
            .ok()
        else {
            return empty();
        };
        let Some(session) = start.get("session").and_then(Value::as_i64) else {
            return empty();
        };
        let mut last = 0usize;
        loop {
            if cancelled() {
                break;
            }
            let Ok(step) =
                backend.call("folder.scan_step", json!({"session": session, "maxDirs": 24}), 600)
            else {
                break;
            };
            let count = step.get("count").and_then(Value::as_i64).unwrap_or(0) as usize;
            if count != last {
                last = count;
                progress(count);
            }
            if step.get("done").and_then(Value::as_bool).unwrap_or(false) {
                break;
            }
        }
        let Ok(snap) =
            backend.call("folder.scan_finish", json!({"session": session}), 600)
        else {
            return empty();
        };
        Self::tree_from_snapshot(backend, &snap, left, right)
    }

    pub fn tree_from_snapshot(
        backend: Arc<dyn HelperBackend>,
        snap: &Value,
        left: &str,
        right: &str,
    ) -> FolderTree {
        let Some(handle) = snap.get("handle").and_then(Value::as_i64) else {
            return FolderTree::new(backend, left, right, -1);
        };
        let mut t = FolderTree::new(backend, left, right, handle);
        t.case_insensitive =
            snap.get("caseInsensitive").and_then(Value::as_bool).unwrap_or(true);
        t.truncated = snap.get("truncated").and_then(Value::as_bool).unwrap_or(false);
        t.errors = snap
            .get("errors")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default();
        let Some(nodes_json) = snap.get("nodes").and_then(Value::as_array) else {
            return t;
        };
        let mut by_id: HashSet<usize> = HashSet::new();
        for j in nodes_json {
            let Some(id) = j.get("id").and_then(Value::as_i64) else { continue };
            let id = id as usize;
            let mut n = FolderNode::new(
                id,
                j.get("key").and_then(Value::as_str).unwrap_or(""),
                j.get("rel").and_then(Value::as_str).unwrap_or(""),
                j.get("name").and_then(Value::as_str).unwrap_or(""),
            );
            n.left = j.get("left").filter(|v| !v.is_null()).map(FolderSideInfo::from_json);
            n.right = j.get("right").filter(|v| !v.is_null()).map(FolderSideInfo::from_json);
            n.status =
                FolderStatus::from_raw(j.get("status").and_then(Value::as_str).unwrap_or("same"));
            n.same_by_metadata =
                j.get("sameByMetadata").and_then(Value::as_bool).unwrap_or(false);
            n.newer =
                FolderNewer::from_raw(j.get("newer").and_then(Value::as_str).unwrap_or("none"));
            n.depth = j.get("depth").and_then(Value::as_i64).unwrap_or(0) as usize;
            by_id.insert(id);
            t.adopt(n);
        }
        for j in nodes_json {
            let Some(id) = j.get("id").and_then(Value::as_i64).map(|v| v as usize) else { continue };
            let parent = j.get("parent").and_then(Value::as_i64).map(|v| v as usize);
            let children: Vec<usize> = j
                .get("children")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_i64).map(|v| v as usize).collect())
                .unwrap_or_default();
            if let Some(n) = t.node_mut(id) {
                n.parent = parent.filter(|p| by_id.contains(p));
                n.children = children.into_iter().filter(|c| by_id.contains(c)).collect();
            }
        }
        t.roots = snap
            .get("roots")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_i64)
                    .map(|v| v as usize)
                    .filter(|i| by_id.contains(i))
                    .collect()
            })
            .unwrap_or_default();
        t
    }

    pub fn restat(tree: &mut FolderTree, id: usize, options: &FolderOptions) {
        let Some(boxed) = tree.call(
            "folder.restat",
            json!({"handle": tree.handle, "id": id, "options": options.json()}),
        ) else {
            return;
        };
        let Some(n) = tree.node_mut(id) else { return };
        n.left = boxed.get("left").filter(|v| !v.is_null()).map(FolderSideInfo::from_json);
        n.right = boxed.get("right").filter(|v| !v.is_null()).map(FolderSideInfo::from_json);
        n.status = FolderStatus::from_raw(boxed.get("status").and_then(Value::as_str).unwrap_or("same"));
        n.same_by_metadata =
            boxed.get("sameByMetadata").and_then(Value::as_bool).unwrap_or(false);
        n.newer = FolderNewer::from_raw(boxed.get("newer").and_then(Value::as_str).unwrap_or("none"));
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FolderContentAnswer {
    Same,
    Different,
    Unimportant,
    Error,
}

impl FolderContentAnswer {
    pub fn raw(self) -> &'static str {
        match self {
            FolderContentAnswer::Same => "same",
            FolderContentAnswer::Different => "different",
            FolderContentAnswer::Unimportant => "unimportant",
            FolderContentAnswer::Error => "error",
        }
    }
    pub fn from_raw(s: &str) -> Self {
        match s {
            "same" => FolderContentAnswer::Same,
            "different" => FolderContentAnswer::Different,
            "unimportant" => FolderContentAnswer::Unimportant,
            _ => FolderContentAnswer::Error,
        }
    }
}

pub struct FolderContent;

impl FolderContent {
    pub fn check(
        backend: &dyn HelperBackend,
        left: &str,
        right: &str,
        sizes: (i64, i64),
        imp: &Importance,
    ) -> FolderContentAnswer {
        match backend.call(
            "folder.content_check",
            json!({"left": left, "right": right, "sizes": [sizes.0, sizes.1], "importance": imp.json()}),
            600,
        ) {
            Ok(b) => FolderContentAnswer::from_raw(
                b.get("answer").and_then(Value::as_str).unwrap_or("error"),
            ),
            Err(_) => FolderContentAnswer::Error,
        }
    }

    pub fn apply(a: FolderContentAnswer, n: &mut FolderNode) {
        n.same_by_metadata = false;
        n.status = match a {
            FolderContentAnswer::Same => FolderStatus::Same,
            FolderContentAnswer::Different => FolderStatus::Different,
            FolderContentAnswer::Unimportant => FolderStatus::Unimportant,
            FolderContentAnswer::Error => FolderStatus::Error,
        };
    }

    pub fn rule_candidates(tree: &FolderTree) -> Vec<usize> {
        tree.call("folder.rule_candidates", json!({"handle": tree.handle}))
            .and_then(|b| b.get("ids").and_then(Value::as_array).cloned())
            .map(|ids| {
                ids.iter()
                    .filter_map(Value::as_i64)
                    .map(|i| i as usize)
                    .filter(|i| tree.node(*i).is_some())
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum SyncMode {
    UpdateRight,
    UpdateLeft,
    UpdateBoth,
    MirrorRight,
    MirrorLeft,
}

impl SyncMode {
    pub const ALL: [SyncMode; 5] = [
        SyncMode::UpdateRight,
        SyncMode::UpdateLeft,
        SyncMode::UpdateBoth,
        SyncMode::MirrorRight,
        SyncMode::MirrorLeft,
    ];

    pub fn raw(self) -> &'static str {
        match self {
            SyncMode::UpdateRight => "updateRight",
            SyncMode::UpdateLeft => "updateLeft",
            SyncMode::UpdateBoth => "updateBoth",
            SyncMode::MirrorRight => "mirrorRight",
            SyncMode::MirrorLeft => "mirrorLeft",
        }
    }
    pub fn from_raw(s: &str) -> Option<Self> {
        Some(match s {
            "updateRight" => SyncMode::UpdateRight,
            "updateLeft" => SyncMode::UpdateLeft,
            "updateBoth" => SyncMode::UpdateBoth,
            "mirrorRight" => SyncMode::MirrorRight,
            "mirrorLeft" => SyncMode::MirrorLeft,
            _ => return None,
        })
    }
    pub fn title(self) -> &'static str {
        match self {
            SyncMode::UpdateRight => "Update Right",
            SyncMode::UpdateLeft => "Update Left",
            SyncMode::UpdateBoth => "Update Both",
            SyncMode::MirrorRight => "Mirror to Right",
            SyncMode::MirrorLeft => "Mirror to Left",
        }
    }
    pub fn explain(self) -> &'static str {
        match self {
            SyncMode::UpdateRight => {
                "Copy newer and left-only items to the right. Nothing is deleted."
            }
            SyncMode::UpdateLeft => {
                "Copy newer and right-only items to the left. Nothing is deleted."
            }
            SyncMode::UpdateBoth => "Copy newer and orphan items each way. Nothing is deleted.",
            SyncMode::MirrorRight => {
                "Make the right side the same as the left: copy every difference over, trash right-only items."
            }
            SyncMode::MirrorLeft => {
                "Make the left side the same as the right: copy every difference over, trash left-only items."
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SyncCopy {
    pub src: String,
    pub dst: String,
    pub to: CompareSide,
    pub rel: String,
    pub replaces: bool,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SyncTrash {
    pub path: String,
    pub side: CompareSide,
    pub rel: String,
}

#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct SyncPlan {
    pub copies: Vec<SyncCopy>,
    pub trash: Vec<SyncTrash>,
    pub skipped: Vec<String>,
}

impl SyncPlan {
    pub fn is_empty(&self) -> bool {
        self.copies.is_empty() && self.trash.is_empty()
    }

    pub fn make(tree: &FolderTree, mode: SyncMode, name_filter: &str) -> SyncPlan {
        let mut p = SyncPlan::default();
        let Some(boxed) = tree.call(
            "folder.sync_plan",
            json!({"handle": tree.handle, "mode": mode.raw(), "nameFilter": name_filter}),
        ) else {
            return p;
        };
        p.copies = boxed
            .get("copies")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|c| {
                        let src = c.get("src").and_then(Value::as_str)?.to_string();
                        let dst = c.get("dst").and_then(Value::as_str)?.to_string();
                        let to = CompareSide::from_raw(c.get("to").and_then(Value::as_str)?)?;
                        Some(SyncCopy {
                            src,
                            dst,
                            to,
                            rel: c.get("rel").and_then(Value::as_str).unwrap_or("").to_string(),
                            replaces: c.get("replaces").and_then(Value::as_bool).unwrap_or(false),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        p.trash = boxed
            .get("trash")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|t| {
                        let path = t.get("path").and_then(Value::as_str)?.to_string();
                        let side = CompareSide::from_raw(t.get("side").and_then(Value::as_str)?)?;
                        Some(SyncTrash {
                            path,
                            side,
                            rel: t.get("rel").and_then(Value::as_str).unwrap_or("").to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        p.skipped = boxed
            .get("skipped")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default();
        p
    }
}

// ---------------------------------------------------------------------------
// ComparePane.swift — sessions + view state
// ---------------------------------------------------------------------------

static NEXT_SESSION_ID: OnceLock<Mutex<i64>> = OnceLock::new();

fn next_session_id() -> i64 {
    let m = NEXT_SESSION_ID.get_or_init(|| Mutex::new(0));
    let mut v = m.lock().unwrap();
    *v += 1;
    *v
}

/// `CompareSession` — one text pair plus its view state.
pub struct CompareSession {
    pub id: i64,
    pub model: TextCompare,
    pub path: HashMap<CompareSide, String>,
    pub title: HashMap<CompareSide, String>,
    pub disk: HashMap<CompareSide, Vec<u8>>,
    pub binary: HashMap<CompareSide, Vec<u8>>,
    pub too_large: HashMap<CompareSide, bool>,
    pub git: bool,
    pub clean_depth: [i64; 2],
    pub filter: CompareFilter,
    visible: Option<Vec<usize>>,
    pub cursor: usize,
    pub anchor: Option<usize>,
    pub focus: CompareSide,
    pub col: usize,
    pub scroll_y: f64,
    pub version: u64,
    pub waiters: usize,
    pub changed_on_disk: HashMap<CompareSide, bool>,
    pub status: String,
    pub folder: Option<FolderSession>,
    pub recovered: bool,
    pub recovery_version: [u64; 2],
}

impl CompareSession {
    pub fn new(model: TextCompare) -> Self {
        CompareSession {
            id: next_session_id(),
            model,
            path: HashMap::new(),
            title: HashMap::new(),
            disk: HashMap::new(),
            binary: HashMap::new(),
            too_large: HashMap::new(),
            git: false,
            clean_depth: [0, 0],
            filter: CompareFilter::All,
            visible: None,
            cursor: 0,
            anchor: None,
            focus: CompareSide::Left,
            col: 0,
            scroll_y: 0.0,
            version: 0,
            waiters: 0,
            changed_on_disk: HashMap::new(),
            status: String::new(),
            folder: None,
            recovered: false,
            recovery_version: [0, 0],
        }
    }

    pub(crate) fn edits(&self, s: CompareSide) -> i64 {
        self.model.undo_count(s) as i64
    }

    pub fn dirty(&self, s: CompareSide) -> bool {
        self.edits(s) != self.clean_depth[s.index()]
    }
    pub fn is_dirty(&self) -> bool {
        self.dirty(CompareSide::Left) || self.dirty(CompareSide::Right)
    }
    pub fn mark_clean(&mut self, s: CompareSide) {
        self.clean_depth[s.index()] = self.edits(s);
    }
    pub fn will_edit(&mut self, s: CompareSide) {
        if self.edits(s) < self.clean_depth[s.index()] {
            self.clean_depth[s.index()] = -1;
        }
    }

    pub fn is_binary(&self) -> bool {
        !self.binary.is_empty() || self.too_large.values().any(|v| *v)
    }

    /// `CompareWindow.syncAll`'s header summary label (`state.current.summary`).
    pub fn summary(&self, cfg: &CompareConfig) -> String {
        let m = &self.model;
        if self.is_binary() {
            return self.binary_summary(cfg);
        }
        let n = m.sections().len();
        if n == 0 {
            return self.identical_summary(cfg);
        }
        let mut out = format!("≠ {n} section{}", if n == 1 { "" } else { "s" });
        if m.unimportant_count() > 0 {
            out.push_str(&format!(
                "  ({} important · {} unimportant)",
                m.important_count(),
                m.unimportant_count()
            ));
        }
        out
    }

    /// `identicalSummary(_:)`.
    fn identical_summary(&self, cfg: &CompareConfig) -> String {
        let (l, r) = (&self.model.left, &self.model.right);
        if l.lines.is_empty() && r.lines.is_empty() {
            return String::new();
        }
        if let (Some(a), Some(b)) = (
            self.disk.get(&CompareSide::Left),
            self.disk.get(&CompareSide::Right),
        ) {
            if a == b && !self.is_dirty() {
                return cfg.label("same", "Identical", "");
            }
        }
        if l.encoding == r.encoding && l.lines == r.lines && l.eols == r.eols {
            return cfg.label("same", "Identical", "");
        }
        let mut why: Vec<String> = Vec::new();
        if l.encoding != r.encoding {
            why.push(format!("encoding {} vs {}", l.encoding.label(), r.encoding.label()));
        }
        if l.eol_label() != r.eol_label() || l.eols.last() != r.eols.last() {
            why.push("line endings".into());
        }
        if self.model.ignore_unimportant
            && self.model.rows().iter().any(|row| row.kind != RowKind::Same)
        {
            why.push("unimportant differences".into());
        }
        if why.is_empty() {
            why.push("bytes (unimportant differences ignored)".into());
        }
        cfg.label("same-text", "Same text — files differ in {}", &why.join(", "))
    }

    /// `binarySummary(_:)`.
    fn binary_summary(&self, cfg: &CompareConfig) -> String {
        let empty: Vec<u8> = Vec::new();
        let disk = |side| self.disk.get(&side).unwrap_or(&empty);
        if self.too_large.values().any(|v| *v) && self.binary.is_empty() {
            let d = BinaryCompare::first_difference(disk(CompareSide::Left), disk(CompareSide::Right));
            let what = match d {
                None => "identical bytes".to_string(),
                Some(d) => format!("differ (first difference at byte {d})"),
            };
            return cfg.label(
                "too-large",
                "Too large for Text Compare ([compare] max-lines / max-bytes): {}",
                &what,
            );
        }
        let pick = |side| self.binary.get(&side).or_else(|| self.disk.get(&side)).unwrap_or(&empty);
        let (a, b) = (pick(CompareSide::Left), pick(CompareSide::Right));
        match BinaryCompare::first_difference(a, b) {
            None => cfg.label("binary", "Binary files {}", "are identical"),
            Some(d) => cfg.label(
                "binary",
                "Binary files {}",
                &format!(
                    "differ ({} vs {}, first difference at byte {d})",
                    byte_count_file(a.len() as u64),
                    byte_count_file(b.len() as u64)
                ),
            ),
        }
    }

    pub fn display_count(&self) -> usize {
        self.visible.as_ref().map(|v| v.len()).unwrap_or(self.model.rows().len())
    }

    pub fn model_row(&self, d: usize) -> usize {
        match &self.visible {
            None => d,
            Some(v) => {
                if d < v.len() {
                    v[d]
                } else {
                    v.last().copied().unwrap_or(0)
                }
            }
        }
    }

    pub fn display_row(&self, m: usize) -> usize {
        let Some(v) = &self.visible else { return m };
        let (mut lo, mut hi) = (0usize, v.len());
        while lo < hi {
            let mid = (lo + hi) / 2;
            if v[mid] < m {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lo.min(v.len().saturating_sub(1))
    }

    pub fn refresh(&mut self, context: usize) {
        self.version += 1;
        self.visible = self.model.visible_rows(self.filter, context);
        let dc = self.display_count();
        self.cursor = self.cursor.min(dc.saturating_sub(1));
        if let Some(a) = self.anchor {
            if a >= dc {
                self.anchor = None;
            }
        }
    }

    pub fn visible(&self) -> Option<&[usize]> {
        self.visible.as_deref()
    }

    fn last_component(p: &str) -> String {
        Path::new(p)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| p.to_string())
    }

    pub fn name(&self, s: CompareSide) -> String {
        if let Some(f) = &self.folder {
            let root = match s {
                CompareSide::Left => &f.left_root,
                CompareSide::Right => &f.right_root,
            };
            return format!("{}/", Self::last_component(root));
        }
        if let Some(t) = self.title.get(&s) {
            if !t.is_empty() {
                return Self::last_component(t);
            }
        }
        if let Some(p) = self.path.get(&s) {
            return Self::last_component(p);
        }
        if self.model.side(s).lines.is_empty() {
            "(empty)".to_string()
        } else {
            "(pasted)".to_string()
        }
    }

    pub fn label(&self) -> String {
        if let Some(f) = &self.folder {
            return format!("{}{}", if self.git { "(git) " } else { "" }, f.label());
        }
        let (l, r) = (self.name(CompareSide::Left), self.name(CompareSide::Right));
        let body = if l == r { l } else { format!("{l} ⇆ {r}") };
        format!("{}{}", if self.git { "(git) " } else { "" }, body)
    }
}

/// `FolderSession` — one folder pair plus its view state.
pub struct FolderSession {
    pub left_root: String,
    pub right_root: String,
    pub tree: Option<FolderTree>,
    pub view: FolderView,
    pub rows: Vec<FolderRow>,
    pub cursor: usize,
    pub anchor: Option<usize>,
    pub marked: BTreeSet<usize>,
    pub focus: CompareSide,
    pub scanning: bool,
    pub scanned: usize,
    pub checking: bool,
    pub check_done: usize,
    pub check_total: usize,
    pub status: String,
    pub hidden: bool,
    pub extra_exclude: Vec<String>,
    pub scroll_y: f64,
    pub content_cache: HashMap<String, FolderContentAnswer>,
    pub scan_ms: f64,
    pub expanded_keys: BTreeSet<String>,
    pub cursor_key: Option<String>,
    pub undo: UndoStack,
    pub always_content: bool,
    pub back: Vec<(String, String)>,
    pub forward: Vec<(String, String)>,
}

impl FolderSession {
    pub fn new(left: &str, right: &str) -> Self {
        FolderSession {
            left_root: left.to_string(),
            right_root: right.to_string(),
            tree: None,
            view: FolderView::default(),
            rows: Vec::new(),
            cursor: 0,
            anchor: None,
            marked: BTreeSet::new(),
            focus: CompareSide::Left,
            scanning: false,
            scanned: 0,
            checking: false,
            check_done: 0,
            check_total: 0,
            status: String::new(),
            hidden: true,
            extra_exclude: Vec::new(),
            scroll_y: 0.0,
            content_cache: HashMap::new(),
            scan_ms: 0.0,
            expanded_keys: BTreeSet::new(),
            cursor_key: None,
            undo: UndoStack::default(),
            always_content: false,
            back: Vec::new(),
            forward: Vec::new(),
        }
    }

    pub fn label(&self) -> String {
        let l = CompareSession::last_component(&self.left_root);
        let r = CompareSession::last_component(&self.right_root);
        let body = if l == r { l } else { format!("{l} ⇆ {r}") };
        format!("{body}/")
    }

    /// Recompute `rows` from the tree + view (mirrors `rebuild`).
    pub fn rebuild_rows(&mut self) {
        let keep = self.cursor_key.clone().or_else(|| {
            let tree = self.tree.as_ref()?;
            let r = self.rows.get(self.cursor)?;
            tree.node(r.id).map(|n| n.key.clone())
        });
        let Some(tree) = &self.tree else {
            self.rows.clear();
            return;
        };
        self.rows = tree.rows(&self.view);
        if let Some(k) = keep {
            if let Some(i) = self.rows.iter().position(|r| {
                tree.node(r.id).map(|n| n.key.as_str()) == Some(k.as_str())
            }) {
                self.cursor = i;
            } else {
                self.cursor = self.cursor.min(self.rows.len().saturating_sub(1));
            }
        } else {
            self.cursor = self.cursor.min(self.rows.len().saturating_sub(1));
        }
        self.cursor_key = None;
        let visible: HashSet<usize> = self.rows.iter().map(|r| r.id).collect();
        self.marked.retain(|id| visible.contains(id));
    }
}

/// `FolderPage`'s model state + `testDo` / `testState` hooks.
pub struct FolderPageState {
    pub session: Option<FolderSession>,
    pub last_plan: Option<(SyncMode, SyncPlan)>,
    pub test_clash: Option<Clash>,
    pub quick_look_paths: Vec<String>,
    backend: Arc<dyn HelperBackend>,
    options: FolderOptions,
}

impl Default for FolderPageState {
    fn default() -> Self {
        FolderPageState {
            session: None,
            last_plan: None,
            test_clash: None,
            quick_look_paths: Vec::new(),
            backend: real_backend(),
            options: FolderOptions::default(),
        }
    }
}

impl FolderPageState {
    pub fn new(backend: Arc<dyn HelperBackend>, options: FolderOptions) -> Self {
        FolderPageState { backend, options, ..Default::default() }
    }

    pub fn bind(&mut self, s: FolderSession) {
        self.session = Some(s);
    }

    pub fn test_state(&self) -> Value {
        let Some(s) = &self.session else {
            return Value::Null;
        };
        let tree_counts = s.tree.as_ref().map(|t| t.counts());
        let c = tree_counts.unwrap_or_default();
        let summary = match tree_counts {
            Some(c) => folder_summary(&c),
            None => {
                if s.scanning {
                    format!("scanning… {} items", s.scanned)
                } else {
                    String::new()
                }
            }
        };
        json!({
            "left": s.left_root,
            "right": s.right_root,
            "scanning": s.scanning,
            "checking": s.checking,
            "filter": s.view.filter.raw(),
            "flatten": s.view.flatten,
            "nameFilter": s.view.name_filter,
            "cursor": s.cursor,
            "focus": s.focus.raw(),
            "marked": s.marked.len(),
            "summary": summary,
            "status": s.status,
            "items": s.tree.as_ref().map(|t| t.all().len()).unwrap_or(0),
            "counts": {
                "different": c.different, "unimportant": c.unimportant,
                "leftOnly": c.left_only, "rightOnly": c.right_only,
                "same": c.same, "unknown": c.unknown,
            },
            "scanMs": s.scan_ms as i64,
            "canUndo": s.undo.can_undo(),
            "sharedCanUndo": FileOps::can_undo(),
            // The drag-hover side; the port has no drag hover, so never set.
            "dropSide": Value::Null,
            "back": s.back.len(),
            "forward": s.forward.len(),
            "quickLook": self.quick_look_paths,
            "syncPlan": self.last_plan.as_ref().map(|(m, p)| json!({
                "mode": m.raw(),
                "copies": p.copies.iter().map(|c| format!("{} {}", if c.to == CompareSide::Right { "→" } else { "←" }, c.rel)).collect::<Vec<_>>(),
                "trash": p.trash.iter().map(|t| format!("{}: {}", t.side.raw(), t.rel)).collect::<Vec<_>>(),
                "skipped": p.skipped,
            })),
            "rows": s.rows.iter().take(200).map(|r| {
                let n = s.tree.as_ref().and_then(|t| t.node(r.id));
                json!({
                    "rel": n.map(|n| n.rel.clone()).unwrap_or_default(),
                    "depth": r.depth,
                    "status": n.map(|n| n.status.raw()).unwrap_or("same"),
                    "newer": n.map(|n| n.newer.raw()).unwrap_or("none"),
                    "dir": n.map(|n| n.is_dir()).unwrap_or(false),
                    "expanded": n.map(|n| n.expanded).unwrap_or(false),
                    "left": n.map(|n| n.left.is_some()).unwrap_or(false),
                    "right": n.map(|n| n.right.is_some()).unwrap_or(false),
                })
            }).collect::<Vec<_>>(),
        })
    }

    pub fn test_do(&mut self, action: &str, arg: &str) -> Option<String> {
        if self.session.is_none() {
            return Some("no folder session".to_string());
        }
        match action {
            "filter" => {
                let Some(f) = FolderFilter::from_raw(arg) else {
                    return Some(format!(
                        "folder-filter:{}",
                        FolderFilter::ALL.iter().map(|f| f.raw()).collect::<Vec<_>>().join("|")
                    ));
                };
                self.session.as_mut().unwrap().view.filter = f;
                self.rebuild();
            }
            "flatten" => {
                let s = self.session.as_mut().unwrap();
                s.view.flatten = !s.view.flatten;
                self.rebuild();
            }
            "names" => {
                self.session.as_mut().unwrap().view.name_filter = arg.to_string();
                self.rebuild();
            }
            "expand" => {
                let open = arg != "none";
                if let Some(s) = self.session.as_mut() {
                    if let Some(t) = s.tree.as_mut() {
                        t.expand_all(open);
                    }
                }
                self.rebuild();
            }
            "select" => {
                let s = self.session.as_mut().unwrap();
                let Some(i) = s
                    .rows
                    .iter()
                    .position(|r| s.tree.as_ref().and_then(|t| t.node(r.id)).map(|n| n.rel == arg).unwrap_or(false))
                else {
                    return Some(format!("no row {arg}"));
                };
                s.cursor = i;
            }
            "focus" => {
                if let Some(s) = self.session.as_mut() {
                    s.focus = if arg == "right" { CompareSide::Right } else { CompareSide::Left };
                }
            }
            "hidden" => {
                if let Some(s) = self.session.as_mut() {
                    s.hidden = !s.hidden;
                }
                self.rescan();
            }
            "rescan" => self.rescan(),
            "undo" => {
                if let Some(s) = self.session.as_mut() {
                    let _ = FileOps::undo(&s.undo);
                }
                self.rescan();
            }
            "trash" => self.trash(),
            "copy" | "move" => {
                let bits: Vec<&str> = arg.split(':').collect();
                self.test_clash = if bits.len() > 1 {
                    match bits[1] {
                        "replace" => Some(Clash::Replace),
                        "keep" => Some(Clash::KeepBoth),
                        "skip" => Some(Clash::Skip),
                        _ => None,
                    }
                } else {
                    None
                };
                let from = if bits.first() == Some(&"left") { CompareSide::Right } else { CompareSide::Left };
                let clash = self.test_clash.unwrap_or(Clash::Replace);
                self.transfer(from, action == "move", clash);
                self.test_clash = None;
            }
            "sync" => {
                let bits: Vec<&str> = arg.split(':').collect();
                let Some(mode) = bits.first().and_then(|m| SyncMode::from_raw(m)) else {
                    return Some(format!(
                        "folder-sync:{}",
                        SyncMode::ALL.iter().map(|m| m.raw()).collect::<Vec<_>>().join("|")
                    ));
                };
                let Some(s) = self.session.as_ref() else { return Some("no tree yet".to_string()) };
                let Some(tree) = s.tree.as_ref() else { return Some("no tree yet".to_string()) };
                let plan = SyncPlan::make(tree, mode, &s.view.name_filter);
                self.last_plan = Some((mode, plan.clone()));
                if bits.len() < 2 {
                    self.run_sync(mode, &plan);
                }
            }
            "base" => {
                let bits: Vec<&str> = arg.split(':').collect();
                let Some(rel) = bits.first().copied() else { return Some(format!("no row {arg}")) };
                let s = self.session.as_ref().unwrap();
                let Some(id) = s.rows.iter().find_map(|r| {
                    let n = s.tree.as_ref()?.node(r.id)?;
                    (n.rel == *rel).then_some(r.id)
                }) else {
                    return Some(format!("no row {arg}"));
                };
                let side = bits.get(1).and_then(|s| CompareSide::from_raw(s));
                self.set_base(id, side);
            }
            "up" => self.up_one_level(),
            "back" => self.go_back(),
            "forward" => self.go_forward(),
            "quicklook" => {
                if self.quick_look_paths.is_empty() {
                    self.quick_look_paths = self.preview_paths();
                } else {
                    self.quick_look_paths.clear();
                }
            }
            "drop" => {
                let bits: Vec<&str> = arg.splitn(2, ':').collect();
                let Some(side) = bits.first().and_then(|s| CompareSide::from_raw(s)) else {
                    return Some("folder-drop:left|right[:PATHS]".to_string());
                };
                if bits.len() > 1 {
                    let paths: Vec<String> =
                        bits[1].split(',').filter(|p| !p.is_empty()).map(str::to_string).collect();
                    self.drop_external(side, &paths);
                } else {
                    self.transfer(side.other(), false, Clash::Replace);
                }
            }
            other => return Some(format!("unknown folder action {other}")),
        }
        None
    }

    fn rebuild(&mut self) {
        if let Some(s) = self.session.as_mut() {
            s.rebuild_rows();
        }
    }

    fn rescan(&mut self) {
        let backend = self.backend.clone();
        let options = self.options.clone();
        let Some(s) = self.session.as_mut() else { return };
        if let Some(t) = &s.tree {
            s.expanded_keys = t.all().iter().filter(|n| n.expanded).map(|n| n.key.clone()).collect();
            if let Some(r) = s.rows.get(s.cursor) {
                s.cursor_key = t.node(r.id).map(|n| n.key.clone());
            }
        }
        s.scanning = true;
        s.scanned = 0;
        s.checking = false;
        let (left, right) = (s.left_root.clone(), s.right_root.clone());
        let mut tree = FolderScan::run(backend, &left, &right, &options);
        for id in tree.pending() {
            let key = cache_key(&tree, id);
            if let Some(a) = s.content_cache.get(&key).copied() {
                if let Some(n) = tree.node_mut(id) {
                    FolderContent::apply(a, n);
                }
            }
        }
        let expanded = s.expanded_keys.clone();
        for n in tree.nodes_iter_mut() {
            if expanded.contains(&n.key) {
                n.expanded = true;
            }
        }
        if s.tree.is_none() && tree.roots.len() < 40 {
            let roots = tree.roots.clone();
            for rid in roots {
                if let Some(n) = tree.node_mut(rid) {
                    if n.is_dir() {
                        n.expanded = true;
                    }
                }
            }
        }
        tree.settle();
        s.tree = Some(tree);
        s.scanning = false;
        s.rebuild_rows();
    }

    fn transfer(&mut self, from: CompareSide, move_: bool, clash: Clash) {
        let Some(s) = self.session.as_mut() else { return };
        let Some(tree) = s.tree.as_ref() else { return };
        let ids: Vec<usize> = if s.marked.is_empty() {
            s.rows.get(s.cursor).map(|r| vec![r.id]).unwrap_or_default()
        } else {
            s.marked.iter().copied().collect()
        };
        let items: Vec<(String, String)> = ids
            .iter()
            .map(|id| (tree.path(*id, from), tree.path(*id, from.other())))
            .collect();
        if items.is_empty() {
            return;
        }
        let _ = FileOps::place(&items, move_, clash, &s.undo);
        self.rescan();
    }

    /// `FolderPage.drop` with external paths: copy files into a root (`left` or
    /// `right`) and rescan.
    fn drop_external(&mut self, side: CompareSide, paths: &[String]) {
        if paths.is_empty() {
            return;
        }
        let Some(s) = self.session.as_mut() else { return };
        let dir = match side {
            CompareSide::Left => s.left_root.clone(),
            CompareSide::Right => s.right_root.clone(),
        };
        let _ = FileOps::transfer(paths, &dir, false, &s.undo);
        self.rescan();
    }

    fn trash(&mut self) {
        let Some(s) = self.session.as_mut() else { return };
        let Some(tree) = s.tree.as_ref() else { return };
        let ids: Vec<usize> = if s.marked.is_empty() {
            s.rows.get(s.cursor).map(|r| vec![r.id]).unwrap_or_default()
        } else {
            s.marked.iter().copied().collect()
        };
        let focus = s.focus;
        let paths: Vec<String> = ids.iter().map(|id| tree.path(*id, focus)).collect();
        if !paths.is_empty() {
            let _ = FileOps::trash(&paths, &s.undo);
        }
        self.rescan();
    }

    fn run_sync(&mut self, _mode: SyncMode, plan: &SyncPlan) {
        let Some(s) = self.session.as_mut() else { return };
        let mark = s.undo.count();
        let copies: Vec<(String, String)> =
            plan.copies.iter().map(|c| (c.src.clone(), c.dst.clone())).collect();
        let trash: Vec<String> = plan.trash.iter().map(|t| t.path.clone()).collect();
        if !copies.is_empty() {
            let _ = FileOps::place(&copies, false, Clash::Replace, &s.undo);
        }
        if !trash.is_empty() {
            let _ = FileOps::trash(&trash, &s.undo);
        }
        s.undo.collapse(mark, "sync");
        self.rescan();
    }

    fn set_base(&mut self, id: usize, side: Option<CompareSide>) {
        let Some(s) = self.session.as_mut() else { return };
        let Some(tree) = s.tree.as_ref() else { return };
        let has_left = tree.node(id).map(|n| n.left.is_some()).unwrap_or(false);
        let side = side.unwrap_or(if has_left { CompareSide::Left } else { CompareSide::Right });
        let path = tree.path(id, side);
        s.back.push((s.left_root.clone(), s.right_root.clone()));
        s.forward.clear();
        s.left_root = path.clone();
        s.right_root = path;
        s.tree = None;
        self.rescan();
    }

    fn up_one_level(&mut self) {
        let Some(s) = self.session.as_mut() else { return };
        let l = Path::new(&s.left_root).parent().map(|p| p.to_string_lossy().into_owned());
        let r = Path::new(&s.right_root).parent().map(|p| p.to_string_lossy().into_owned());
        let (Some(l), Some(r)) = (l, r) else { return };
        s.back.push((s.left_root.clone(), s.right_root.clone()));
        s.forward.clear();
        s.left_root = l;
        s.right_root = r;
        s.tree = None;
        self.rescan();
    }

    fn go_back(&mut self) {
        let Some(s) = self.session.as_mut() else { return };
        let Some((l, r)) = s.back.pop() else { return };
        s.forward.push((s.left_root.clone(), s.right_root.clone()));
        s.left_root = l;
        s.right_root = r;
        s.tree = None;
        self.rescan();
    }

    fn go_forward(&mut self) {
        let Some(s) = self.session.as_mut() else { return };
        let Some((l, r)) = s.forward.pop() else { return };
        s.back.push((s.left_root.clone(), s.right_root.clone()));
        s.left_root = l;
        s.right_root = r;
        s.tree = None;
        self.rescan();
    }

    fn preview_paths(&self) -> Vec<String> {
        let Some(s) = &self.session else { return Vec::new() };
        let Some(tree) = &s.tree else { return Vec::new() };
        let Some(r) = s.rows.get(s.cursor) else { return Vec::new() };
        let mut out = Vec::new();
        if tree.node(r.id).map(|n| match s.focus {
            CompareSide::Left => n.left.is_some(),
            CompareSide::Right => n.right.is_some(),
        }) == Some(true)
        {
            out.push(tree.path(r.id, s.focus));
        }
        out
    }
}

fn cache_key(tree: &FolderTree, id: usize) -> String {
    let Some(n) = tree.node(id) else { return String::new() };
    match (&n.left, &n.right) {
        (Some(l), Some(r)) => format!(
            "{}|{}|{}|{}|{}|{}",
            tree.path(id, CompareSide::Left),
            l.size,
            l.mtime,
            tree.path(id, CompareSide::Right),
            r.size,
            r.mtime
        ),
        _ => n.key.clone(),
    }
}

/// `FolderPage.summaryText` — the header summary from the tree counts. Reads
/// `"Identical — N files"` when nothing differs.
fn folder_summary(c: &FolderTreeCounts) -> String {
    let mut parts: Vec<String> = Vec::new();
    if c.different > 0 {
        parts.push(format!("{} differ", c.different));
    }
    if c.unimportant > 0 {
        parts.push(format!("{} unimportant", c.unimportant));
    }
    if c.left_only > 0 {
        parts.push(format!("{} left only", c.left_only));
    }
    if c.right_only > 0 {
        parts.push(format!("{} right only", c.right_only));
    }
    if c.unknown > 0 {
        parts.push(format!("{} unchecked", c.unknown));
    }
    let by_date =
        if c.same_by_metadata > 0 { format!(" ({} by date/size only)", c.same_by_metadata) } else { String::new() };
    if c.same > 0 {
        parts.push(format!("{} same{by_date}", c.same));
    }
    if c.error > 0 {
        parts.push(format!("{} unreadable", c.error));
    }
    if parts.is_empty() {
        return "empty".to_string();
    }
    if c.different + c.left_only + c.right_only + c.unknown + c.unimportant == 0 {
        return format!("Identical — {} files{by_date}", c.same);
    }
    parts.join(" · ")
}

// ---------------------------------------------------------------------------
// CompareWindow.swift — config, recent, window model, test hooks
// ---------------------------------------------------------------------------

pub struct CompareConfig {
    pub entries: HashMap<String, String>,
}

impl Default for CompareConfig {
    fn default() -> Self {
        CompareConfig { entries: HashMap::new() }
    }
}

impl CompareConfig {
    pub fn from_entries(entries: HashMap<String, String>) -> Self {
        CompareConfig { entries }
    }

    pub fn load() -> Self {
        let path = crate::app::paths::Paths::from_env().commands_conf_path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return CompareConfig::default();
        };
        let lines = crate::engines::config_text::config_lines(&text);
        let entries = crate::engines::config_text::config_section_entries(&lines, "compare")
            .into_iter()
            .map(|(_, k, v)| (k, v))
            .collect();
        CompareConfig::from_entries(entries)
    }

    pub fn string(&self, k: &str, d: &str) -> String {
        match self.entries.get(k) {
            Some(v) if !v.trim().is_empty() => v.trim().to_string(),
            _ => d.to_string(),
        }
    }

    pub fn number(&self, k: &str, d: f64) -> f64 {
        self.entries.get(k).and_then(|v| v.trim().parse::<f64>().ok()).unwrap_or(d)
    }

    pub fn bool(&self, k: &str, d: bool) -> bool {
        match self.entries.get(k) {
            Some(v) => crate::engines::config_text::tri(Some(v)).unwrap_or_else(|| {
                match v.trim().to_ascii_lowercase().as_str() {
                    "true" | "yes" | "on" | "1" => true,
                    "false" | "no" | "off" | "0" => false,
                    _ => d,
                }
            }),
            None => d,
        }
    }

    pub fn label(&self, k: &str, d: &str, arg: &str) -> String {
        self.string(&format!("{k}-label"), d).replace("{}", arg)
    }

    pub fn importance(&self) -> Importance {
        Importance::new(
            self.bool("ignore-leading-ws", true),
            self.bool("ignore-trailing-ws", true),
            self.bool("ignore-embedded-ws", false),
            self.bool("ignore-case", false),
            self.bool("ignore-line-endings", true),
            self.bool("ignore-blank-lines", false),
        )
    }

    pub fn context_lines(&self) -> usize {
        self.number("context-lines", 3.0).max(0.0) as usize
    }
    pub fn tab_width(&self) -> usize {
        (self.number("tab-width", 4.0) as i64).clamp(1, 16) as usize
    }
    pub fn max_lines(&self) -> usize {
        (self.number("max-lines", 200_000.0) as i64).max(1000) as usize
    }
    pub fn max_bytes(&self) -> usize {
        (self.number("max-bytes", (50 * 1024 * 1024) as f64) as i64).max(1024 * 1024) as usize
    }
    pub fn recent_limit(&self) -> usize {
        (self.number("recent", 30.0) as i64).clamp(0, 500) as usize
    }
    pub fn gutter_arrows(&self) -> String {
        let v = self.string("gutter-arrows", "hover").to_ascii_lowercase();
        if ["hover", "always", "off"].contains(&v.as_str()) {
            v
        } else {
            "hover".to_string()
        }
    }
}

pub fn compare_enabled(conf: &CompareConfig) -> bool {
    conf.bool("enabled", false)
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct CompareRecentEntry {
    pub left: String,
    pub right: String,
    pub used: f64,
}

pub struct CompareRecent {
    pub base_dir: String,
}

impl CompareRecent {
    pub fn default_dir() -> String {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        format!("{home}/.cache/kitchen-sink")
    }

    pub fn new(base_dir: impl Into<String>) -> Self {
        CompareRecent { base_dir: base_dir.into() }
    }

    pub fn from_env() -> Self {
        CompareRecent::new(Self::default_dir())
    }

    pub fn path(&self) -> String {
        format!("{}/compare-recent.json", self.base_dir)
    }

    pub fn pasted_dir(&self) -> String {
        format!("{}/compare-pasted", self.base_dir)
    }

    pub fn is_pasted(&self, p: &str) -> bool {
        p.starts_with(&format!("{}/", self.pasted_dir()))
    }

    pub fn load(&self) -> Vec<CompareRecentEntry> {
        let Ok(data) = std::fs::read_to_string(self.path()) else {
            return Vec::new();
        };
        serde_json::from_str::<Vec<CompareRecentEntry>>(&data).unwrap_or_default()
    }

    pub fn add(&self, left: &str, right: &str, limit: usize) {
        self.add_at(left, right, limit, now_secs());
    }

    pub fn add_at(&self, left: &str, right: &str, limit: usize, used: f64) {
        if limit == 0 {
            return;
        }
        let mut all: Vec<CompareRecentEntry> = self
            .load()
            .into_iter()
            .filter(|e| !(e.left == left && e.right == right))
            .collect();
        all.insert(0, CompareRecentEntry { left: left.to_string(), right: right.to_string(), used });
        all.truncate(limit);
        self.save(&all);
    }

    pub fn remove(&self, e: &CompareRecentEntry) {
        let all: Vec<CompareRecentEntry> = self.load().into_iter().filter(|x| x != e).collect();
        self.save(&all);
    }

    pub fn clear_all(&self) {
        let _ = std::fs::remove_dir_all(self.pasted_dir());
        let _ = std::fs::create_dir_all(self.pasted_dir());
        let _ = std::fs::write(self.path(), "[]");
    }

    pub fn save(&self, all: &[CompareRecentEntry]) {
        let keep: HashSet<String> = all
            .iter()
            .flat_map(|e| [format!("{}/{}", self.pasted_dir(), file_name(&e.left)), format!("{}/{}", self.pasted_dir(), file_name(&e.right))])
            .collect();
        if let Ok(entries) = std::fs::read_dir(self.pasted_dir()) {
            for entry in entries.flatten() {
                let full = entry.path().to_string_lossy().into_owned();
                if !keep.contains(&full) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        let _ = std::fs::create_dir_all(&self.base_dir);
        if let Ok(data) = serde_json::to_string_pretty(all) {
            let _ = std::fs::write(self.path(), data);
        }
    }
}

fn file_name(p: &str) -> String {
    Path::new(p).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

fn now_secs() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// `CompareWindow`'s model: sessions, the start page, recent pairs and the
/// `do:compare:*` test hooks. The AppKit build (`CardWindowController`) is the
/// host's job (drawing is out of scope for this cut).
pub struct CompareWindowModel {
    pub sessions: Vec<CompareSession>,
    pub selected: Option<usize>,
    pub sub: bool,
    /// Index of the session a dirty close is asking about (`confirmCard`); the
    /// `do:compare:close-session` sheet. Cleared on cancel or when the list
    /// changes.
    pub confirm: Option<usize>,
    pub recent: CompareRecent,
    pub config: CompareConfig,
    pub folder_page: FolderPageState,
    /// Id of the session whose `FolderSession` is bound into `folder_page`
    /// (Swift keeps it on `CompareSession.folder`; the page shows the selected
    /// one). Unselected folder sessions park theirs on `CompareSession.folder`.
    folder_bound: Option<i64>,
    /// `alignPick`: the first line picked for Align With (side, 0-based line).
    pub align_pick: Option<(CompareSide, i64)>,
    /// The compareText sub-view (Swift's separate `CompareWindow.sub`): the
    /// session it shows and the one to reselect when it closes.
    sub_session: Option<i64>,
    sub_return: Option<i64>,
    /// Set when the model opened the sub-view itself (`folder-open`); the
    /// host takes it and pushes `.compareText`.
    sub_pending: bool,
    pub recent_selection: usize,
    backend: Arc<dyn HelperBackend>,
}

impl CompareWindowModel {
    pub fn new(config: CompareConfig, recent: CompareRecent) -> Self {
        Self::with_backend(real_backend(), config, recent)
    }

    pub fn with_backend(
        backend: Arc<dyn HelperBackend>,
        config: CompareConfig,
        recent: CompareRecent,
    ) -> Self {
        let folder_page = FolderPageState::new(backend.clone(), FolderOptions::default());
        CompareWindowModel {
            sessions: Vec::new(),
            selected: None,
            sub: false,
            confirm: None,
            recent,
            config,
            folder_page,
            folder_bound: None,
            align_pick: None,
            sub_session: None,
            sub_return: None,
            sub_pending: false,
            recent_selection: 0,
            backend,
        }
    }

    pub fn backend(&self) -> &dyn HelperBackend {
        self.backend.as_ref()
    }

    pub fn current(&self) -> Option<&CompareSession> {
        self.selected.and_then(|i| self.sessions.get(i))
    }
    pub fn current_mut(&mut self) -> Option<&mut CompareSession> {
        self.selected.and_then(|i| self.sessions.get_mut(i))
    }
    pub fn on_start_page(&self) -> bool {
        self.selected.is_none()
    }

    /// The selected session is a folder pair (its `FolderSession` is bound
    /// into `folder_page`, or parked on the session).
    pub fn current_is_folder(&self) -> bool {
        self.current()
            .map_or(false, |s| s.folder.is_some() || self.folder_bound == Some(s.id))
    }

    /// A cheap fingerprint of everything the live compare surface draws, so
    /// the daemon UI repaints only on change (no line text is hashed: edits,
    /// reloads and re-diffs all move the row tuples / edit depths).
    pub fn draw_fingerprint(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (self.selected, self.sub, self.sessions.len(), self.current_is_folder()).hash(&mut h);
        if let Some(fs) = &self.folder_page.session {
            (fs.rows.len(), fs.cursor, fs.scanning, fs.scanned, fs.tree.is_some()).hash(&mut h);
            for r in &fs.rows {
                (r.id, r.depth).hash(&mut h);
            }
        }
        if let Some(s) = self.current() {
            let m = &s.model;
            (
                s.id,
                s.cursor,
                s.focus.raw(),
                s.filter.raw(),
                s.display_count(),
                s.edits(CompareSide::Left),
                s.edits(CompareSide::Right),
                m.left.lines.len(),
                m.right.lines.len(),
                m.sections().len(),
                m.anchors().len(),
                m.ignore_unimportant,
            )
                .hash(&mut h);
            for r in m.rows() {
                (r.l, r.r, r.kind as u8, r.important).hash(&mut h);
            }
            s.status.hash(&mut h);
        }
        h.finish()
    }

    fn load_side(&self, path: Option<&str>) -> (TextSide, Option<Vec<u8>>, bool) {
        let Some(p) = path else {
            return (TextSide::default(), None, false);
        };
        let Ok(data) = std::fs::read(p) else {
            return (TextSide::default(), None, false);
        };
        if data.len() > self.config.max_bytes() {
            return (TextSide::default(), Some(Vec::new()), true);
        }
        match TextSide::decode(self.backend.as_ref(), &data) {
            Some(side) => (side, None, false),
            None => (TextSide::default(), Some(data), false),
        }
    }

    /// Park the bound folder session back on its `CompareSession`.
    fn unbind_folder(&mut self) {
        let Some(id) = self.folder_bound.take() else { return };
        let fs = self.folder_page.session.take();
        if let Some(s) = self.sessions.iter_mut().find(|s| s.id == id) {
            s.folder = fs;
        }
    }

    /// Bind the selected session's parked folder session into the page.
    fn bind_selected_folder(&mut self) {
        self.unbind_folder();
        let Some(s) = self.selected.and_then(|i| self.sessions.get_mut(i)) else { return };
        if let Some(fs) = s.folder.take() {
            self.folder_bound = Some(s.id);
            self.folder_page.bind(fs);
        }
    }

    /// `CompareWindow.openFolders`: a folder session, scanned and bound.
    pub fn open_folders(&mut self, left: &str, right: &str) -> usize {
        self.unbind_folder();
        let model =
            TextCompare::with_backend(self.backend.clone(), TextSide::default(), TextSide::default(), self.config.importance(), false);
        let mut s = CompareSession::new(model);
        s.path.insert(CompareSide::Left, left.to_string());
        s.path.insert(CompareSide::Right, right.to_string());
        self.folder_bound = Some(s.id);
        self.sessions.push(s);
        let idx = self.sessions.len() - 1;
        self.selected = Some(idx);
        self.folder_page.bind(FolderSession::new(left, right));
        self.folder_page.rescan();
        self.recent.add(left, right, self.config.recent_limit());
        idx
    }

    /// Open a pair (`CompareWindow.openPair`): two folders open a folder
    /// session; otherwise a text pair, a missing side empty.
    pub fn open_pair(&mut self, left: Option<&str>, right: Option<&str>) -> usize {
        let is_dir = |p: Option<&str>| p.map_or(false, |p| Path::new(p).is_dir());
        if let (Some(l), Some(r)) = (left, right) {
            if is_dir(left) && is_dir(right) {
                return self.open_folders(l, r);
            }
        }
        self.unbind_folder();
        let (lside, lbin, ltoo) = self.load_side(left);
        let (rside, rbin, rtoo) = self.load_side(right);
        let model = TextCompare::with_backend(
            self.backend.clone(),
            lside,
            rside,
            self.config.importance(),
            false,
        );
        let mut s = CompareSession::new(model);
        if let Some(p) = left {
            s.path.insert(CompareSide::Left, p.to_string());
        }
        if let Some(p) = right {
            s.path.insert(CompareSide::Right, p.to_string());
        }
        if let Some(b) = lbin {
            s.binary.insert(CompareSide::Left, b);
        }
        if let Some(b) = rbin {
            s.binary.insert(CompareSide::Right, b);
        }
        s.too_large.insert(CompareSide::Left, ltoo);
        s.too_large.insert(CompareSide::Right, rtoo);
        s.refresh(self.config.context_lines());
        // `openPair`: the cursor starts on the first difference.
        if let Some(start) = s.model.sections().first().map(|x| x.rows.start) {
            s.cursor = s.display_row(start);
        }
        self.sessions.push(s);
        let idx = self.sessions.len() - 1;
        self.selected = Some(idx);
        if let (Some(l), Some(r)) = (left, right) {
            self.recent.add(l, r, self.config.recent_limit());
        }
        idx
    }

    /// `beginEdit` + `commitEditor`: replace the cursor's block on `side` —
    /// its whole section (unless filtering to Same), else its one row — with
    /// `text`, then re-diff keeping the cursor.
    pub fn edit_at_cursor(&mut self, side: CompareSide, text: &str) {
        let ctx = self.config.context_lines();
        let Some(s) = self.current_mut() else { return };
        if s.is_binary() {
            return;
        }
        s.focus = side;
        let n = s.display_count();
        let m = if n > 0 { s.model_row(s.cursor) } else { 0 };
        let rows = if n == 0 {
            0..0
        } else {
            match s.model.section_at(m) {
                Some(si) if s.filter != CompareFilter::Same => s.model.sections()[si].rows.clone(),
                _ => m..m + 1,
            }
        };
        let lines = if n == 0 { 0..0 } else { s.model.line_range(side, rows) };
        let original: String =
            s.model.side(side).lines[lines.clone()].iter().map(|l| format!("{l}\n")).collect();
        if text == original {
            s.status.clear();
            return;
        }
        s.will_edit(side);
        s.model.replace(side, lines, &edited_lines(text));
        s.status = "edited".to_string();
        s.refresh(ctx);
    }

    /// `showCompare(paths, titles:, git:, waiter:)` from a `compare\t…` socket
    /// message: open the pair with its titles; `wait` registers a waiter (the
    /// git difftool `--wait`). Returns the session id.
    pub fn open_message(
        &mut self,
        left: &str,
        right: Option<&str>,
        titles: &HashMap<CompareSide, String>,
        wait: bool,
    ) -> Option<i64> {
        self.close_sub();
        let idx = self.open_pair(Some(left), right);
        let s = self.sessions.get_mut(idx)?;
        s.title = titles.clone();
        s.git = wait;
        if wait {
            s.waiters += 1;
        }
        Some(s.id)
    }

    /// Whether a `--wait` caller on session `id` is still waiting (the session
    /// is open and its waiter has not been released).
    pub fn is_waiting(&self, id: i64) -> bool {
        self.sessions.iter().any(|s| s.id == id && s.waiters > 0)
    }

    /// `finishWaiters` (the view parked on hide): release every waiter, and
    /// drop the clean git sessions they were holding.
    pub fn finish_waiters(&mut self) {
        let done: Vec<i64> = self
            .sessions
            .iter_mut()
            .filter(|s| s.waiters > 0)
            .map(|s| {
                s.waiters = 0;
                s.id
            })
            .collect();
        for id in done {
            if let Some(i) = self.sessions.iter().position(|s| s.id == id && s.git && !s.is_dirty()) {
                self.close_session(i);
            }
        }
    }

    /// Open a pair in the compareText sub-view (`createSub` + `openPair`);
    /// the current session is reselected by [`Self::close_sub`].
    pub fn open_sub(&mut self, left: Option<&str>, right: Option<&str>) {
        self.close_sub();
        let ret = self.current().map(|s| s.id);
        self.open_pair(left, right);
        self.sub_session = self.current().map(|s| s.id);
        self.sub_return = ret;
        self.sub = true;
        self.sub_pending = true;
    }

    /// The host's cue to push `.compareText` after a model-side `open_sub`.
    pub fn take_sub_pending(&mut self) -> bool {
        std::mem::take(&mut self.sub_pending)
    }

    /// The sub-view closed (back / Esc): drop its session and return to the
    /// one underneath (rebinding a folder session).
    pub fn close_sub(&mut self) {
        if !self.sub {
            return;
        }
        self.sub = false;
        self.sub_pending = false;
        if let Some(id) = self.sub_session.take() {
            if let Some(i) = self.sessions.iter().position(|s| s.id == id) {
                if self.folder_bound == Some(id) {
                    self.folder_bound = None;
                    self.folder_page.session = None;
                }
                self.sessions.remove(i);
            }
        }
        let ret = self.sub_return.take();
        self.selected = ret
            .and_then(|id| self.sessions.iter().position(|s| s.id == id))
            .or_else(|| self.sessions.len().checked_sub(1));
        self.bind_selected_folder();
    }

    pub fn show_start_page(&mut self) {
        self.selected = None;
    }

    pub fn start_pasted(&mut self) {
        let model =
            TextCompare::with_backend(self.backend.clone(), TextSide::default(), TextSide::default(), self.config.importance(), false);
        let mut s = CompareSession::new(model);
        s.focus = CompareSide::Right;
        s.refresh(self.config.context_lines());
        self.sessions.push(s);
        self.selected = Some(self.sessions.len() - 1);
    }

    /// Replace one side's text in place (`CompareWindow.setPasted`): the whole
    /// side is swapped through `TextCompare.replace`, so the edit lands on the
    /// undo stack and marks the side dirty; empty text clears the side. A paste
    /// into an empty side drops its path (it is no longer backed by the file).
    pub fn set_pasted(&mut self, text: &str, side: CompareSide) {
        let ctx = self.config.context_lines();
        let Some(si) = self.selected else { return };
        let lines = pasted_side(text).lines;
        let s = &mut self.sessions[si];
        let count = s.model.side(side).lines.len();
        s.model.replace(side, 0..count, &lines);
        // A paste into an empty side is no longer backed by a file; replacing a
        // side in place keeps its path so it can still be saved (Swift's
        // `setPasted` only clears the path on the empty-side branch).
        if count == 0 {
            s.path.remove(&side);
        }
        s.refresh(ctx);
    }

    pub fn close_session(&mut self, i: usize) {
        self.confirm = None;
        if i < self.sessions.len() {
            if self.folder_bound == Some(self.sessions[i].id) {
                self.folder_bound = None;
                self.folder_page.session = None;
            }
            self.sessions.remove(i);
            self.selected = if self.sessions.is_empty() {
                None
            } else {
                Some(i.min(self.sessions.len() - 1))
            };
            self.bind_selected_folder();
        }
    }

    pub fn close_all(&mut self) {
        self.confirm = None;
        self.sessions.clear();
        self.selected = None;
        self.folder_bound = None;
        self.folder_page.session = None;
    }

    /// `folder-open`: activate the selected folder row — expand a directory, or
    /// open its two paths as a text compare (Swift's `FolderPage.activate` +
    /// `folderOpenPair`). Handled at the model layer because it needs the
    /// session list; the other `folder-*` actions stay in [`FolderPageState`].
    fn folder_open(&mut self) -> Option<String> {
        enum Act {
            Expand(usize),
            Open(Option<String>, Option<String>),
            Nothing,
        }
        let act = {
            let Some(s) = self.folder_page.session.as_ref() else {
                return Some("no folder session".to_string());
            };
            let Some(tree) = s.tree.as_ref() else {
                return Some("no tree yet".to_string());
            };
            let Some(row) = s.rows.get(s.cursor) else {
                return Some("no row".to_string());
            };
            let Some(n) = tree.node(row.id) else {
                return Some("no row".to_string());
            };
            if n.is_dir() && !n.kind_mismatch() {
                Act::Expand(row.id)
            } else if n.left.as_ref().map(|x| x.is_dir).unwrap_or(false)
                || n.right.as_ref().map(|x| x.is_dir).unwrap_or(false)
            {
                Act::Nothing
            } else {
                let l = n.left.is_some().then(|| tree.path(row.id, CompareSide::Left));
                let r = n.right.is_some().then(|| tree.path(row.id, CompareSide::Right));
                Act::Open(l, r)
            }
        };
        match act {
            Act::Expand(id) => {
                if let Some(s) = self.folder_page.session.as_mut() {
                    if let Some(t) = s.tree.as_mut() {
                        if let Some(n) = t.node_mut(id) {
                            n.expanded = !n.expanded;
                        }
                    }
                    s.rebuild_rows();
                }
            }
            Act::Open(l, r) => self.open_sub(l.as_deref(), r.as_deref()),
            Act::Nothing => {}
        }
        None
    }

    pub fn set_filter(&mut self, f: CompareFilter) {
        let ctx = self.config.context_lines();
        if let Some(s) = self.current_mut() {
            s.filter = f;
            s.refresh(ctx);
        }
    }

    pub fn move_cursor(&mut self, row: i64, extend: bool) {
        let Some(s) = self.current_mut() else { return };
        let n = s.display_count() as i64;
        if n == 0 {
            return;
        }
        let target = row.clamp(0, n - 1) as usize;
        if extend {
            if s.anchor.is_none() {
                s.anchor = Some(s.cursor);
            }
        } else {
            s.anchor = None;
        }
        s.cursor = target;
    }

    pub fn select(&mut self, i: usize) {
        if self.selected.is_some() {
            self.move_cursor(i as i64, false);
        } else {
            self.recent_selection = i;
        }
    }

    pub fn jump_section(&mut self, dir: i32) {
        let Some(s) = self.current_mut() else { return };
        let row = s.model_row(s.cursor);
        let next = if dir >= 0 { s.model.next_section(row) } else { s.model.prev_section(row) };
        if let Some(si) = next {
            let target = s.model.sections()[si].rows.start;
            s.cursor = s.display_row(target);
        }
    }

    pub fn copy_across(&mut self, from: CompareSide) {
        let ctx = self.config.context_lines();
        let Some(s) = self.current_mut() else { return };
        let row = s.model_row(s.cursor);
        let range = if let Some(si) = s.model.section_at(row) {
            s.model.sections()[si].rows.clone()
        } else if row < s.model.rows().len() {
            row..row + 1
        } else {
            return;
        };
        s.model.copy_rows(range, from);
        s.refresh(ctx);
    }

    pub fn swap_sides(&mut self) {
        let ctx = self.config.context_lines();
        let Some(s) = self.current_mut() else { return };
        s.model.swap_sides();
        s.path = swap_map(&s.path);
        s.title = swap_map(&s.title);
        s.disk = swap_map(&s.disk);
        s.binary = swap_map(&s.binary);
        s.too_large = swap_map(&s.too_large);
        s.refresh(ctx);
    }

    pub fn undo(&mut self, redo: bool) {
        let ctx = self.config.context_lines();
        let Some(s) = self.current_mut() else { return };
        if redo {
            s.model.redo();
        } else {
            let _ = s.model.undo(None);
        }
        s.refresh(ctx);
    }

    /// `alignWith(side:line:)`: the first call picks a line; a pick on the
    /// other side then anchors the two together and moves the cursor there.
    pub fn align_with(&mut self, side: CompareSide, line: i64) {
        let ctx = self.config.context_lines();
        let pick = self.align_pick;
        let Some(s) = self.current_mut() else { return };
        if line < 0 {
            return;
        }
        if let Some((_, pline)) = pick.filter(|(ps, _)| *ps != side) {
            let (l, r) = if side == CompareSide::Left { (line, pline) } else { (pline, line) };
            s.model.align(l as usize, r as usize);
            s.status = format!("aligned left {} with right {}", l + 1, r + 1);
            s.refresh(ctx);
            if let Some(row) = s.model.rows.iter().position(|x| x.l as i64 == l && x.r as i64 == r) {
                s.cursor = s.display_row(row);
            }
            self.align_pick = None;
            return;
        }
        s.status = format!(
            "Align With: now pick a line on the {} side (right-click ▸ Align With Picked…)",
            side.other().raw()
        );
        self.align_pick = Some((side, line));
    }

    pub fn clear_alignment(&mut self, row: Option<usize>) {
        let ctx = self.config.context_lines();
        let Some(s) = self.current_mut() else { return };
        s.model.clear_alignment(row);
        s.refresh(ctx);
        self.align_pick = None;
    }

    pub fn trim(&mut self, side: CompareSide) {
        let ctx = self.config.context_lines();
        let Some(s) = self.current_mut() else { return };
        s.model.trim_trailing_whitespace(side);
        s.refresh(ctx);
    }

    pub fn convert_eol(&mut self, side: CompareSide, eol: Eol) {
        let ctx = self.config.context_lines();
        let Some(s) = self.current_mut() else { return };
        s.model.convert_line_endings(side, eol);
        s.refresh(ctx);
    }

    pub fn save(&mut self, side: CompareSide) -> bool {
        let ctx = self.config.context_lines();
        let backend = self.backend.clone();
        let Some(s) = self.current_mut() else { return false };
        let Some(path) = s.path.get(&side).cloned() else { return false };
        let Some(data) = s.model.side(side).encoded(backend.as_ref()) else { return false };
        if std::fs::write(&path, &data).is_err() {
            return false;
        }
        s.disk.insert(side, data);
        s.mark_clean(side);
        s.refresh(ctx);
        true
    }

    fn reload(&mut self) {
        let ctx = self.config.context_lines();
        let backend = self.backend.clone();
        let Some(s) = self.current_mut() else { return };
        for side in [CompareSide::Left, CompareSide::Right] {
            if let Some(p) = s.path.get(&side).cloned() {
                if let Ok(data) = std::fs::read(&p) {
                    if let Some(ts) = TextSide::decode(backend.as_ref(), &data) {
                        s.model.set_side(side, ts);
                    }
                }
            }
        }
        s.refresh(ctx);
    }

    pub fn test_state(&self) -> Value {
        let sessions: Vec<Value> = self
            .sessions
            .iter()
            .map(|s| {
                json!({
                    "kind": if s.folder.is_some() { "folder" } else { "text" },
                    "left": s.path.get(&CompareSide::Left).cloned().unwrap_or_else(|| s.name(CompareSide::Left)),
                    "right": s.path.get(&CompareSide::Right).cloned().unwrap_or_else(|| s.name(CompareSide::Right)),
                    "dirtyL": s.dirty(CompareSide::Left),
                    "dirtyR": s.dirty(CompareSide::Right),
                    "git": s.git,
                    "waiting": s.waiters > 0,
                })
            })
            .collect();
        let mut st = json!({
            "sub": self.sub,
            "startPage": self.selected.is_none(),
            "selected": self.selected.map(|i| i as i64).unwrap_or(-1),
            "sheet": self.confirm.is_some(),
            "sessions": sessions,
            "recent": self.recent.load().len(),
            "folder": self.folder_page.test_state(),
        });
        if let Some(s) = self.current() {
            let m = &s.model;
            let rows: Vec<Value> = (0..s.display_count().min(50))
                .map(|d| {
                    let r = m.rows()[s.model_row(d)];
                    let status = if r.kind == RowKind::Same {
                        "same"
                    } else if !m.is_diff(&r) {
                        "same"
                    } else if r.important {
                        "important"
                    } else {
                        "unimportant"
                    };
                    json!({
                        "l": if r.l >= 0 { Value::String(m.left.lines[r.l as usize].clone()) } else { Value::Null },
                        "r": if r.r >= 0 { Value::String(m.right.lines[r.r as usize].clone()) } else { Value::Null },
                        "status": status,
                        "kind": r.kind.raw(),
                    })
                })
                .collect();
            st["current"] = json!({
                "sections": m.sections().len(),
                "important": m.important_count(),
                "unimportant": m.unimportant_count(),
                "cursorRow": s.cursor,
                "focus": s.focus.raw(),
                "filter": s.filter.raw(),
                "rows": rows,
                "editing": false,
                "find": false,
                "pathEdit": false,
                "banner": false,
                "alignPick": self
                    .align_pick
                    .map(|(side, line)| json!(format!("{}:{}", side.raw(), line + 1)))
                    .unwrap_or(Value::Null),
                "summary": s.summary(&self.config),
                "status": s.status,
                "scrollY": s.scroll_y as i64,
                "leftLines": m.left.lines.len(),
                "rightLines": m.right.lines.len(),
                "displayRows": s.display_count(),
                "canUndo": m.can_undo(),
                "anchors": m.anchors().iter().map(|(l, r)| json!([l + 1, r + 1])).collect::<Vec<_>>(),
                "eol": [m.left.eol_label(), m.right.eol_label()],
                "whitespace": false,
                "recovered": s.recovered,
            });
        }
        st
    }

    /// `CompareWindow.testDo` — the `do:compare:*` hook body. `None` = ok.
    pub fn test_do(&mut self, action: &str) -> Option<String> {
        let parts: Vec<&str> = action.splitn(3, ':').collect();
        let head = parts.first().copied().unwrap_or("");
        if let Some(rest) = head.strip_prefix("folder-") {
            let arg = if parts.len() > 1 {
                if parts.len() > 2 {
                    format!("{}:{}", parts[1], parts[2])
                } else {
                    parts[1].to_string()
                }
            } else {
                String::new()
            };
            if rest == "open" {
                return self.folder_open();
            }
            return self.folder_page.test_do(rest, &arg);
        }
        match head {
            "next" => self.jump_section(1),
            "prev" => self.jump_section(-1),
            "copy-right" => self.copy_across(CompareSide::Left),
            "copy-left" => self.copy_across(CompareSide::Right),
            "swap" => self.swap_sides(),
            "reload" => self.reload(),
            "undo" => self.undo(false),
            "redo" => self.undo(true),
            "start" => self.show_start_page(),
            "start-paste" => self.start_pasted(),
            "edit-begin" | "edit-commit" => {}
            "filter" => {
                let Some(f) = parts.get(1).and_then(|s| CompareFilter::from_raw(s)) else {
                    return Some("filter:all|diffs|same|context".to_string());
                };
                self.set_filter(f);
            }
            "save" => {
                let Some(side) = parts.get(1).and_then(|s| CompareSide::from_raw(s)) else {
                    return Some("save:left|right".to_string());
                };
                self.save(side);
            }
            "select" => {
                let Some(i) = parts.get(1).and_then(|s| s.parse::<usize>().ok()) else {
                    return Some("select:N".to_string());
                };
                self.select(i);
            }
            "close-session" => {
                let force = parts.get(1) == Some(&"force");
                let Some(i) = self.selected else { return Some("no session".to_string()) };
                if !force && self.current().map(|s| s.is_dirty()).unwrap_or(false) {
                    self.confirm = Some(i);
                } else {
                    self.close_session(i);
                }
            }
            "close-all" => self.close_all(),
            "align" => {
                let nums: Vec<i64> = parts
                    .get(1)
                    .map(|s| s.split(',').filter_map(|x| x.parse::<i64>().ok()).collect())
                    .unwrap_or_default();
                if nums.len() != 2 {
                    return Some("align:LEFT,RIGHT (1-based lines)".to_string());
                }
                self.align_pick = None;
                self.align_with(CompareSide::Left, nums[0] - 1);
                self.align_with(CompareSide::Right, nums[1] - 1);
            }
            "align-clear" => self.clear_alignment(None),
            "trim" => {
                let Some(side) = parts.get(1).and_then(|s| CompareSide::from_raw(s)) else {
                    return Some("trim:left|right".to_string());
                };
                self.trim(side);
            }
            "eol" => {
                let eol = match parts.get(2).copied() {
                    Some("lf") => Eol::Lf,
                    Some("crlf") => Eol::Crlf,
                    Some("cr") => Eol::Cr,
                    _ => return Some("eol:left|right:lf|crlf|cr".to_string()),
                };
                let Some(side) = parts.get(1).and_then(|s| CompareSide::from_raw(s)) else {
                    return Some("eol:left|right:lf|crlf|cr".to_string());
                };
                self.convert_eol(side, eol);
            }
            "whitespace" => {}
            "sheet-cancel" => self.confirm = None,
            "paste" => {
                let (Some(side), Some(text)) =
                    (parts.get(1).and_then(|s| CompareSide::from_raw(s)), parts.get(2))
                else {
                    return Some("paste:left|right:TEXT".to_string());
                };
                self.set_pasted(&text.replace("\\n", "\n"), side);
            }
            "edit" => {
                let (Some(side), Some(text)) =
                    (parts.get(1).and_then(|s| CompareSide::from_raw(s)), parts.get(2))
                else {
                    return Some("edit:left|right:TEXT".to_string());
                };
                self.edit_at_cursor(side, &text.replace("\\n", "\n"));
            }
            "cursor" => {
                let Some(n) = parts.get(1).and_then(|s| s.parse::<i64>().ok()) else {
                    return Some("cursor:N".to_string());
                };
                self.move_cursor(n, false);
            }
            "key" => {}
            other => return Some(format!("unknown compare action {other}")),
        }
        None
    }
}

/// `CompareEditor.editedLines`: normalize line endings, drop one trailing
/// newline, split; empty text is no lines.
pub(crate) fn edited_lines(text: &str) -> Vec<String> {
    let t = text.replace("\r\n", "\n").replace('\r', "\n");
    if t.is_empty() {
        return Vec::new();
    }
    let t = t.strip_suffix('\n').unwrap_or(&t);
    t.split('\n').map(str::to_string).collect()
}

fn swap_map<V: Clone>(m: &HashMap<CompareSide, V>) -> HashMap<CompareSide, V> {
    let mut out = HashMap::new();
    if let Some(v) = m.get(&CompareSide::Left) {
        out.insert(CompareSide::Right, v.clone());
    }
    if let Some(v) = m.get(&CompareSide::Right) {
        out.insert(CompareSide::Left, v.clone());
    }
    out
}

/// Local mirror of `compare_text._split`: a trailing newline ends the last
/// line without adding an empty one; new lines take LF.
fn pasted_side(text: &str) -> TextSide {
    let mut lines = Vec::new();
    let mut eols = Vec::new();
    let mut cur = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\n' => {
                lines.push(std::mem::take(&mut cur));
                eols.push(Eol::Lf);
            }
            '\r' => {
                lines.push(std::mem::take(&mut cur));
                if chars.peek() == Some(&'\n') {
                    chars.next();
                    eols.push(Eol::Crlf);
                } else {
                    eols.push(Eol::Cr);
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
        eols.push(Eol::None);
    }
    TextSide { lines, eols, encoding: TextEncodingKind::Utf8 }
}

// ---------------------------------------------------------------------------
// ComparePane.swift / CompareFolderView.swift — drawing models (pure, tested)
// ---------------------------------------------------------------------------

use crate::ui::chrome::Rect;

/// Row height of the two-pane diff (`ComparePaneView.rowH`).
pub const PANE_ROW_HEIGHT: f64 = 18.0;
/// Height of the header strip above the two panes.
pub const PANE_HEADER_HEIGHT: f64 = 26.0;
/// Width of the centre gutter between the panes (`ComparePaneView.gutterW`).
pub const PANE_GUTTER_WIDTH: f64 = 34.0;

/// The geometry `ComparePaneView` uses to map points to rows/panes and lay out
/// line numbers + text. Pure so the math is unit-tested without AppKit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaneLayout {
    pub width: f64,
    pub height: f64,
    pub row_height: f64,
    pub header: f64,
    pub gutter: f64,
    pub char_w: f64,
}

impl PaneLayout {
    pub fn new(width: f64, height: f64) -> Self {
        PaneLayout {
            width,
            height,
            row_height: PANE_ROW_HEIGHT,
            header: PANE_HEADER_HEIGHT,
            gutter: PANE_GUTTER_WIDTH,
            char_w: 7.8,
        }
    }

    /// `paneW`: half the width minus the centre gutter.
    pub fn pane_w(&self) -> f64 {
        ((self.width - self.gutter) / 2.0).floor().max(1.0)
    }

    /// `paneX(_:)`: left pane at 0, right pane past the gutter.
    pub fn pane_x(&self, side: CompareSide) -> f64 {
        match side {
            CompareSide::Left => 0.0,
            CompareSide::Right => self.pane_w() + self.gutter,
        }
    }

    /// `side(at:)`: which pane a horizontal offset falls in.
    pub fn side_at(&self, x: f64) -> CompareSide {
        if x < self.pane_w() + self.gutter / 2.0 {
            CompareSide::Left
        } else {
            CompareSide::Right
        }
    }

    pub fn body_top(&self) -> f64 {
        self.header
    }

    pub fn row_y(&self, display_row: usize) -> f64 {
        self.header + display_row as f64 * self.row_height
    }

    /// `row(at:)`: the display row under a vertical offset.
    pub fn row_at(&self, y: f64) -> usize {
        let rel = (y - self.header).max(0.0);
        (rel / self.row_height).floor() as usize
    }

    /// `NSRect` of one pane cell (flipped, top-left origin).
    pub fn row_rect(&self, side: CompareSide, display_row: usize) -> Rect {
        Rect::new(self.pane_x(side), self.row_y(display_row), self.pane_w(), self.row_height)
    }

    /// `docHeight` for a scroll view's document view.
    pub fn doc_height(&self, display_count: usize) -> f64 {
        self.header + display_count as f64 * self.row_height
    }

    /// `lineNoW`: the gutter width for the largest line number.
    pub fn line_no_width(&self, max_lines: usize) -> f64 {
        digit_count(max_lines.max(99)) as f64 * self.char_w + 14.0
    }

    /// `textRect(_:row:).minX`: where a line's text starts.
    pub fn text_x(&self, side: CompareSide, max_lines: usize) -> f64 {
        self.pane_x(side) + self.line_no_width(max_lines) + 6.0
    }
}

fn digit_count(mut n: usize) -> usize {
    if n == 0 {
        return 1;
    }
    let mut c = 0;
    while n > 0 {
        n /= 10;
        c += 1;
    }
    c
}

/// The per-pane background class of one row.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PaneCell {
    /// This side has no line for the row (`row.line(side) < 0`).
    Empty,
    /// Identical line on both sides.
    Same,
    /// A differing, important line.
    DiffImportant,
    /// A differing line the current settings do not treat as important.
    DiffUnimportant,
    /// A differing line that `ignore_unimportant` hides (drawn as `Same`).
    Ignored,
}

/// `TextCompare.isDiff(_:)` extracted so the pane/cell classifiers share it.
pub fn is_diff_row(row: &CompareRow, ignore_unimportant: bool) -> bool {
    row.kind != RowKind::Same && (row.important || !ignore_unimportant)
}

/// `ComparePaneView.drawRow`'s per-side classification.
pub fn pane_cell(row: &CompareRow, side: CompareSide, ignore_unimportant: bool) -> PaneCell {
    if row.line(side) < 0 {
        return PaneCell::Empty;
    }
    if row.kind == RowKind::Same {
        return PaneCell::Same;
    }
    if !row.important && ignore_unimportant {
        return PaneCell::Ignored;
    }
    if row.important {
        PaneCell::DiffImportant
    } else {
        PaneCell::DiffUnimportant
    }
}

/// The three thumbnail levels: nothing, a plain difference, an important one.
pub const THUMB_LEVEL_NONE: u8 = 0;
pub const THUMB_LEVEL_DIFF: u8 = 1;
pub const THUMB_LEVEL_IMPORTANT: u8 = 2;

/// `CompareThumbnail.image`'s per-pixel diff level map (display-order rows).
pub fn thumbnail_levels(
    rows: &[CompareRow],
    height: usize,
    ignore_unimportant: bool,
) -> Vec<u8> {
    let mut level = vec![THUMB_LEVEL_NONE; height];
    let n = rows.len();
    if n == 0 || height == 0 {
        return level;
    }
    for (d, row) in rows.iter().enumerate() {
        if !is_diff_row(row, ignore_unimportant) {
            continue;
        }
        let y0 = d * height / n;
        let y1 = (((d + 1) * height) / n).max(y0 + 1).min(height);
        let v = if row.important { THUMB_LEVEL_IMPORTANT } else { THUMB_LEVEL_DIFF };
        for slot in level.iter_mut().take(y1).skip(y0) {
            if *slot < v {
                *slot = v;
            }
        }
    }
    level
}

/// The drawing model handed to [`macos::ComparePaneView`] — built from a
/// [`CompareSession`] or [`TextCompare`], never by calling the backend.
#[derive(Clone, Debug)]
pub struct PaneModel {
    pub rows: Vec<CompareRow>,
    pub left_lines: Vec<String>,
    pub right_lines: Vec<String>,
    /// Display order as model-row indices; empty means identity.
    pub display: Vec<usize>,
    /// Model-row indices carrying an alignment anchor.
    pub anchor_rows: Vec<usize>,
    pub cursor: usize,
    pub focus: CompareSide,
    pub ignore_unimportant: bool,
    pub left_name: String,
    pub right_name: String,
    pub mode: String,
    pub section: String,
    /// `summary.stringValue` — the header summary (`CompareSession::summary`).
    pub summary: String,
    /// The summary reads as "no differences" (Swift tints it success).
    pub identical: bool,
    /// The first display row drawn at the top of the body (the scroll view's
    /// offset in rows; kept so the cursor stays visible).
    pub top: usize,
}

impl Default for PaneModel {
    fn default() -> Self {
        PaneModel::empty()
    }
}

impl PaneModel {
    pub fn empty() -> Self {
        PaneModel {
            rows: Vec::new(),
            left_lines: Vec::new(),
            right_lines: Vec::new(),
            display: Vec::new(),
            anchor_rows: Vec::new(),
            cursor: 0,
            focus: CompareSide::Left,
            ignore_unimportant: false,
            left_name: String::new(),
            right_name: String::new(),
            mode: CompareFilter::All.title().to_string(),
            section: String::new(),
            summary: String::new(),
            identical: false,
            top: 0,
        }
    }

    /// Snapshot a live [`CompareSession`] (no helper calls).
    pub fn from_session(s: &CompareSession) -> Self {
        let m = &s.model;
        let display: Vec<usize> = match s.visible() {
            Some(v) => v.to_vec(),
            None => (0..m.rows().len()).collect(),
        };
        let anchor_rows: Vec<usize> =
            (0..m.rows().len()).filter(|i| m.is_anchor(*i)).collect();
        let section = m
            .section_at(s.model_row(s.cursor))
            .map(|i| format!("section {}/{}", i + 1, m.sections().len()))
            .unwrap_or_default();
        PaneModel {
            rows: m.rows().to_vec(),
            left_lines: m.left.lines.clone(),
            right_lines: m.right.lines.clone(),
            display,
            anchor_rows,
            cursor: s.cursor,
            focus: s.focus,
            ignore_unimportant: m.ignore_unimportant,
            left_name: s.name(CompareSide::Left),
            right_name: s.name(CompareSide::Right),
            mode: s.filter.title().to_string(),
            section,
            summary: String::new(),
            identical: m.sections().is_empty() && !s.is_binary(),
            top: 0,
        }
    }

    /// Keep `cursor` inside a body of `visible` rows: the smallest scroll from
    /// `top` (Swift's `scrollRowToVisible`), clamped to the content.
    pub fn scroll_top(top: usize, cursor: usize, visible: usize, count: usize) -> usize {
        let visible = visible.max(1);
        let mut t = top;
        if cursor < t {
            t = cursor;
        } else if cursor >= t + visible {
            t = cursor + 1 - visible;
        }
        t.min(count.saturating_sub(visible))
    }

    pub fn display_count(&self) -> usize {
        if self.display.is_empty() {
            self.rows.len()
        } else {
            self.display.len()
        }
    }

    /// `CompareSession.modelRow(_:)`.
    pub fn model_row(&self, display_row: usize) -> usize {
        if self.display.is_empty() {
            display_row.min(self.rows.len().saturating_sub(1))
        } else {
            self.display
                .get(display_row)
                .copied()
                .or_else(|| self.display.last().copied())
                .unwrap_or(0)
        }
    }

    pub fn is_anchor_row(&self, model_row: usize) -> bool {
        self.anchor_rows.contains(&model_row)
    }

    pub fn line_text(&self, side: CompareSide, line: i32) -> Option<&str> {
        if line < 0 {
            return None;
        }
        let lines = match side {
            CompareSide::Left => &self.left_lines,
            CompareSide::Right => &self.right_lines,
        };
        lines.get(line as usize).map(String::as_str)
    }

    pub fn max_lines(&self) -> usize {
        self.left_lines.len().max(self.right_lines.len())
    }
}

/// `CompareFolderView.FolderTreeView.size(_:)` — the human byte label.
pub fn folder_size_label(bytes: i64) -> String {
    let b = bytes.max(0);
    if b < 1024 {
        return format!("{b} B");
    }
    let units = ["KB", "MB", "GB", "TB"];
    let mut v = b as f64 / 1024.0;
    let mut i = 0;
    while v >= 1024.0 && i < units.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if v < 10.0 {
        format!("{v:.1} {}", units[i])
    } else {
        format!("{v:.0} {}", units[i])
    }
}

/// `CompareFolderView.FolderTreeView.glyph(_:)`.
pub fn folder_glyph(
    status: FolderStatus,
    is_dir: bool,
    kind_mismatch: bool,
    newer: FolderNewer,
) -> &'static str {
    match status {
        FolderStatus::Same => "=",
        FolderStatus::Different => {
            if is_dir && !kind_mismatch {
                "≠"
            } else {
                match newer {
                    FolderNewer::Left => ">",
                    FolderNewer::Right => "<",
                    FolderNewer::None => "≠",
                }
            }
        }
        FolderStatus::Unimportant => "≈",
        FolderStatus::LeftOnly | FolderStatus::RightOnly => "",
        FolderStatus::Unknown => "…",
        FolderStatus::Error => "!",
    }
}

/// The theme-token class `FolderTreeView.color(_:_:)` returns.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FolderColor {
    Text,
    Dim,
    Danger,
    Info,
    Accent2,
    Warning,
}

/// `FolderTreeView.color(_:_:)` as a pure classification.
pub fn folder_row_color(
    status: FolderStatus,
    newer: FolderNewer,
    is_dir: bool,
    side: CompareSide,
) -> FolderColor {
    match status {
        FolderStatus::Same => FolderColor::Text,
        FolderStatus::Different => {
            if newer != FolderNewer::None
                && !is_dir
                && (newer == FolderNewer::Left) != (side == CompareSide::Left)
            {
                FolderColor::Dim
            } else {
                FolderColor::Danger
            }
        }
        FolderStatus::Unimportant => FolderColor::Info,
        FolderStatus::LeftOnly | FolderStatus::RightOnly => FolderColor::Accent2,
        FolderStatus::Unknown => FolderColor::Dim,
        FolderStatus::Error => FolderColor::Warning,
    }
}

/// One rendered folder-tree row (`FolderTreeView` reads this, not the backend).
#[derive(Clone, Debug, PartialEq)]
pub struct FolderTreeRow {
    pub id: usize,
    pub depth: usize,
    pub name: String,
    pub rel: String,
    pub is_dir: bool,
    pub kind_mismatch: bool,
    pub status: FolderStatus,
    pub newer: FolderNewer,
    pub left: Option<FolderSideInfo>,
    pub right: Option<FolderSideInfo>,
}

impl FolderTreeRow {
    /// The name shown for the row: the relative path when flattened.
    pub fn display_name(&self, flatten: bool) -> &str {
        if flatten {
            &self.rel
        } else {
            &self.name
        }
    }

    pub fn glyph(&self) -> &'static str {
        folder_glyph(self.status, self.is_dir, self.kind_mismatch, self.newer)
    }
}

/// The drawing model handed to [`macos::FolderTreeView`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FolderTreeModel {
    pub rows: Vec<FolderTreeRow>,
    pub counts: FolderTreeCounts,
    pub cursor: usize,
    pub flatten: bool,
    pub name_filter: String,
    /// First row drawn under the header (scroll offset in rows).
    pub top: usize,
}

impl FolderTreeModel {
    /// Build from a [`FolderTree`] + its already-computed rows and counts (no
    /// helper/backend calls).
    pub fn build(
        tree: &FolderTree,
        rows: &[FolderRow],
        counts: FolderTreeCounts,
        view: &FolderView,
    ) -> Self {
        let rows = rows
            .iter()
            .filter_map(|r| {
                let n = tree.node(r.id)?;
                Some(FolderTreeRow {
                    id: n.id,
                    depth: r.depth,
                    name: n.name.clone(),
                    rel: n.rel.clone(),
                    is_dir: n.is_dir(),
                    kind_mismatch: n.kind_mismatch(),
                    status: n.status,
                    newer: n.newer,
                    left: n.left.clone(),
                    right: n.right.clone(),
                })
            })
            .collect();
        FolderTreeModel {
            rows,
            counts,
            cursor: 0,
            flatten: view.flatten,
            name_filter: view.name_filter.clone(),
            top: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The counts strip shown in the header.
    pub fn summary(&self) -> String {
        let c = &self.counts;
        let mut parts = Vec::new();
        if c.different > 0 {
            parts.push(format!("{} different", c.different));
        }
        if c.unimportant > 0 {
            parts.push(format!("{} unimportant", c.unimportant));
        }
        if c.left_only > 0 {
            parts.push(format!("{} left only", c.left_only));
        }
        if c.right_only > 0 {
            parts.push(format!("{} right only", c.right_only));
        }
        if c.same > 0 {
            parts.push(format!("{} same", c.same));
        }
        if c.error > 0 {
            parts.push(format!("{} errors", c.error));
        }
        if parts.is_empty() {
            "no items".to_string()
        } else {
            parts.join(" · ")
        }
    }
}

/// `ComparePaneView`'s cursor/selection is fine at 0; the folder header sits at
/// the top of the tree view.
pub const FOLDER_ROW_HEIGHT: f64 = 22.0;
pub const FOLDER_HEADER_HEIGHT: f64 = 22.0;
pub const FOLDER_INDENT: f64 = 14.0;

/// `CompareEditor` key routing delegated to the shared pure engine
/// (`TextEditKeys`); extracted so it is testable without AppKit.
pub fn editor_edit_action(
    key_code: u16,
    mods: crate::engines::text_edit_keys::Modifiers,
    focus: crate::engines::text_edit_keys::Focus,
) -> Option<crate::engines::text_edit_keys::EditAction> {
    crate::engines::text_edit_keys::route(key_code, mods, focus)
}

// ---------------------------------------------------------------------------
// The embeddable AppKit content view (`CompareWindow`'s pane build)
// ---------------------------------------------------------------------------

/// The embeddable content view for the shared host window.
///
/// Returns a [`macos::ComparePaneView`] rooted tree showing an empty two-pane
/// compare with its header, built from model values only (no helper calls).
#[cfg(target_os = "macos")]
pub fn build_content(
    mtm: objc2::MainThreadMarker,
) -> Option<objc2::rc::Retained<objc2_app_kit::NSView>> {
    let view = macos::ComparePaneView::create(mtm);
    view.set_model(PaneModel::empty());
    Some(view.into_super())
}

/// The live compare surface for the shared window: the text pane and the
/// folder tree, repainted from the [`CompareWindowModel`] whenever its
/// [`CompareWindowModel::draw_fingerprint`] changes (Swift's `syncAll`).
#[cfg(target_os = "macos")]
pub struct CompareSurface {
    root: objc2::rc::Retained<objc2_app_kit::NSView>,
    pane: objc2::rc::Retained<macos::ComparePaneView>,
    tree: objc2::rc::Retained<macos::FolderTreeView>,
    last: std::cell::Cell<Option<(u64, i64)>>,
    pane_top: std::cell::Cell<usize>,
    tree_top: std::cell::Cell<usize>,
}

#[cfg(target_os = "macos")]
impl CompareSurface {
    pub fn build(mtm: objc2::MainThreadMarker) -> Self {
        use objc2::MainThreadOnly;
        use objc2_app_kit::{NSAutoresizingMaskOptions as M, NSView};
        use objc2_foundation::{NSPoint, NSRect, NSSize};
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(900.0, 600.0));
        let root = NSView::initWithFrame(NSView::alloc(mtm), frame);
        let pane = macos::ComparePaneView::create(mtm);
        let tree = macos::FolderTreeView::create(mtm);
        for v in [&*pane as &NSView, &*tree as &NSView] {
            v.setFrame(frame);
            v.setAutoresizingMask(M::ViewWidthSizable | M::ViewHeightSizable);
            root.addSubview(v);
        }
        pane.set_model(PaneModel::empty());
        tree.setHidden(true);
        CompareSurface {
            root,
            pane,
            tree,
            last: std::cell::Cell::new(None),
            pane_top: std::cell::Cell::new(0),
            tree_top: std::cell::Cell::new(0),
        }
    }

    pub fn content_view(&self) -> objc2::rc::Retained<objc2_app_kit::NSView> {
        self.root.clone()
    }

    /// Repaint from `model` when anything drawn changed (or the height did).
    /// Writes the live scroll offset back as `scrollY` (`state.current`).
    pub fn sync(&self, model: &mut CompareWindowModel) {
        let height = self.root.bounds().size.height;
        let key = (model.draw_fingerprint(), height as i64);
        if self.last.get() == Some(key) {
            return;
        }
        self.last.set(Some(key));
        if model.current_is_folder() {
            self.pane.setHidden(true);
            self.tree.setHidden(false);
            let m = match model.folder_page.session.as_ref() {
                Some(fs) => match fs.tree.as_ref() {
                    Some(t) => {
                        let mut m = FolderTreeModel::build(t, &fs.rows, t.counts(), &fs.view);
                        m.cursor = fs.cursor;
                        let vis = ((height - FOLDER_HEADER_HEIGHT) / FOLDER_ROW_HEIGHT).floor() as usize;
                        m.top = PaneModel::scroll_top(self.tree_top.get(), fs.cursor, vis, m.rows.len());
                        self.tree_top.set(m.top);
                        m
                    }
                    None => FolderTreeModel::default(),
                },
                None => FolderTreeModel::default(),
            };
            self.tree.set_model(m);
            return;
        }
        self.tree.setHidden(true);
        self.pane.setHidden(false);
        let summary = model.current().map(|s| s.summary(&model.config)).unwrap_or_default();
        let Some(s) = model.current_mut() else {
            self.pane_top.set(0);
            self.pane.set_model(PaneModel::empty());
            return;
        };
        let mut m = PaneModel::from_session(s);
        m.summary = summary;
        let vis = ((height - PANE_HEADER_HEIGHT) / PANE_ROW_HEIGHT).floor() as usize;
        m.top = PaneModel::scroll_top(self.pane_top.get(), s.cursor, vis, m.display_count());
        self.pane_top.set(m.top);
        s.scroll_y = m.top as f64 * PANE_ROW_HEIGHT;
        self.pane.set_model(m);
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use crate::ui::theme::{PopupColors, PopupTone, Rgba};
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly, Message};
    use objc2_app_kit::{
        NSAutoresizingMaskOptions, NSBezierPath, NSBorderType, NSColor, NSEvent,
        NSEventModifierFlags, NSFont, NSFontAttributeName, NSFontWeightRegular, NSFontWeightSemibold,
        NSForegroundColorAttributeName, NSScrollView, NSStringDrawing, NSTextView, NSView,
    };
    use objc2_foundation::{
        NSAttributedStringKey, NSDictionary, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
    };
    use std::cell::{Cell, RefCell};

    fn as_any<T: Message + ?Sized>(obj: &T) -> &AnyObject {
        unsafe { &*(obj as *const T as *const AnyObject) }
    }

    fn nsrect(r: Rect) -> NSRect {
        NSRect::new(NSPoint::new(r.x, r.y), NSSize::new(r.width, r.height))
    }

    fn fill(r: Rect, c: Rgba) {
        c.to_nscolor().setFill();
        NSBezierPath::bezierPathWithRect(nsrect(r)).fill();
    }

    fn stroke_rect(r: Rect, c: Rgba, width: f64) {
        let path = NSBezierPath::bezierPathWithRect(nsrect(r));
        path.setLineWidth(width);
        c.to_nscolor().setStroke();
        path.stroke();
    }

    fn mono_font() -> Retained<NSFont> {
        NSFont::monospacedSystemFontOfSize_weight(13.0, unsafe { NSFontWeightRegular })
    }

    /// `[.font: font, .foregroundColor: color]` as an attribute dictionary.
    fn text_attrs(
        font: &NSFont,
        color: &NSColor,
    ) -> Retained<NSDictionary<NSAttributedStringKey, AnyObject>> {
        let font_obj: &AnyObject = as_any(font);
        let color_obj: &AnyObject = as_any(color);
        let keys: [&NSAttributedStringKey; 2] =
            unsafe { [NSFontAttributeName, NSForegroundColorAttributeName] };
        let objs: [&AnyObject; 2] = [font_obj, color_obj];
        NSDictionary::from_slices(&keys, &objs)
    }

    fn draw_text(s: &str, x: f64, y: f64, attrs: &NSDictionary<NSAttributedStringKey, AnyObject>) {
        let text = NSString::from_str(s);
        unsafe { text.drawAtPoint_withAttributes(NSPoint::new(x, y), Some(attrs)) };
    }

    fn measure_text(s: &str, attrs: &NSDictionary<NSAttributedStringKey, AnyObject>) -> f64 {
        let text = NSString::from_str(s);
        unsafe { text.sizeWithAttributes(Some(attrs)).width }
    }

    fn folder_color(colors: &PopupColors, c: FolderColor) -> Rgba {
        match c {
            FolderColor::Text => colors.text,
            FolderColor::Dim => colors.dim,
            FolderColor::Danger => colors.tone(PopupTone::Danger),
            FolderColor::Info => colors.tone(PopupTone::Info),
            FolderColor::Accent2 => colors.tone(PopupTone::Accent2),
            FolderColor::Warning => colors.tone(PopupTone::Warning),
        }
    }

    // -- ComparePaneView ----------------------------------------------------

    pub struct ComparePaneViewIvars {
        pub colors: RefCell<PopupColors>,
        pub model: RefCell<PaneModel>,
    }

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSComparePaneView"]
        #[ivars = ComparePaneViewIvars]
        pub struct ComparePaneView;

        impl ComparePaneView {
            #[unsafe(method(isFlipped))]
            fn is_flipped(&self) -> bool {
                true
            }

            #[unsafe(method(drawRect:))]
            fn draw_rect(&self, dirty: NSRect) {
                self.render(dirty);
            }
        }

        unsafe impl NSObjectProtocol for ComparePaneView {}
    );

    impl ComparePaneView {
        pub fn create(mtm: MainThreadMarker) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(ComparePaneViewIvars {
                colors: RefCell::new(PopupColors::default()),
                model: RefCell::new(PaneModel::empty()),
            });
            unsafe {
                msg_send![
                    super(this),
                    initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0))
                ]
            }
        }

        pub fn set_colors(&self, colors: PopupColors) {
            *self.ivars().colors.borrow_mut() = colors;
            self.setNeedsDisplay(true);
        }

        pub fn set_model(&self, model: PaneModel) {
            *self.ivars().model.borrow_mut() = model;
            self.setNeedsDisplay(true);
        }

        fn render(&self, dirty: NSRect) {
            let colors = *self.ivars().colors.borrow();
            let model = self.ivars().model.borrow();
            let b = self.bounds();
            let layout = PaneLayout::new(b.size.width, b.size.height);
            fill(Rect::new(0.0, 0.0, b.size.width, b.size.height), colors.base());

            // Header strip.
            fill(Rect::new(0.0, 0.0, b.size.width, layout.header), colors.mantle());
            fill(
                Rect::new(0.0, (layout.header - 1.0).max(0.0), b.size.width, 1.0),
                colors.hairline(),
            );
            let hfont = NSFont::systemFontOfSize_weight(12.0, unsafe { NSFontWeightSemibold });
            let hattrs = text_attrs(&hfont, &colors.accent_on().to_nscolor());
            let title = if model.section.is_empty() {
                model.mode.clone()
            } else {
                format!("{}  ·  {}", model.mode, model.section)
            };
            draw_text(&title, 10.0, 6.0, &hattrs);
            let nfont = NSFont::systemFontOfSize(11.0);
            let nattrs = text_attrs(&nfont, &colors.dim.to_nscolor());
            let names = format!("{}  ⇆  {}", model.left_name, model.right_name);
            let nw = measure_text(&names, &nattrs);
            draw_text(&names, (b.size.width - nw - 10.0).max(10.0), 6.0, &nattrs);
            // `summary` label: success-tinted when there are no differences.
            if !model.summary.is_empty() {
                let tone = if model.identical {
                    colors.tone(PopupTone::Success)
                } else {
                    colors.text
                };
                let sattrs = text_attrs(&nfont, &tone.to_nscolor());
                let tw = measure_text(&title, &hattrs);
                draw_text(&model.summary, 10.0 + tw + 16.0, 6.5, &sattrs);
            }

            // Centre gutter.
            let pane_w = layout.pane_w();
            let body_h = (b.size.height - layout.header).max(0.0);
            fill(
                Rect::new(pane_w, layout.header, layout.gutter, body_h),
                colors.mantle(),
            );

            let n = model.display_count();
            let max_lines = model.max_lines();
            if n == 0 {
                let msg = if model.rows.is_empty() {
                    "(no differences)"
                } else {
                    "(filtered)"
                };
                let f = NSFont::systemFontOfSize(12.0);
                let a = text_attrs(&f, &colors.dim.to_nscolor());
                let w = measure_text(msg, &a);
                draw_text(
                    msg,
                    ((b.size.width - w) / 2.0).max(0.0),
                    layout.header + body_h / 2.0 - 8.0,
                    &a,
                );
            } else {
                let first = layout.row_at(dirty.origin.y) + model.top;
                let last = layout.row_at(dirty.origin.y + dirty.size.height) + model.top;
                let lo = first.min(n - 1);
                let hi = last.min(n - 1);
                if lo <= hi {
                    let mono = mono_font();
                    for d in lo..=hi {
                        self.draw_row(d, &model, &layout, &colors, &mono, max_lines);
                    }
                }
            }

            // Pane separators.
            fill(
                Rect::new(pane_w, dirty.origin.y, 1.0, dirty.size.height),
                colors.hairline(),
            );
            fill(
                Rect::new(pane_w + layout.gutter - 1.0, dirty.origin.y, 1.0, dirty.size.height),
                colors.hairline(),
            );
        }

        fn draw_row(
            &self,
            d: usize,
            model: &PaneModel,
            layout: &PaneLayout,
            colors: &PopupColors,
            mono: &NSFont,
            max_lines: usize,
        ) {
            let m = model.model_row(d);
            let Some(row) = model.rows.get(m) else {
                return;
            };
            let Some(rel) = d.checked_sub(model.top) else {
                return;
            };
            let y = layout.row_y(rel);
            let cursor = d == model.cursor;
            let line_no_w = layout.line_no_width(max_lines);

            for side in [CompareSide::Left, CompareSide::Right] {
                let px = layout.pane_x(side);
                let rect = Rect::new(px, y, layout.pane_w(), layout.row_height);
                let cell = pane_cell(row, side, model.ignore_unimportant);
                let mut under = colors.base();
                match cell {
                    PaneCell::Empty => {
                        under = colors.mantle();
                        fill(rect, under);
                    }
                    PaneCell::Same | PaneCell::Ignored => {}
                    PaneCell::DiffImportant | PaneCell::DiffUnimportant => {
                        let hue = if cell == PaneCell::DiffImportant {
                            colors.tone(PopupTone::Danger)
                        } else {
                            colors.tone(PopupTone::Info)
                        };
                        let a = if cell == PaneCell::DiffImportant { 0.15 } else { 0.13 };
                        fill(rect, hue.with_alpha(a));
                        under = colors.over(hue, a, colors.base());
                    }
                }
                if cursor {
                    let on = side == model.focus;
                    let a = if on { 0.55 } else { 0.22 };
                    fill(rect, colors.highlight.with_alpha(a));
                    under = colors.over(colors.highlight, a, under);
                    if on {
                        fill(Rect::new(px, y, 3.0, layout.row_height), colors.accent_on());
                    }
                }

                let line = row.line(side);
                if line < 0 {
                    continue;
                }
                let num = format!("{}", line + 1);
                let num_attrs = text_attrs(mono, &colors.ensure(colors.dim, under, 4.5).to_nscolor());
                let nw = measure_text(&num, &num_attrs);
                draw_text(&num, px + line_no_w - nw - 4.0, y + 2.0, &num_attrs);

                if let Some(text) = model.line_text(side, line) {
                    if !text.is_empty() {
                        let tattrs =
                            text_attrs(mono, &colors.ensure(colors.text, under, 4.5).to_nscolor());
                        draw_text(text, layout.text_x(side, max_lines), y + 2.0, &tattrs);
                    }
                }
            }

            if model.is_anchor_row(m) {
                let c = colors.tone(PopupTone::Accent2).with_alpha(0.8);
                fill(Rect::new(0.0, y, self.bounds().size.width, 1.5), c);
            }
        }
    }

    // -- CompareThumbnail ---------------------------------------------------

    pub struct CompareThumbnailIvars {
        pub colors: RefCell<PopupColors>,
        pub rows: RefCell<Vec<CompareRow>>,
        pub ignore_unimportant: Cell<bool>,
        /// Viewport as `(top fraction, height fraction)`.
        pub visible: Cell<(f64, f64)>,
    }

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSCompareThumbnail"]
        #[ivars = CompareThumbnailIvars]
        pub struct CompareThumbnail;

        impl CompareThumbnail {
            #[unsafe(method(isFlipped))]
            fn is_flipped(&self) -> bool {
                true
            }

            #[unsafe(method(drawRect:))]
            fn draw_rect(&self, _dirty: NSRect) {
                self.render();
            }
        }

        unsafe impl NSObjectProtocol for CompareThumbnail {}
    );

    impl CompareThumbnail {
        pub fn create(mtm: MainThreadMarker) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(CompareThumbnailIvars {
                colors: RefCell::new(PopupColors::default()),
                rows: RefCell::new(Vec::new()),
                ignore_unimportant: Cell::new(false),
                visible: Cell::new((0.0, 1.0)),
            });
            unsafe {
                msg_send![
                    super(this),
                    initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(10.0, 100.0))
                ]
            }
        }

        pub fn set_colors(&self, colors: PopupColors) {
            *self.ivars().colors.borrow_mut() = colors;
            self.setNeedsDisplay(true);
        }

        pub fn set_rows(&self, rows: Vec<CompareRow>, ignore_unimportant: bool) {
            *self.ivars().rows.borrow_mut() = rows;
            self.ivars().ignore_unimportant.set(ignore_unimportant);
            self.setNeedsDisplay(true);
        }

        pub fn set_visible_fraction(&self, top: f64, height: f64) {
            self.ivars().visible.set((top.clamp(0.0, 1.0), height.clamp(0.0, 1.0)));
            self.setNeedsDisplay(true);
        }

        fn render(&self) {
            let colors = *self.ivars().colors.borrow();
            let b = self.bounds();
            fill(Rect::new(0.0, 0.0, b.size.width, b.size.height), colors.mantle());
            let h = b.size.height.max(1.0) as usize;
            let rows = self.ivars().rows.borrow();
            let levels = thumbnail_levels(&rows, h, self.ivars().ignore_unimportant.get());
            for (y, level) in levels.iter().enumerate() {
                if *level == THUMB_LEVEL_NONE {
                    continue;
                }
                let c = if *level >= THUMB_LEVEL_IMPORTANT {
                    colors.tone(PopupTone::Danger)
                } else {
                    colors.tone(PopupTone::Info)
                };
                fill(Rect::new(2.0, y as f64, (b.size.width - 4.0).max(0.0), 1.0), c);
            }
            let (top, frac) = self.ivars().visible.get();
            let vh = (frac * b.size.height).max(4.0);
            let vy = (top * b.size.height).clamp(0.0, (b.size.height - vh).max(0.0));
            stroke_rect(
                Rect::new(0.5, vy + 0.5, (b.size.width - 1.0).max(0.0), (vh - 1.0).max(1.0)),
                colors.accent_on().with_alpha(0.8),
                1.0,
            );
        }
    }

    // -- CompareEditor ------------------------------------------------------

    pub struct CompareEditorIvars {
        pub side: Cell<CompareSide>,
        pub original: RefCell<String>,
        pub border_color: RefCell<Rgba>,
    }

    define_class!(
        #[unsafe(super(NSTextView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSCompareEditor"]
        #[ivars = CompareEditorIvars]
        pub struct CompareEditor;

        impl CompareEditor {
            #[unsafe(method(drawRect:))]
            fn draw_rect(&self, dirty: NSRect) {
                let _: () = unsafe { msg_send![super(self), drawRect: dirty] };
                let b = self.bounds();
                stroke_rect(
                    Rect::new(b.origin.x, b.origin.y, b.size.width, b.size.height),
                    *self.ivars().border_color.borrow(),
                    1.5,
                );
            }

            #[unsafe(method(keyDown:))]
            fn key_down(&self, event: &NSEvent) {
                use crate::engines::text_edit_keys::{EditAction, Focus, Modifiers};
                let flags = event.modifierFlags();
                let mods = Modifiers {
                    ctrl: flags.contains(NSEventModifierFlags::Control),
                    cmd: flags.contains(NSEventModifierFlags::Command),
                    option: flags.contains(NSEventModifierFlags::Option),
                    shift: flags.contains(NSEventModifierFlags::Shift),
                };
                let focus = Focus { is_text: true, is_editable: self.isEditable() };
                if editor_edit_action(event.keyCode(), mods, focus) == Some(EditAction::KillWordBackward)
                {
                    let nil: Option<&AnyObject> = None;
                    let _: () = unsafe { msg_send![self, deleteWordBackward: nil] };
                    return;
                }
                let _: () = unsafe { msg_send![super(self), keyDown: event] };
            }
        }

        unsafe impl NSObjectProtocol for CompareEditor {}
    );

    impl CompareEditor {
        pub fn create(mtm: MainThreadMarker) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(CompareEditorIvars {
                side: Cell::new(CompareSide::Left),
                original: RefCell::new(String::new()),
                border_color: RefCell::new(PopupColors::default().accent_on()),
            });
            let view: Retained<Self> = unsafe {
                msg_send![
                    super(this),
                    initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(320.0, 120.0))
                ]
            };
            view.setEditable(true);
            view.setRichText(false);
            view.setAllowsUndo(true);
            view.setVerticallyResizable(true);
            view.setHorizontallyResizable(false);
            view.setFont(Some(&mono_font()));
            view
        }

        pub fn set_side(&self, side: CompareSide) {
            self.ivars().side.set(side);
        }

        pub fn side(&self) -> CompareSide {
            self.ivars().side.get()
        }

        pub fn set_original(&self, text: &str) {
            *self.ivars().original.borrow_mut() = text.to_string();
        }

        pub fn original(&self) -> String {
            self.ivars().original.borrow().clone()
        }

        /// `string` after normalising CR/CRLF and dropping a trailing newline
        /// (mirrors `CompareEditor.editedLines` for a single side).
        pub fn set_text(&self, text: &str) {
            self.setString(&NSString::from_str(text));
        }

        pub fn text(&self) -> String {
            self.string().to_string()
        }

        pub fn set_colors(&self, colors: PopupColors) {
            *self.ivars().border_color.borrow_mut() = colors.accent_on();
            let text = colors.text.to_nscolor();
            self.setTextColor(Some(&text));
            self.setInsertionPointColor(Some(&text));
            self.setNeedsDisplay(true);
        }

        /// `editedLines`: the text split on normalised newlines, no trailing
        /// empty line.
        pub fn edited_lines(&self) -> Vec<String> {
            super::edited_lines(&self.string().to_string())
        }
    }

    /// Wrap a [`CompareEditor`] in the scroll view the pane uses.
    pub fn editor_scroll(mtm: MainThreadMarker, editor: &CompareEditor) -> Retained<NSScrollView> {
        let scroll = NSScrollView::new(mtm);
        scroll.setBorderType(NSBorderType::NoBorder);
        scroll.setDrawsBackground(false);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setDocumentView(Some(editor));
        scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        scroll
    }

    // -- FolderTreeView -----------------------------------------------------

    pub struct FolderTreeViewIvars {
        pub colors: RefCell<PopupColors>,
        pub model: RefCell<FolderTreeModel>,
    }

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSFolderTreeView"]
        #[ivars = FolderTreeViewIvars]
        pub struct FolderTreeView;

        impl FolderTreeView {
            #[unsafe(method(isFlipped))]
            fn is_flipped(&self) -> bool {
                true
            }

            #[unsafe(method(drawRect:))]
            fn draw_rect(&self, dirty: NSRect) {
                self.render(dirty);
            }
        }

        unsafe impl NSObjectProtocol for FolderTreeView {}
    );

    impl FolderTreeView {
        pub fn create(mtm: MainThreadMarker) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(FolderTreeViewIvars {
                colors: RefCell::new(PopupColors::default()),
                model: RefCell::new(FolderTreeModel::default()),
            });
            unsafe {
                msg_send![
                    super(this),
                    initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0))
                ]
            }
        }

        pub fn set_colors(&self, colors: PopupColors) {
            *self.ivars().colors.borrow_mut() = colors;
            self.setNeedsDisplay(true);
        }

        pub fn set_model(&self, model: FolderTreeModel) {
            *self.ivars().model.borrow_mut() = model;
            self.setNeedsDisplay(true);
        }

        fn render(&self, dirty: NSRect) {
            let colors = *self.ivars().colors.borrow();
            let model = self.ivars().model.borrow();
            let b = self.bounds();
            fill(Rect::new(0.0, 0.0, b.size.width, b.size.height), colors.base());
            fill(Rect::new(0.0, 0.0, b.size.width, FOLDER_HEADER_HEIGHT), colors.mantle());
            fill(
                Rect::new(0.0, FOLDER_HEADER_HEIGHT - 1.0, b.size.width, 1.0),
                colors.hairline(),
            );

            let hfont = NSFont::systemFontOfSize(11.0);
            let hattrs = text_attrs(&hfont, &colors.dim.to_nscolor());
            draw_text(&model.summary(), 10.0, 4.0, &hattrs);

            if model.is_empty() {
                let msg = "(empty)";
                let f = NSFont::systemFontOfSize(12.0);
                let a = text_attrs(&f, &colors.dim.to_nscolor());
                let w = measure_text(msg, &a);
                draw_text(
                    msg,
                    ((b.size.width - w) / 2.0).max(0.0),
                    FOLDER_HEADER_HEIGHT + (b.size.height - FOLDER_HEADER_HEIGHT) / 2.0 - 8.0,
                    &a,
                );
                return;
            }

            let mono = NSFont::systemFontOfSize(12.0);
            for (i, row) in model.rows.iter().enumerate().skip(model.top) {
                let y = FOLDER_HEADER_HEIGHT + (i - model.top) as f64 * FOLDER_ROW_HEIGHT;
                if y + FOLDER_ROW_HEIGHT < dirty.origin.y || y > dirty.origin.y + dirty.size.height
                {
                    continue;
                }
                if i == model.cursor {
                    let pill = Rect::new(2.0, y + 1.0, (b.size.width - 4.0).max(0.0), FOLDER_ROW_HEIGHT - 2.0);
                    let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                        nsrect(pill),
                        5.0,
                        5.0,
                    );
                    colors.highlight.with_alpha(0.6).to_nscolor().setFill();
                    path.fill();
                }
                let indent = 10.0 + row.depth as f64 * FOLDER_INDENT;
                let color = folder_color(&colors, folder_row_color(row.status, row.newer, row.is_dir, CompareSide::Left));
                let gattrs = text_attrs(&mono, &color.to_nscolor());
                draw_text(row.glyph(), indent, y + 3.0, &gattrs);
                let name = row.display_name(model.flatten);
                let nattrs = text_attrs(&mono, &color.to_nscolor());
                draw_text(name, indent + 16.0, y + 3.0, &nattrs);
                if let Some(info) = &row.left {
                    if !info.is_dir {
                        let s = folder_size_label(info.size);
                        let a = text_attrs(&hfont, &colors.dim.to_nscolor());
                        let w = measure_text(&s, &a);
                        draw_text(&s, (b.size.width - w - 10.0).max(0.0), y + 4.0, &a);
                    }
                }
            }
        }
    }
}

/// Register the `do:compare:*` hook table (mirrors `compareTestDo` /
/// `CompareWindow.testDo`) and the palette row.
pub fn register_compare_hooks(
    registry: &mut crate::app::registry::Registry,
    model: Arc<Mutex<CompareWindowModel>>,
) {
    registry.register_test_do(move |action: &str| {
        let rest = action.strip_prefix("compare:")?;
        let mut m = model.lock().unwrap();
        match m.test_do(rest) {
            None => Some(Value::Null),
            Some(err) => Some(json!({"error": err})),
        }
    });
    registry.add_palette(crate::app::registry::PaletteCommand::new("compare", "Compare", "views"));
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::process;

    #[derive(Debug, Default)]
    struct Stub {
        calls: Mutex<Vec<(String, Value)>>,
        replies: Mutex<HashMap<String, VecDeque<Value>>>,
        errors: Mutex<HashSet<String>>,
    }

    impl Stub {
        fn new() -> Arc<Stub> {
            Arc::new(Stub::default())
        }
        fn push(&self, method: &str, v: Value) {
            self.replies.lock().unwrap().entry(method.to_string()).or_default().push_back(v);
        }
        fn fail(&self, method: &str) {
            self.errors.lock().unwrap().insert(method.to_string());
        }
        fn calls(&self) -> Vec<(String, Value)> {
            self.calls.lock().unwrap().clone()
        }
        fn count(&self, method: &str) -> usize {
            self.calls().into_iter().filter(|(m, _)| m == method).count()
        }
        fn params(&self, method: &str) -> Value {
            self.calls().into_iter().find(|(m, _)| m == method).map(|(_, p)| p).unwrap()
        }
    }

    impl HelperBackend for Stub {
        fn call(&self, method: &str, params: Value, _t: u64) -> Result<Value, String> {
            self.calls.lock().unwrap().push((method.to_string(), params));
            if self.errors.lock().unwrap().contains(method) {
                return Err("stub error".to_string());
            }
            if let Some(q) = self.replies.lock().unwrap().get_mut(method) {
                if let Some(v) = q.pop_front() {
                    return Ok(v);
                }
            }
            Ok(json!({}))
        }
    }

    fn side(l: &str, r: &str) -> TextSide {
        TextSide {
            lines: vec![l.to_string(), r.to_string()],
            eols: vec![Eol::Lf, Eol::Lf],
            encoding: TextEncodingKind::Utf8,
        }
    }

    fn new_snapshot() -> Value {
        json!({
            "handle": 7,
            "left": side("a", "b").json(),
            "right": side("a", "c").json(),
            "rows": [[0, 0, 0], [1, 1, 5]],
            "sections": [[1, 2, 1]],
            "anchors": [[0, 0]],
            "undo": [0, 0],
            "redo": [0, 0],
            "importance": Importance::default().json(),
            "ignoreUnimportant": false,
        })
    }

    #[test]
    fn text_compare_call_shaping() {
        let stub = Stub::new();
        stub.push("compare.new", new_snapshot());
        let mut tc = TextCompare::with_backend(
            stub.clone(),
            side("a", "b"),
            side("a", "c"),
            Importance::default(),
            false,
        );
        assert_eq!(tc.handle(), 7);
        assert_eq!(stub.count("compare.new"), 1);

        let newp = stub.params("compare.new");
        assert!(newp["left"]["lines"].is_array());
        assert_eq!(newp["ignoreUnimportant"], false);

        tc.replace(CompareSide::Left, 0..1, &["x".to_string()]);
        let p = stub.params("compare.replace");
        assert_eq!(p["handle"], 7);
        assert_eq!(p["side"], "left");
        assert_eq!(p["start"], 0);
        assert_eq!(p["count"], 1);
        assert_eq!(p["lines"], json!(["x"]));

        tc.copy_rows(2..4, CompareSide::Right);
        let p = stub.params("compare.copy_rows");
        assert_eq!(p["lo"], 2);
        assert_eq!(p["hi"], 4);
        assert_eq!(p["from"], "right");

        tc.copy_section(1, CompareSide::Left);
        assert_eq!(stub.params("compare.copy_section")["index"], 1);

        tc.align(3, 5);
        let p = stub.params("compare.align");
        assert_eq!(p["l"], 3);
        assert_eq!(p["r"], 5);

        tc.set_importance(Importance::exact());
        assert_eq!(stub.params("compare.set_importance")["importance"]["leadingWS"], false);

        tc.set_ignore_unimportant(true);
        assert_eq!(stub.params("compare.set_ignore_unimportant")["on"], true);

        // undo guards: the snapshot has no undo counts, so nothing is emitted.
        assert!(!tc.undo(None));
        assert!(!tc.redo());
        assert_eq!(stub.count("compare.undo"), 0);
        assert_eq!(stub.count("compare.redo"), 0);
    }

    #[test]
    fn text_compare_parses_snapshot() {
        let stub = Stub::new();
        stub.push("compare.new", new_snapshot());
        let tc = TextCompare::with_backend(
            stub,
            TextSide::default(),
            TextSide::default(),
            Importance::default(),
            false,
        );
        assert_eq!(tc.rows().len(), 2);
        assert_eq!(tc.rows()[1].kind, RowKind::Changed);
        assert!(tc.rows()[1].important);
        assert_eq!(tc.rows()[0].kind, RowKind::Same);
        assert_eq!(tc.sections().len(), 1);
        assert_eq!(tc.sections()[0].rows, 1..2);
        assert!(tc.sections()[0].important);
        assert_eq!(tc.anchors(), &[(0, 0)]);
        assert!(tc.is_anchor(0));
        assert!(!tc.is_anchor(1));
        assert!(tc.identical_text() == false);
    }

    #[test]
    fn undo_redo_guards_read_counts() {
        let stub = Stub::new();
        let mut snap = new_snapshot();
        snap["undo"] = json!([1, 0]);
        snap["redo"] = json!([0, 1]);
        stub.push("compare.new", snap);
        let mut redo_snap = new_snapshot();
        redo_snap["redo"] = json!([0, 1]);
        stub.push("compare.undo", redo_snap);
        stub.push("compare.redo", new_snapshot());
        let mut tc = TextCompare::with_backend(
            stub.clone(),
            side("a", "b"),
            side("a", "b"),
            Importance::default(),
            false,
        );
        assert!(tc.can_undo());
        assert_eq!(tc.undo_count(CompareSide::Left), 1);
        assert!(tc.undo(Some(CompareSide::Left)));
        assert_eq!(stub.count("compare.undo"), 1);
        assert!(tc.can_redo());
        assert!(tc.redo());
        assert_eq!(stub.count("compare.redo"), 1);
    }

    #[test]
    fn visible_rows_filters() {
        let stub = Stub::new();
        stub.push(
            "compare.new",
            json!({
                "handle": 1,
                "rows": [[0,0,0], [1,1,5], [2,2,1], [3,3,0]],
                "sections": [[1,3,1]],
                "anchors": [], "undo": [0,0], "redo": [0,0],
                "importance": Importance::default().json(), "ignoreUnimportant": false,
            }),
        );
        let tc = TextCompare::with_backend(
            stub,
            TextSide::default(),
            TextSide::default(),
            Importance::default(),
            false,
        );
        assert_eq!(tc.visible_rows(CompareFilter::All, 3), None);
        assert_eq!(tc.visible_rows(CompareFilter::Diffs, 3), Some(vec![1, 2]));
        assert_eq!(tc.visible_rows(CompareFilter::Context, 1), Some(vec![0, 1, 2, 3]));
        assert_eq!(tc.section_at(1), Some(0));
        assert_eq!(tc.section_at(0), None);
        assert_eq!(tc.next_section(0), Some(0));
        assert_eq!(tc.prev_section(3), Some(0));
    }

    #[test]
    fn binary_first_difference_matches_engine() {
        assert_eq!(BinaryCompare::first_difference(&[1, 2, 3], &[1, 2, 3]), None);
        let a = [7u8; 100];
        assert_eq!(
            BinaryCompare::first_difference(&[&a[..], &[1]].concat(), &[&a[..], &[2]].concat()),
            Some(100)
        );
        assert_eq!(BinaryCompare::first_difference(&[1, 2], &[1, 2, 3]), Some(2));
        assert_eq!(BinaryCompare::first_difference(&[], &[9]), Some(0));
    }

    #[test]
    fn side_json_and_eol_labels() {
        let s = TextSide {
            lines: vec!["a".into(), "b".into()],
            eols: vec![Eol::Crlf, Eol::None],
            encoding: TextEncodingKind::Utf8Bom,
        };
        assert_eq!(s.text(), "a\r\nb");
        assert_eq!(s.eol_label(), "CRLF");
        let back = TextSide::from_json(&s.json());
        assert_eq!(back, s);
        assert_eq!(back.encoding, TextEncodingKind::Utf8Bom);

        let mixed = TextSide {
            lines: vec!["a".into(), "b".into()],
            eols: vec![Eol::Lf, Eol::Crlf],
            encoding: TextEncodingKind::Utf8,
        };
        assert_eq!(mixed.eol_label(), "mixed");

        let empty = TextSide::default();
        assert_eq!(empty.eol_label(), "LF");
    }

    #[test]
    fn importance_round_trip() {
        let imp = Importance::exact();
        assert_eq!(Importance::from_json(&imp.json()), imp);
        let d = Importance::from_json(&json!({}));
        assert!(d.leading_ws && d.trailing_ws && d.line_endings);
        assert!(!d.embedded_ws && !d.ignore_case && !d.blank_lines);
        assert_eq!(Importance::default().cache_key().len(), 6);
    }

    #[test]
    fn pasted_side_splits_like_engine() {
        let s = pasted_side("a\r\nb\nc");
        assert_eq!(s.lines, vec!["a", "b", "c"]);
        assert_eq!(s.eols, vec![Eol::Crlf, Eol::Lf, Eol::None]);
        let t = pasted_side("a\nb\n");
        assert_eq!(t.lines, vec!["a", "b"]);
        assert_eq!(t.eols, vec![Eol::Lf, Eol::Lf]);
        assert!(pasted_side("").lines.is_empty());
    }

    fn temp_dir(tag: &str) -> String {
        let p = std::env::temp_dir().join(format!("ws-compare-{tag}-{}", process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn recent_store_round_trip() {
        let dir = temp_dir("recent");
        let r = CompareRecent::new(&dir);
        assert!(r.load().is_empty());
        r.add_at("/a", "/b", 30, 100.0);
        r.add_at("/c", "/d", 30, 200.0);
        // Newest first.
        let all = r.load();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].left, "/c");
        assert_eq!(all[1].left, "/a");
        // Re-adding an existing pair moves it to the front with the new time.
        r.add_at("/a", "/b", 30, 300.0);
        let all = r.load();
        assert_eq!(all[0].left, "/a");
        assert_eq!(all[0].used, 300.0);
        assert_eq!(all[1].left, "/c");

        // A limit of 2 keeps only the two newest.
        r.add_at("/e", "/f", 2, 400.0);
        let all = r.load();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].left, "/e");
        assert_eq!(all[1].left, "/a");

        r.remove(&all[1].clone());
        assert_eq!(r.load().len(), 1);
        assert_eq!(r.load()[0].left, "/e");

        assert!(r.is_pasted(&format!("{}/x.txt", r.pasted_dir())));
        assert!(!r.is_pasted("/tmp/x.txt"));

        let json = std::fs::read_to_string(r.path()).unwrap();
        assert!(json.contains("\"left\": \"/e\""));

        r.clear_all();
        assert!(r.load().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sync_plan_parsing() {
        let stub = Stub::new();
        let backend: Arc<dyn HelperBackend> = stub.clone();
        let tree = FolderTree::new(backend, "/l", "/r", 5);
        stub.push(
            "folder.sync_plan",
            json!({
                "copies": [
                    {"src": "/l/a.txt", "dst": "/r/a.txt", "to": "right", "rel": "a.txt", "replaces": true},
                ],
                "trash": [{"path": "/r/orphan", "side": "right", "rel": "orphan"}],
                "skipped": ["tie.txt"],
            }),
        );
        let p = SyncPlan::make(&tree, SyncMode::UpdateRight, "*.txt");
        assert_eq!(p.copies.len(), 1);
        assert_eq!(p.copies[0].to, CompareSide::Right);
        assert!(p.copies[0].replaces);
        assert_eq!(p.trash[0].side, CompareSide::Right);
        assert_eq!(p.skipped, vec!["tie.txt"]);
        let params = stub.params("folder.sync_plan");
        assert_eq!(params["mode"], "updateRight");
        assert_eq!(params["nameFilter"], "*.txt");
        assert!(!p.is_empty());
    }

    fn tree_snapshot() -> Value {
        json!({
            "handle": 9,
            "caseInsensitive": true,
            "truncated": false,
            "errors": [],
            "roots": [0],
            "nodes": [
                {"id": 0, "key": "root", "rel": "", "name": "root", "children": [1],
                 "left": {"name": "root", "isDir": true}, "right": {"name": "root", "isDir": true},
                 "status": "different", "sameByMetadata": false, "newer": "none", "depth": 0},
                {"id": 1, "key": "a.txt", "rel": "a.txt", "name": "a.txt", "parent": 0, "children": [],
                 "left": {"name": "a.txt", "isDir": false, "size": 3, "mtime": 10.0},
                 "right": {"name": "a.txt", "isDir": false, "size": 4, "mtime": 20.0},
                 "status": "different", "sameByMetadata": false, "newer": "right", "depth": 1}
            ]
        })
    }

    #[test]
    fn folder_tree_mirror_calls() {
        let stub = Stub::new();
        let backend: Arc<dyn HelperBackend> = stub.clone();
        let mut tree = FolderScan::tree_from_snapshot(backend, &tree_snapshot(), "/l", "/r");
        assert_eq!(tree.handle(), 9);
        assert_eq!(tree.all().len(), 2);
        assert_eq!(tree.roots, vec![0]);
        assert_eq!(tree.node(1).unwrap().status, FolderStatus::Different);
        assert_eq!(tree.node(1).unwrap().newer, FolderNewer::Right);
        assert!(tree.node(0).unwrap().is_dir());

        stub.push("folder.pending", json!({"ids": [1, 99]}));
        assert_eq!(tree.pending(), vec![1]);

        stub.push(
            "folder.counts",
            json!({"different": 2, "unimportant": 0, "leftOnly": 0, "rightOnly": 0,
                   "same": 0, "sameByMetadata": 0, "unknown": 0, "error": 0}),
        );
        let c = tree.counts();
        assert_eq!(c.different, 2);
        assert_eq!(c.same_by_metadata, 0);

        stub.push("folder.rows", json!({"rows": [{"id": 0, "depth": 0}, {"id": 1, "depth": 1}]}));
        let rows = tree.rows(&FolderView::default());
        assert_eq!(rows, vec![FolderRow { id: 0, depth: 0 }, FolderRow { id: 1, depth: 1 }]);
        let rp = stub.params("folder.rows");
        assert_eq!(rp["filter"], "all");
        assert_eq!(rp["flatten"], false);

        stub.push(
            "folder.settle",
            json!({"statuses": [[0, "different", false, "none"], [1, "same", true, "left"]]}),
        );
        tree.settle();
        assert_eq!(tree.node(1).unwrap().status, FolderStatus::Same);
        assert!(tree.node(1).unwrap().same_by_metadata);
        assert_eq!(tree.node(1).unwrap().newer, FolderNewer::Left);
    }

    #[test]
    fn folder_tree_path_and_expand() {
        let stub = Stub::new();
        stub.push("folder.path", json!({"path": "/l/a.txt"}));
        let backend: Arc<dyn HelperBackend> = stub.clone();
        let mut tree = FolderScan::tree_from_snapshot(backend, &tree_snapshot(), "/l", "/r");
        assert_eq!(tree.path(1, CompareSide::Left), "/l/a.txt");
        tree.expand_all(true);
        assert!(tree.node(0).unwrap().expanded);
        assert!(!tree.node(1).unwrap().expanded);
    }

    #[test]
    fn folder_page_state_actions() {
        let stub = Stub::new();
        let backend: Arc<dyn HelperBackend> = stub.clone();
        let mut page = FolderPageState::new(backend.clone(), FolderOptions::default());
        assert!(page.test_do("filter", "diffs").is_some()); // no session

        let tree = FolderScan::tree_from_snapshot(backend, &tree_snapshot(), "/l", "/r");
        let mut session = FolderSession::new("/l", "/r");
        session.tree = Some(tree);
        session.rows = vec![FolderRow { id: 0, depth: 0 }, FolderRow { id: 1, depth: 1 }];
        page.bind(session);

        // Every view change rebuilds rows through `folder.rows`.
        for _ in 0..8 {
            stub.push("folder.rows", json!({"rows": [{"id": 0, "depth": 0}, {"id": 1, "depth": 1}]}));
        }

        assert_eq!(page.test_do("filter", "diffs"), None);
        assert_eq!(page.session.as_ref().unwrap().view.filter, FolderFilter::Diffs);
        assert!(page.test_do("filter", "bogus").unwrap().starts_with("folder-filter:"));

        assert_eq!(page.test_do("flatten", ""), None);
        assert!(page.session.as_ref().unwrap().view.flatten);

        assert_eq!(page.test_do("names", "*.txt"), None);
        assert_eq!(page.session.as_ref().unwrap().view.name_filter, "*.txt");

        assert_eq!(page.test_do("select", "a.txt"), None);
        assert_eq!(page.session.as_ref().unwrap().cursor, 1);
        assert!(page.test_do("select", "nope").unwrap().contains("no row"));

        assert_eq!(page.test_do("focus", "right"), None);
        assert_eq!(page.session.as_ref().unwrap().focus, CompareSide::Right);

        // sync preview: sets lastPlan without running.
        stub.push(
            "folder.sync_plan",
            json!({"copies": [{"src": "/l/a", "dst": "/r/a", "to": "right", "rel": "a", "replaces": true}],
                   "trash": [], "skipped": []}),
        );
        assert_eq!(page.test_do("sync", "updateRight:preview"), None);
        let st = page.test_state();
        assert_eq!(st["syncPlan"]["mode"], "updateRight");
        assert_eq!(st["syncPlan"]["copies"], json!(["→ a"]));

        assert!(page.test_do("sync", "bogus").unwrap().starts_with("folder-sync:"));
        let _ = page;
    }

    #[test]
    fn window_model_test_hooks() {
        let stub = Stub::new();
        stub.push("compare.new", new_snapshot());
        let dir = temp_dir("window");
        let model = CompareWindowModel::with_backend(
            stub.clone(),
            CompareConfig::default(),
            CompareRecent::new(&dir),
        );
        let mut model = model;
        assert!(model.on_start_page());
        let idx = model.open_pair(None, None);
        assert_eq!(idx, 0);
        assert!(!model.on_start_page());
        assert_eq!(model.sessions[0].display_count(), 2);

        assert_eq!(model.test_do("cursor:1"), None);
        assert_eq!(model.sessions[0].cursor, 1);
        assert!(model.test_do("cursor:x").is_some());
        assert_eq!(model.test_do("filter:diffs"), None);
        assert_eq!(model.sessions[0].filter, CompareFilter::Diffs);
        assert!(model.test_do("filter:bogus").is_some());
        assert_eq!(model.test_do("swap"), None);
        assert_eq!(model.test_do("start"), None);
        assert!(model.on_start_page());
        assert_eq!(model.test_do("close-all"), None);
        assert!(model.sessions.is_empty());
        assert!(model.test_do("nope").is_some());

        let st = model.test_state();
        assert_eq!(st["startPage"], true);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_defaults_and_parsing() {
        let mut entries = HashMap::new();
        entries.insert("context-lines".to_string(), "5".to_string());
        entries.insert("recent".to_string(), "10".to_string());
        entries.insert("ignore-case".to_string(), "true".to_string());
        entries.insert("gutter-arrows".to_string(), "always".to_string());
        let c = CompareConfig::from_entries(entries);
        assert_eq!(c.context_lines(), 5);
        assert_eq!(c.recent_limit(), 10);
        assert!(c.importance().ignore_case);
        assert_eq!(c.gutter_arrows(), "always");
        assert_eq!(c.tab_width(), 4);
        assert!(!compare_enabled(&c));

        let mut on = HashMap::new();
        on.insert("enabled".to_string(), "true".to_string());
        assert!(compare_enabled(&CompareConfig::from_entries(on)));
    }

    fn helper_ready() -> bool {
        let lib = format!("{}/../../pylib", env!("CARGO_MANIFEST_DIR"));
        if !crate::app::python_helper::lib_has_helper(&lib) {
            return false;
        }
        let h = PythonHelper::shared();
        h.configure(&lib);
        h.call_default("ping", json!({})).is_ok()
    }

    #[test]
    fn text_compare_round_trip_when_helper_present() {
        if !helper_ready() {
            eprintln!("skipping: no python 3.11+ or pylib/helper not found");
            return;
        }
        let backend = real_backend();
        let left = TextSide::decode(backend.as_ref(), b"one\ntwo\nthree\n").unwrap();
        let right = TextSide::decode(backend.as_ref(), b"one\nTWO\nthree\n").unwrap();
        let mut tc = TextCompare::with_backend(
            backend,
            left,
            right,
            Importance::default(),
            false,
        );
        assert!(tc.handle() >= 0);
        assert_eq!(tc.rows().len(), 3);
        assert_eq!(tc.rows()[1].kind, RowKind::Changed);
        assert!(tc.rows()[1].important);
        assert_eq!(tc.sections().len(), 1);
        assert!(tc.can_undo() == false);

        tc.replace(CompareSide::Right, 1..2, &["TWO".to_string()]);
        assert!(tc.can_undo());
        assert!(tc.undo(None));
        assert!(!tc.can_undo());
    }

    #[test]
    fn start_pasted_then_edit_replaces_side() {
        let stub = Stub::new();
        stub.push("compare.new", new_snapshot());
        let dir = temp_dir("paste");
        let mut model = CompareWindowModel::with_backend(
            stub.clone(),
            CompareConfig::default(),
            CompareRecent::new(&dir),
        );
        assert_eq!(model.test_do("start-paste"), None);
        assert_eq!(model.sessions.len(), 1);
        assert_eq!(model.sessions[0].focus, CompareSide::Right);
        // Paste replaces the (empty) left side in place and re-diffs.
        stub.push("compare.replace", new_snapshot());
        assert_eq!(model.test_do("paste:left:hello\\nworld"), None);
        let p = stub.params("compare.replace");
        assert_eq!(p["side"], "left");
        assert_eq!(p["count"], 2);
        assert_eq!(p["lines"], json!(["hello", "world"]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn align_with_picks_then_anchors_both_lines() {
        // Swift `alignWith`: one side picks, the other side's pick aligns the
        // two 0-based lines (never line 0 of the other side).
        let stub = Stub::new();
        stub.push("compare.new", new_snapshot());
        let dir = temp_dir("align");
        let mut model = CompareWindowModel::with_backend(
            stub.clone(),
            CompareConfig::default(),
            CompareRecent::new(&dir),
        );
        assert_eq!(model.test_do("start-paste"), None);
        model.align_with(CompareSide::Left, 1);
        assert_eq!(model.align_pick, Some((CompareSide::Left, 1)));
        assert_eq!(model.test_state()["current"]["alignPick"], json!("left:2"));
        stub.push("compare.align", new_snapshot());
        assert_eq!(model.test_do("align:2,3"), None);
        let p = stub.params("compare.align");
        assert_eq!((p["l"].clone(), p["r"].clone()), (json!(1), json!(2)));
        assert_eq!(model.align_pick, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_pair_with_two_folders_binds_a_folder_session() {
        let dir = temp_dir("folders");
        let (a, b) = (format!("{dir}/A"), format!("{dir}/B"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let mut model = CompareWindowModel::with_backend(
            Stub::new(),
            CompareConfig::default(),
            CompareRecent::new(&dir),
        );
        model.open_pair(Some(&a), Some(&b));
        assert_eq!(model.sessions.len(), 1);
        let fs = model.folder_page.session.as_ref().expect("folder session bound");
        assert_eq!((fs.left_root.as_str(), fs.right_root.as_str()), (a.as_str(), b.as_str()));
        model.close_session(0);
        assert!(model.folder_page.session.is_none(), "closing it unbinds the page");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wait_message_waits_until_finish_waiters() {
        let stub = Stub::new();
        stub.push("compare.new", new_snapshot());
        let dir = temp_dir("wait");
        let mut model = CompareWindowModel::with_backend(
            stub.clone(),
            CompareConfig::default(),
            CompareRecent::new(&dir),
        );
        let mut titles = HashMap::new();
        titles.insert(CompareSide::Left, "a.txt (HEAD)".to_string());
        let id = model.open_message("/nope/a", Some("/nope/b"), &titles, true).unwrap();
        assert!(model.is_waiting(id));
        assert!(model.sessions[0].git);
        assert_eq!(model.sessions[0].title[&CompareSide::Left], "a.txt (HEAD)");
        model.finish_waiters();
        assert!(!model.is_waiting(id), "hiding releases the --wait caller");
        assert!(model.sessions.is_empty(), "a clean git session is dropped");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn edit_marks_dirty_and_save_marks_clean() {
        let stub = Stub::new();
        let dir = temp_dir("dirty");
        let path = format!("{dir}/left.txt");
        std::fs::write(&path, "a\nb\n").unwrap();
        stub.push("compare.decode", json!({"side": side("a", "b").json()}));
        stub.push("compare.new", new_snapshot());
        let mut model = CompareWindowModel::with_backend(
            stub.clone(),
            CompareConfig::default(),
            CompareRecent::new(&dir),
        );
        model.open_pair(Some(&path), None);
        assert!(!model.sessions[0].is_dirty(), "a freshly opened pair is clean");

        // An edit swaps the whole side through compare.replace, pushing undo.
        let mut snap = new_snapshot();
        snap["undo"] = json!([1, 0]);
        snap["left"] = side("edited", "b").json();
        stub.push("compare.replace", snap);
        assert_eq!(model.test_do("edit:left:edited\\n"), None);
        assert_eq!(model.sessions[0].focus, CompareSide::Left);
        assert!(model.sessions[0].dirty(CompareSide::Left), "edit marks the side dirty");
        assert!(model.test_state()["sessions"][0]["dirtyL"].as_bool().unwrap());

        // Saving marks it clean again.
        stub.push("compare.encode", json!({"data": b64_encode(b"edited\n")}));
        assert_eq!(model.test_do("save:left"), None);
        assert!(!model.sessions[0].is_dirty(), "save marks the side clean");
        assert_eq!(std::fs::read(&path).unwrap(), b"edited\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn close_session_confirmation_flow() {
        let stub = Stub::new();
        stub.push("compare.new", new_snapshot());
        let dir = temp_dir("close");
        let mut model = CompareWindowModel::with_backend(
            stub.clone(),
            CompareConfig::default(),
            CompareRecent::new(&dir),
        );
        model.open_pair(None, None);
        let mut snap = new_snapshot();
        snap["undo"] = json!([1, 0]);
        snap["left"] = side("edited", "b").json();
        stub.push("compare.replace", snap);
        assert_eq!(model.test_do("edit:left:edited\\n"), None);
        assert!(model.sessions[0].is_dirty());

        // A dirty close asks instead of closing.
        let n = model.sessions.len();
        assert_eq!(model.test_do("close-session"), None);
        assert_eq!(model.confirm, Some(0));
        assert_eq!(model.sessions.len(), n, "the dirty session stays until confirmed");
        assert_eq!(model.test_state()["sheet"], true);

        // Cancel clears the sheet and keeps the session.
        assert_eq!(model.test_do("sheet-cancel"), None);
        assert_eq!(model.confirm, None);
        assert_eq!(model.sessions.len(), n);
        assert_eq!(model.test_state()["sheet"], false);

        // Force closes.
        assert_eq!(model.test_do("close-session:force"), None);
        assert!(model.sessions.is_empty());
        assert_eq!(model.confirm, None);

        // A clean session closes without asking.
        stub.push("compare.new", new_snapshot());
        model.open_pair(None, None);
        assert!(!model.sessions[0].is_dirty());
        assert_eq!(model.test_do("close-session"), None);
        assert!(model.sessions.is_empty());
        assert_eq!(model.confirm, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn folder_open_opens_pair_and_expands_dirs() {
        let stub = Stub::new();
        let dir = temp_dir("folderopen");
        let lpath = format!("{dir}/l.txt");
        let rpath = format!("{dir}/r.txt");
        std::fs::write(&lpath, "a\n").unwrap();
        std::fs::write(&rpath, "b\n").unwrap();
        let mut model = CompareWindowModel::with_backend(
            stub.clone(),
            CompareConfig::default(),
            CompareRecent::new(&dir),
        );
        let backend: Arc<dyn HelperBackend> = stub.clone();
        let tree = FolderScan::tree_from_snapshot(backend, &tree_snapshot(), "/l", "/r");
        let mut session = FolderSession::new("/l", "/r");
        session.tree = Some(tree);
        session.rows = vec![FolderRow { id: 0, depth: 0 }, FolderRow { id: 1, depth: 1 }];
        session.cursor = 1;
        model.folder_page.bind(session);
        // `a.txt`'s two paths, then the decode/new for the opened pair.
        stub.push("folder.path", json!({"path": lpath}));
        stub.push("folder.path", json!({"path": rpath}));
        stub.push("compare.decode", json!({"side": side("a", "").json()}));
        stub.push("compare.decode", json!({"side": side("b", "").json()}));
        stub.push("compare.new", new_snapshot());

        assert_eq!(model.test_do("folder-open"), None);
        assert_eq!(model.sessions.len(), 1, "a folder row opens a text compare");
        assert!(model.sub, "a folder pair opens as a sub compare");
        assert_eq!(model.sessions[0].path.get(&CompareSide::Left), Some(&lpath));
        assert_eq!(model.sessions[0].path.get(&CompareSide::Right), Some(&rpath));

        // Activating a directory toggles its expansion instead.
        model.folder_page.session.as_mut().unwrap().cursor = 0;
        assert_eq!(model.test_do("folder-open"), None);
        assert!(model.folder_page.session.as_ref().unwrap().tree.as_ref().unwrap().node(0).unwrap().expanded);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn folder_drop_action() {
        let ready = helper_ready();
        let stub = Stub::new();
        let dir = temp_dir("folderdrop");
        let left = format!("{dir}/A");
        let right = format!("{dir}/B");
        std::fs::create_dir_all(&left).unwrap();
        std::fs::create_dir_all(&right).unwrap();
        let ext = format!("{dir}/dropped.txt");
        std::fs::write(&ext, "d\n").unwrap();
        let mut model = CompareWindowModel::with_backend(
            stub.clone(),
            CompareConfig::default(),
            CompareRecent::new(&dir),
        );
        let backend: Arc<dyn HelperBackend> = stub.clone();
        let tree = FolderScan::tree_from_snapshot(backend, &tree_snapshot(), &left, &right);
        let mut session = FolderSession::new(&left, &right);
        session.tree = Some(tree);
        session.rows = vec![FolderRow { id: 0, depth: 0 }, FolderRow { id: 1, depth: 1 }];
        session.cursor = 1;
        model.folder_page.bind(session);

        // A bad side is rejected.
        assert!(model.test_do("folder-drop:mid:x").is_some());
        // An external path drops into the named root.
        assert_eq!(model.test_do(&format!("folder-drop:left:{ext}")), None);
        if ready {
            assert!(
                Path::new(&left).join("dropped.txt").exists(),
                "the dropped file lands in the left root"
            );
        }
        // A bare side drags the selection across to the other side.
        assert_eq!(model.test_do("folder-drop:right"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn folder_summary_matches_swift() {
        let c = FolderTreeCounts { different: 1, left_only: 1, same: 1, ..Default::default() };
        let s = folder_summary(&c);
        assert!(s.contains("1 differ"));
        assert!(s.contains("1 left only"));
        assert!(s.contains("1 same"));

        let identical = FolderTreeCounts { same: 3, ..Default::default() };
        assert_eq!(folder_summary(&identical), "Identical — 3 files");
        assert_eq!(folder_summary(&FolderTreeCounts::default()), "empty");
    }

    // -- drawing-layer pure logic ------------------------------------------

    fn drow(l: i32, r: i32, kind: RowKind, important: bool) -> CompareRow {
        CompareRow { l, r, kind, important }
    }

    #[test]
    fn pane_cell_classification() {
        let same = drow(0, 0, RowKind::Same, false);
        assert_eq!(pane_cell(&same, CompareSide::Left, false), PaneCell::Same);

        let changed = drow(1, 1, RowKind::Changed, true);
        assert_eq!(pane_cell(&changed, CompareSide::Left, false), PaneCell::DiffImportant);
        assert_eq!(pane_cell(&changed, CompareSide::Right, false), PaneCell::DiffImportant);

        let soft = drow(1, 1, RowKind::Changed, false);
        assert_eq!(pane_cell(&soft, CompareSide::Left, false), PaneCell::DiffUnimportant);
        assert_eq!(pane_cell(&soft, CompareSide::Left, true), PaneCell::Ignored);

        // missing sides become empty regardless of kind
        let left_only = drow(2, -1, RowKind::LeftOnly, true);
        assert_eq!(pane_cell(&left_only, CompareSide::Right, false), PaneCell::Empty);
        assert_eq!(pane_cell(&left_only, CompareSide::Left, false), PaneCell::DiffImportant);
    }

    #[test]
    fn pane_layout_geometry() {
        let layout = PaneLayout::new(800.0, 600.0);
        assert_eq!(layout.pane_w(), (800.0 - 34.0) / 2.0);
        assert_eq!(layout.pane_x(CompareSide::Left), 0.0);
        assert_eq!(layout.pane_x(CompareSide::Right), layout.pane_w() + 34.0);

        let mid = layout.pane_w() + 17.0;
        assert_eq!(layout.side_at(mid - 1.0), CompareSide::Left);
        assert_eq!(layout.side_at(mid + 1.0), CompareSide::Right);

        // row_at accounts for the header.
        assert_eq!(layout.row_at(0.0), 0);
        assert_eq!(layout.row_at(layout.header - 1.0), 0);
        assert_eq!(layout.row_at(layout.header + 18.0), 1);
        assert_eq!(layout.row_y(2), layout.header + 36.0);

        let r = layout.row_rect(CompareSide::Left, 1);
        assert_eq!(r.x, 0.0);
        assert_eq!(r.y, layout.header + 18.0);
        assert_eq!(r.height, 18.0);

        assert_eq!(layout.line_no_width(5), digit_count(99) as f64 * 7.8 + 14.0);
        assert_eq!(layout.line_no_width(1234), 4.0 * 7.8 + 14.0);
        assert!(layout.text_x(CompareSide::Right, 5) > layout.pane_x(CompareSide::Right));
        assert_eq!(layout.doc_height(3), layout.header + 54.0);
    }

    #[test]
    fn thumbnail_levels_maps_rows() {
        let rows = vec![
            drow(0, 0, RowKind::Same, false),
            drow(1, 1, RowKind::Changed, true),
            drow(2, 2, RowKind::Changed, false),
            drow(-1, 3, RowKind::RightOnly, true),
        ];
        let levels = thumbnail_levels(&rows, 4, false);
        assert_eq!(levels.len(), 4);
        // Every diff marks its slot; important outranks plain.
        assert_eq!(levels[0], THUMB_LEVEL_NONE);
        assert_eq!(levels[1], THUMB_LEVEL_IMPORTANT);
        assert_eq!(levels[2], THUMB_LEVEL_DIFF);
        assert_eq!(levels[3], THUMB_LEVEL_IMPORTANT);

        // With unimportant diffs ignored, the plain row vanishes.
        let ignored = thumbnail_levels(&rows, 4, true);
        assert_eq!(ignored[2], THUMB_LEVEL_NONE);

        assert!(thumbnail_levels(&[], 4, false).iter().all(|v| *v == THUMB_LEVEL_NONE));
        assert!(thumbnail_levels(&rows, 0, false).is_empty());
    }

    #[test]
    fn folder_size_label_matches_swift() {
        assert_eq!(folder_size_label(0), "0 B");
        assert_eq!(folder_size_label(1023), "1023 B");
        assert_eq!(folder_size_label(1024), "1.0 KB");
        assert_eq!(folder_size_label(1536), "1.5 KB");
        assert_eq!(folder_size_label(10 * 1024), "10 KB");
        assert_eq!(folder_size_label(1024 * 1024), "1.0 MB");
        assert_eq!(folder_size_label(-5), "0 B");
    }

    #[test]
    fn folder_glyph_matches_swift() {
        assert_eq!(folder_glyph(FolderStatus::Same, false, false, FolderNewer::None), "=");
        assert_eq!(
            folder_glyph(FolderStatus::Different, true, false, FolderNewer::None),
            "≠"
        );
        assert_eq!(
            folder_glyph(FolderStatus::Different, false, false, FolderNewer::Left),
            ">"
        );
        assert_eq!(
            folder_glyph(FolderStatus::Different, false, false, FolderNewer::Right),
            "<"
        );
        assert_eq!(
            folder_glyph(FolderStatus::Different, false, false, FolderNewer::None),
            "≠"
        );
        assert_eq!(
            folder_glyph(FolderStatus::Unimportant, false, false, FolderNewer::None),
            "≈"
        );
        assert_eq!(folder_glyph(FolderStatus::LeftOnly, false, false, FolderNewer::None), "");
        assert_eq!(folder_glyph(FolderStatus::Unknown, false, false, FolderNewer::None), "…");
        assert_eq!(folder_glyph(FolderStatus::Error, false, false, FolderNewer::None), "!");
    }

    #[test]
    fn folder_row_color_mapping() {
        use FolderColor::*;
        assert_eq!(
            folder_row_color(FolderStatus::Same, FolderNewer::None, false, CompareSide::Left),
            Text
        );
        assert_eq!(
            folder_row_color(FolderStatus::Different, FolderNewer::None, true, CompareSide::Left),
            Danger
        );
        // The older side of a "newer" file is dimmed.
        assert_eq!(
            folder_row_color(FolderStatus::Different, FolderNewer::Right, false, CompareSide::Left),
            Dim
        );
        assert_eq!(
            folder_row_color(FolderStatus::Different, FolderNewer::Right, false, CompareSide::Right),
            Danger
        );
        assert_eq!(
            folder_row_color(FolderStatus::Unimportant, FolderNewer::None, false, CompareSide::Left),
            Info
        );
        assert_eq!(
            folder_row_color(FolderStatus::LeftOnly, FolderNewer::None, false, CompareSide::Left),
            Accent2
        );
        assert_eq!(
            folder_row_color(FolderStatus::Error, FolderNewer::None, false, CompareSide::Left),
            Warning
        );
    }

    #[test]
    fn folder_tree_model_builds_from_nodes() {
        let stub = Stub::new();
        let backend: Arc<dyn HelperBackend> = stub.clone();
        let tree = FolderScan::tree_from_snapshot(backend, &tree_snapshot(), "/l", "/r");
        let counts = FolderTreeCounts { different: 2, ..Default::default() };
        let rows = vec![FolderRow { id: 0, depth: 0 }, FolderRow { id: 1, depth: 1 }];
        let model = FolderTreeModel::build(&tree, &rows, counts, &FolderView::default());
        assert_eq!(model.rows.len(), 2);
        assert!(model.rows[0].is_dir);
        assert!(model.rows[0].glyph() == "≠");
        assert_eq!(model.rows[1].name, "a.txt");
        assert_eq!(model.rows[1].newer, FolderNewer::Right);
        assert!(model.summary().contains("2 different"));
        assert!(!model.is_empty());

        // A missing id is skipped, not panicked on.
        let bad = vec![FolderRow { id: 99, depth: 0 }];
        let empty = FolderTreeModel::build(&tree, &bad, FolderTreeCounts::default(), &FolderView::default());
        assert!(empty.is_empty());
        assert_eq!(empty.summary(), "no items");
    }

    #[test]
    fn byte_count_file_matches_bytecountformatter() {
        assert_eq!(byte_count_file(0), "Zero KB");
        assert_eq!(byte_count_file(1), "1 byte");
        assert_eq!(byte_count_file(999), "999 bytes");
        assert_eq!(byte_count_file(1_000), "1 KB");
        assert_eq!(byte_count_file(12_345), "12 KB");
        assert_eq!(byte_count_file(1_500_000), "1.5 MB");
        assert_eq!(byte_count_file(2_000_000), "2 MB");
        assert_eq!(byte_count_file(1_250_000_000), "1.25 GB");
    }

    #[test]
    fn scroll_top_keeps_the_cursor_visible() {
        assert_eq!(PaneModel::scroll_top(0, 3, 10, 100), 0);
        assert_eq!(PaneModel::scroll_top(0, 10, 10, 100), 1);
        assert_eq!(PaneModel::scroll_top(20, 5, 10, 100), 5);
        assert_eq!(PaneModel::scroll_top(5, 9, 10, 100), 5);
        // Clamped to the content; a short list never scrolls.
        assert_eq!(PaneModel::scroll_top(95, 99, 10, 100), 90);
        assert_eq!(PaneModel::scroll_top(4, 2, 10, 5), 0);
    }

    #[test]
    fn session_summary_mirrors_syncall() {
        let cfg = CompareConfig::default();
        let session = |snap: Value, l: TextSide, r: TextSide| {
            let stub = Stub::new();
            stub.push("compare.new", snap);
            let backend: Arc<dyn HelperBackend> = stub.clone();
            let tc = TextCompare::with_backend(backend, l, r, Importance::default(), false);
            CompareSession::new(tc)
        };
        // Sections: "≠ N section(s)".
        let s = session(new_snapshot(), side("a", "b"), side("a", "c"));
        assert!(s.summary(&cfg).starts_with("≠ 1 section"), "{}", s.summary(&cfg));
        // No sections, same text: "Identical".
        let mut same = new_snapshot();
        same["right"] = side("a", "b").json();
        same["rows"] = json!([[0, 0, 0], [1, 1, 0]]);
        same["sections"] = json!([]);
        let s = session(same.clone(), side("a", "b"), side("a", "b"));
        assert_eq!(s.summary(&cfg), "Identical");
        // Both empty: no label.
        let mut empty = same.clone();
        empty["left"] = TextSide::default().json();
        empty["right"] = TextSide::default().json();
        empty["rows"] = json!([]);
        let s = session(empty, TextSide::default(), TextSide::default());
        assert_eq!(s.summary(&cfg), "");
        // Binary: identical / first difference.
        let mut s = session(same, side("a", "b"), side("a", "b"));
        s.binary.insert(CompareSide::Left, vec![1, 2, 3]);
        s.binary.insert(CompareSide::Right, vec![1, 2, 3]);
        assert_eq!(s.summary(&cfg), "Binary files are identical");
        s.binary.insert(CompareSide::Right, vec![1, 9, 3, 4]);
        assert_eq!(
            s.summary(&cfg),
            "Binary files differ (3 bytes vs 4 bytes, first difference at byte 1)"
        );
    }

    #[test]
    fn pane_model_from_session_snapshots() {
        use crate::engines::text_edit_keys::{Focus, Modifiers};
        let stub = Stub::new();
        stub.push("compare.new", new_snapshot());
        let backend: Arc<dyn HelperBackend> = stub.clone();
        let tc = TextCompare::with_backend(
            backend,
            TextSide::default(),
            TextSide::default(),
            Importance::default(),
            false,
        );
        let mut s = CompareSession::new(tc);
        s.focus = CompareSide::Right;
        s.refresh(3);
        let m = PaneModel::from_session(&s);
        assert_eq!(m.rows.len(), 2);
        assert_eq!(m.display_count(), 2);
        assert_eq!(m.model_row(1), 1);
        assert_eq!(m.focus, CompareSide::Right);
        assert!(m.is_anchor_row(0));
        assert!(!m.is_anchor_row(1));
        assert_eq!(m.mode, "All");

        // An empty model still renders.
        let empty = PaneModel::empty();
        assert_eq!(empty.display_count(), 0);
        assert_eq!(empty.model_row(5), 0);

        // Editor routing is the shared engine's decision.
        let editable = Focus { is_text: true, is_editable: true };
        let bare = Modifiers { ctrl: true, cmd: false, option: false, shift: false };
        assert_eq!(
            editor_edit_action(crate::engines::text_edit_keys::KEY_W, bare, editable),
            Some(crate::engines::text_edit_keys::EditAction::KillWordBackward)
        );
        // The engine fires clipboard actions on Cmd OR Ctrl.
        assert_eq!(
            editor_edit_action(crate::engines::text_edit_keys::KEY_V, bare, editable),
            Some(crate::engines::text_edit_keys::EditAction::Paste)
        );
    }
}
