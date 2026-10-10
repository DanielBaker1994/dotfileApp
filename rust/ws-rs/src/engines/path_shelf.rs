//! Port of `PathShelf.swift` — the `/paths` recent-file shelf.
//!
//! The store rules (canonical, normalize, bump, dedup, finalize, load,
//! save, rekey) live in `pylib/shelf.py`; the ignore rules live in
//! `pylib/ignore_rules.py`. Both are reached through the Python helper,
//! exactly as the Swift facade does. The Swift `DispatchQueue` / debounce
//! plumbing is synchronous here (the model is all a first cut needs).

use serde_json::{json, Value};
use std::cell::RefCell;
use std::fs;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::app::python_helper::PythonHelper;
use crate::engines::recent_files::RecentFiles;

pub const MAX_LIMIT: usize = 25;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    Created,
    Modified,
    Downloaded,
    Clipboard,
    Filefast,
    Copied,
    Screenshot,
}

impl Why {
    pub fn raw(self) -> &'static str {
        match self {
            Why::Created => "created",
            Why::Modified => "modified",
            Why::Downloaded => "downloaded",
            Why::Clipboard => "clipboard",
            Why::Filefast => "filefast",
            Why::Copied => "copied",
            Why::Screenshot => "screenshot",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Why::Created => "new",
            Why::Modified => "edited",
            Why::Downloaded => "downloaded",
            Why::Clipboard => "copied",
            Why::Filefast => "filefast",
            Why::Copied => "files view",
            Why::Screenshot => "screenshot",
        }
    }

    pub fn from_raw(s: &str) -> Option<Why> {
        match s {
            "created" => Some(Why::Created),
            "modified" => Some(Why::Modified),
            "downloaded" => Some(Why::Downloaded),
            "clipboard" => Some(Why::Clipboard),
            "filefast" => Some(Why::Filefast),
            "copied" => Some(Why::Copied),
            "screenshot" => Some(Why::Screenshot),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub path: String,
    pub at: f64,
    pub why: Why,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Canonical {
    pub path: String,
    pub is_file: bool,
    pub is_dir: bool,
}

fn home_dir() -> String {
    std::env::var("HOME").unwrap_or_default()
}

/// Strictly increasing epoch seconds — the Swift store relies on `Date()`
/// ordering; a tight test loop would otherwise tie.
fn next_stamp() -> f64 {
    static C: AtomicU64 = AtomicU64::new(0);
    let real = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut prev = C.load(Ordering::SeqCst);
    loop {
        let next = prev.max(real).saturating_add(1);
        match C.compare_exchange(prev, next, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => return next as f64 / 1e9,
            Err(p) => prev = p,
        }
    }
}

fn shelf_call(method: &str, params: Value) -> Option<Value> {
    PythonHelper::shared().call_default(method, params).ok()
}

fn opt_str(v: Option<&str>) -> Value {
    match v {
        Some(s) => json!(s),
        None => Value::Null,
    }
}

fn item_value(i: &Item) -> Value {
    json!({"path": i.path, "at": i.at, "why": i.why.raw()})
}

fn items_value(items: &[Item]) -> Value {
    Value::Array(items.iter().map(item_value).collect())
}

fn items_from_value(v: &Value) -> Vec<Item> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|d| {
                    let path = d.get("path")?.as_str()?.to_string();
                    let at = d.get("at").and_then(Value::as_f64).unwrap_or(0.0);
                    let why = Why::from_raw(d.get("why").and_then(Value::as_str).unwrap_or("modified"))?;
                    Some(Item { path, at, why })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `shelf.canonical` — full realpath + stat.
pub fn canonical(p: &str) -> Option<Canonical> {
    let b = shelf_call("shelf.canonical", json!({"path": p}))?;
    let r = b.get("result")?;
    if r.is_null() {
        return None;
    }
    Some(Canonical {
        path: r.get("path")?.as_str()?.to_string(),
        is_file: r.get("isFile").and_then(Value::as_bool).unwrap_or(false),
        is_dir: r.get("isDir").and_then(Value::as_bool).unwrap_or(false),
    })
}

/// `shelf.normalize`.
pub fn normalize(p: &str) -> String {
    shelf_call("shelf.normalize", json!({"path": p}))
        .and_then(|b| b.get("path").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| p.to_string())
}

/// `shelf.rekey` — the path after a rename, else None.
pub fn rekey(p: &str, old: &str, new: &str) -> Option<String> {
    let b = shelf_call("shelf.rekey", json!({"path": p, "old": old, "new": new}))?;
    match b.get("path") {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}

pub fn bump(items: &[Item], path: &str, why: Why, at: f64, limit: usize) -> Vec<Item> {
    let params = json!({
        "items": items_value(items), "path": path, "why": why.raw(), "at": at, "limit": limit
    });
    shelf_call("shelf.bump", params)
        .and_then(|b| b.get("items").map(items_from_value))
        .unwrap_or_default()
}

pub fn dedup(items: &[Item]) -> Vec<Item> {
    shelf_call("shelf.dedup", json!({"items": items_value(items)}))
        .and_then(|b| b.get("items").map(items_from_value))
        .unwrap_or_default()
}

pub fn load_candidates(raw: &[Value]) -> Vec<Item> {
    shelf_call("shelf.load", json!({"raw": raw}))
        .and_then(|b| b.get("items").map(items_from_value))
        .unwrap_or_default()
}

pub fn finalize(items: &[Item], limit: usize) -> Vec<Item> {
    shelf_call(
        "shelf.finalize",
        json!({"items": items_value(items), "limit": limit}),
    )
    .and_then(|b| b.get("items").map(items_from_value))
    .unwrap_or_default()
}

pub fn save(path: &str, items: &[Item]) {
    let _ = shelf_call("shelf.save", json!({"path": path, "items": items_value(items)}));
}

fn ir_call(method: &str, params: Value) -> Option<Value> {
    PythonHelper::shared().call_default(method, params).ok()
}

/// `IgnoreRules` — facade over `pylib/ignore_rules.py`'s `Rules` handle.
pub struct IgnoreRules {
    handle: i64,
}

impl IgnoreRules {
    pub fn new(home: &str, shelf_file: Option<&str>) -> Self {
        let handle = ir_call(
            "ignore.new",
            json!({"home": home, "shelfFile": opt_str(shelf_file)}),
        )
        .and_then(|b| b.get("handle").and_then(Value::as_i64))
        .unwrap_or(-1);
        IgnoreRules { handle }
    }

    pub fn handle(&self) -> i64 {
        self.handle
    }

    pub fn set_shelf(&mut self, file: Option<&str>) {
        if self.handle >= 0 {
            let _ = ir_call(
                "ignore.set_shelf",
                json!({"handle": self.handle, "file": opt_str(file)}),
            );
        }
    }

    pub fn set_git_excludes(&mut self, path: Option<&str>) {
        if self.handle >= 0 {
            let _ = ir_call(
                "ignore.set_git_excludes",
                json!({"handle": self.handle, "path": opt_str(path)}),
            );
        }
    }

    pub fn set_recheck(&mut self, seconds: f64) {
        if self.handle >= 0 {
            let _ = ir_call(
                "ignore.set_recheck",
                json!({"handle": self.handle, "seconds": seconds}),
            );
        }
    }

    pub fn ignored(&self, path: &str, is_dir: bool) -> bool {
        if self.handle < 0 {
            return false;
        }
        ir_call(
            "ignore.ignored",
            json!({"handle": self.handle, "path": path, "isDir": is_dir}),
        )
        .and_then(|b| b.get("ignored").and_then(Value::as_bool))
        .unwrap_or(false)
    }
}

impl Drop for IgnoreRules {
    fn drop(&mut self) {
        if self.handle >= 0 {
            let _ = ir_call("ignore.drop", json!({"handle": self.handle}));
        }
    }
}

/// `PathShelf` — the shelf model (queue/timers synchronous here).
pub struct PathShelf {
    pub limit: usize,
    pub immediate: bool,
    loaded: bool,
    store: String,
    items: Vec<Item>,
    snapshot: Vec<Item>,
    pub rules: IgnoreRules,
    pub on_changed: Option<Box<dyn FnMut()>>,
}

impl PathShelf {
    pub fn new() -> Self {
        let store = format!("{}/.cache/kitchen-sink/paths.json", home_dir());
        Self::with_rules(store, IgnoreRules::new(&home_dir(), None))
    }

    pub fn with_store(store: &str) -> Self {
        Self::with_rules(store.to_string(), IgnoreRules::new(&home_dir(), None))
    }

    pub fn with_rules(store: String, rules: IgnoreRules) -> Self {
        PathShelf {
            limit: MAX_LIMIT,
            immediate: false,
            loaded: false,
            store,
            items: Vec::new(),
            snapshot: Vec::new(),
            rules,
            on_changed: None,
        }
    }

    pub fn configure(&mut self, limit: i64, ignore_file: Option<&str>) {
        self.limit = (limit.max(1) as usize).min(MAX_LIMIT);
        self.rules.set_shelf(ignore_file);
        if !self.loaded {
            self.load();
            self.loaded = true;
        }
        self.items.truncate(self.limit);
        self.publish();
    }

    pub fn entries(&self) -> Vec<Item> {
        self.snapshot
            .iter()
            .filter(|i| Path::new(&i.path).exists())
            .cloned()
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn observe(&mut self, path: &str, created: bool, origin: Option<&str>) {
        if !self.loaded {
            return;
        }
        let Some(c) = canonical(path) else { return };
        if !c.is_file || self.rules.ignored(&c.path, false) {
            return;
        }
        let why = if origin.is_some() {
            Why::Downloaded
        } else if created {
            Why::Created
        } else {
            Why::Modified
        };
        self.bump_one(&c.path, why);
    }

    pub fn add(&mut self, paths: &[String], why: Why) {
        if !self.loaded {
            return;
        }
        for p in paths.iter().rev() {
            let n = normalize(p);
            let Some(c) = canonical(&n) else { continue };
            if !(c.is_file || c.is_dir) {
                continue;
            }
            self.bump_one(&c.path, why);
        }
    }

    pub fn renamed(&mut self, old: &str, to: &str) {
        let mut hit = false;
        for i in 0..self.items.len() {
            if let Some(p) = rekey(&self.items[i].path, old, to) {
                self.items[i].path = p;
                hit = true;
            }
        }
        if hit {
            self.dedup_now();
            self.publish();
        }
    }

    pub fn remove(&mut self, paths: &[String]) {
        let gone: std::collections::HashSet<&str> = paths.iter().map(String::as_str).collect();
        let before = self.items.len();
        self.items.retain(|i| !gone.contains(i.path.as_str()));
        if self.items.len() != before {
            self.publish();
        }
    }

    pub fn seed(&mut self, recent: &[(String, f64, Option<String>)]) {
        if !self.loaded || !self.items.is_empty() {
            return;
        }
        for (path, at, source) in recent {
            if self.items.len() >= self.limit {
                break;
            }
            let Some(c) = canonical(path) else { continue };
            if !c.is_file || self.rules.ignored(&c.path, false) {
                continue;
            }
            if self.items.iter().any(|i| i.path == c.path) {
                continue;
            }
            let why = if source.is_some() {
                Why::Downloaded
            } else {
                Why::Modified
            };
            self.items.push(Item {
                path: c.path,
                at: *at,
                why,
            });
        }
        if !self.items.is_empty() {
            self.publish();
        }
    }

    pub fn sync(&self) {}

    /// Wire the shelf to the "one FSEvents stream" feed: `onKept` →
    /// `observe`, `onRenamed` → `renamed`, exactly as the app controller does.
    pub fn wire_recent(shelf: Rc<RefCell<PathShelf>>, recent: &mut RecentFiles) {
        let kept = shelf.clone();
        recent.on_kept = Some(Box::new(move |path, created, origin| {
            kept.borrow_mut().observe(path, created, origin);
        }));
        recent.on_renamed = Some(Box::new(move |from, to| {
            shelf.borrow_mut().renamed(from, to);
        }));
    }

    fn bump_one(&mut self, path: &str, why: Why) {
        let out = bump(&self.items, path, why, next_stamp(), self.limit);
        self.items = out;
        self.publish();
    }

    fn dedup_now(&mut self) {
        self.items = dedup(&self.items);
    }

    fn publish(&mut self) {
        self.snapshot = self.items.clone();
        if let Some(cb) = &mut self.on_changed {
            cb();
        }
        save(&self.store, &self.items);
    }

    fn load(&mut self) {
        let Ok(data) = fs::read(&self.store) else { return };
        let Ok(raw) = serde_json::from_slice::<Value>(&data) else {
            return;
        };
        let raw_arr = raw.as_array().cloned().unwrap_or_default();
        let mut candidates = load_candidates(&raw_arr);
        candidates.retain(|i| match i.why {
            Why::Created | Why::Modified | Why::Downloaded => !self.rules.ignored(&i.path, false),
            _ => true,
        });
        self.items = finalize(&candidates, self.limit);
    }
}

impl Default for PathShelf {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------- clipboard

pub const SKIP_TYPES: &[&str] = &[
    "org.nspasteboard.ConcealedType",
    "org.nspasteboard.TransientType",
    "org.nspasteboard.AutoGeneratedType",
    "com.agilebits.onepassword",
];
pub const FILE_URL_TYPE: &str = "public.file-url";
pub const STRING_TYPE: &str = "public.utf8-plain-text";

/// The slice of `NSPasteboard` `ClipboardPaths` needs (AppKit-free model).
pub trait Pasteboard {
    fn change_count(&self) -> i64;
    fn types(&self) -> Vec<String>;
    fn file_urls(&self) -> Vec<String>;
    fn string(&self) -> Option<String>;
}

pub struct ClipboardPaths {
    pb: Box<dyn Pasteboard>,
    pub on_paths: Option<Box<dyn FnMut(Vec<String>)>>,
    last_count: i64,
    own_count: Option<i64>,
}

impl ClipboardPaths {
    pub fn new(pb: Box<dyn Pasteboard>) -> Self {
        let last_count = pb.change_count();
        ClipboardPaths {
            pb,
            on_paths: None,
            last_count,
            own_count: None,
        }
    }

    pub fn own_write(&mut self) {
        self.own_count = Some(self.pb.change_count());
    }

    pub fn check(&mut self) {
        let n = self.pb.change_count();
        if n == self.last_count {
            return;
        }
        self.last_count = n;
        if Some(n) == self.own_count {
            return;
        }
        let paths = Self::paths_in(self.pb.as_ref());
        if !paths.is_empty() {
            if let Some(cb) = &mut self.on_paths {
                cb(paths);
            }
        }
    }

    pub fn paths_in(pb: &dyn Pasteboard) -> Vec<String> {
        let types = pb.types();
        if types.iter().any(|t| SKIP_TYPES.contains(&t.as_str())) {
            return Vec::new();
        }
        if types.iter().any(|t| t == FILE_URL_TYPE) {
            let urls = pb.file_urls();
            if !urls.is_empty() {
                return urls
                    .iter()
                    .take(25)
                    .filter_map(|u| canonical(u).map(|c| c.path))
                    .collect();
            }
        }
        match pb.string() {
            Some(t) => Self::paths_in_text(&t),
            None => Vec::new(),
        }
    }

    pub fn paths_in_text(text: &str) -> Vec<String> {
        if text.chars().count() > 4096 {
            return Vec::new();
        }
        let lines: Vec<String> = text
            .split(|c: char| c == '\n' || c == '\r')
            .map(|s| s.trim_matches(|c: char| c == ' ' || c == '\t').to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if lines.is_empty() || lines.len() > 5 {
            return Vec::new();
        }
        let mut out = Vec::new();
        for mut l in lines {
            let chars: Vec<char> = l.chars().collect();
            if chars.len() > 1
                && chars.first() == chars.last()
                && (chars[0] == '\'' || chars[0] == '"')
            {
                l = chars[1..chars.len() - 1].iter().collect();
            }
            if let Some(rest) = l.strip_prefix("file://") {
                match file_url_path(rest) {
                    Some(p) => l = p,
                    None => return Vec::new(),
                }
            } else {
                l = l.replace("\\ ", " ");
            }
            l = expand_tilde(&l);
            if !l.starts_with('/') {
                return Vec::new();
            }
            match canonical(&l) {
                Some(c) => out.push(c.path),
                None => return Vec::new(),
            }
        }
        out
    }
}

fn expand_tilde(p: &str) -> String {
    let home = home_dir();
    if p == "~" {
        return home;
    }
    if let Some(rest) = p.strip_prefix("~/") {
        return format!("{home}/{rest}");
    }
    p.to_string()
}

fn file_url_path(rest: &str) -> Option<String> {
    let path = if rest.starts_with('/') {
        rest.to_string()
    } else {
        // `file://host/path` — drop the authority
        let slash = rest.find('/')?;
        rest[slash..].to_string()
    };
    Some(percent_decode(&path))
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = |b: u8| -> Option<u8> {
                match b {
                    b'0'..=b'9' => Some(b - b'0'),
                    b'a'..=b'f' => Some(b - b'a' + 10),
                    b'A'..=b'F' => Some(b - b'A' + 10),
                    _ => None,
                }
            };
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engines::recent_files::{
        FSEVENT_ITEM_CREATED, FSEVENT_ITEM_IS_FILE, FSEVENT_ITEM_RENAMED,
    };
    use std::cell::RefCell;
    use std::rc::Rc;

    fn helper_ready() -> bool {
        let lib = format!("{}/../../pylib", env!("CARGO_MANIFEST_DIR"));
        if !crate::app::python_helper::lib_has_helper(&lib) {
            return false;
        }
        let h = PythonHelper::shared();
        h.configure(&lib);
        h.call_default("ping", json!({})).is_ok()
    }

    fn tmp(name: &str) -> String {
        let base = std::env::temp_dir().join(format!("ws-rs-shelf-{}-{}", std::process::id(), name));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        fs::canonicalize(&base).unwrap().to_string_lossy().into_owned()
    }

    fn write_file(p: &str, text: &str) {
        if let Some(d) = Path::new(p).parent() {
            let _ = fs::create_dir_all(d);
        }
        fs::write(p, text).unwrap();
    }

    #[test]
    fn why_labels() {
        assert_eq!(Why::Created.label(), "new");
        assert_eq!(Why::Modified.label(), "edited");
        assert_eq!(Why::Downloaded.label(), "downloaded");
        assert_eq!(Why::Clipboard.label(), "copied");
        assert_eq!(Why::Filefast.label(), "filefast");
        assert_eq!(Why::Copied.label(), "files view");
        assert_eq!(Why::Screenshot.label(), "screenshot");
        assert_eq!(Why::from_raw("filefast"), Some(Why::Filefast));
        assert_eq!(Why::from_raw("bogus"), None);
    }

    #[test]
    fn limit_hard_cap() {
        // `min(max(1, limit), MAX_LIMIT)` as configure computes it.
        let clamp = |l: i64| (l.max(1) as usize).min(MAX_LIMIT);
        assert_eq!(clamp(99), 25);
        assert_eq!(clamp(0), 1);
        assert_eq!(clamp(-4), 1);
        assert_eq!(clamp(10), 10);
        assert_eq!(MAX_LIMIT, 25);
    }

    #[test]
    fn canonical_normalize_rekey() {
        if !helper_ready() {
            eprintln!("skipping: no python 3.11+ or pylib/helper not found");
            return;
        }
        let root = tmp("canonical");
        assert!(canonical(&format!("{root}/nope.txt")).is_none());

        let f = format!("{root}/a.txt");
        write_file(&f, "x");
        let cf = canonical(&f).unwrap();
        assert_eq!(cf.path, f);
        assert!(cf.is_file && !cf.is_dir);
        let cd = canonical(&root).unwrap();
        assert!(!cd.is_file && cd.is_dir);

        let real = format!("{root}/real");
        fs::create_dir_all(&real).unwrap();
        std::os::unix::fs::symlink(&real, format!("{root}/link")).unwrap();
        assert!(canonical(&format!("{root}/link/x.txt")).is_none());
        write_file(&format!("{real}/x.txt"), "x");
        assert_eq!(
            canonical(&format!("{root}/link/x.txt")).unwrap().path,
            format!("{real}/x.txt")
        );

        let sock = format!("{root}/s.sock");
        let _l = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        let cs = canonical(&sock).unwrap();
        assert!(!cs.is_file && !cs.is_dir);

        assert_eq!(normalize("/tmp/a/../b"), "/private/tmp/b");
        assert_eq!(normalize("/tmp"), "/private/tmp");
        assert_eq!(normalize("file:///Users/x/a%20b.txt"), "/Users/x/a b.txt");
        assert_eq!(normalize("/a/b/../c"), "/a/c");
        assert_eq!(normalize("~/x"), format!("{}/x", home_dir()));

        assert_eq!(rekey("/a.txt", "/a.txt", "/b.txt").as_deref(), Some("/b.txt"));
        assert_eq!(rekey("/dir/x.txt", "/dir", "/new").as_deref(), Some("/new/x.txt"));
        assert!(rekey("/other/x.txt", "/dir", "/new").is_none());
        assert!(rekey("/dirsuffix", "/dir", "/new").is_none());
    }

    #[test]
    fn store_bump_finalize_save() {
        if !helper_ready() {
            eprintln!("skipping: no python 3.11+ or pylib/helper not found");
            return;
        }
        let items: Vec<Item> = (0..30)
            .map(|i| Item {
                path: format!("/f{i}"),
                at: i as f64,
                why: Why::Clipboard,
            })
            .collect();
        let out = bump(&items, "/f10", Why::Filefast, 99.0, 25);
        assert_eq!(out.len(), 25);
        assert_eq!(out[0], Item { path: "/f10".into(), at: 99.0, why: Why::Filefast });
        let out = bump(&out, "/f5", Why::Modified, 100.0, 25);
        assert_eq!(out[0].why, Why::Clipboard, "an edit keeps the last why");
        let out = bump(&out, "/new", Why::Screenshot, 101.0, 25);
        assert_eq!(out[0].path, "/new");
        assert_eq!(out.len(), 25);

        let raw = vec![
            json!({"path": "/a", "at": 1.0, "why": "clipboard"}),
            json!({"path": "/b", "at": 3.0, "why": "clipboard"}),
            json!({"path": "/a", "at": 2.0, "why": "clipboard"}),
        ];
        let cands = load_candidates(&raw);
        // load_candidates only keeps paths that exist; /a and /b don't.
        let _ = cands;
        let fin = finalize(
            &[
                Item { path: "/a".into(), at: 1.0, why: Why::Clipboard },
                Item { path: "/b".into(), at: 3.0, why: Why::Clipboard },
                Item { path: "/a".into(), at: 2.0, why: Why::Clipboard },
            ],
            25,
        );
        let ps: Vec<String> = fin.iter().map(|i| i.path.clone()).collect();
        assert_eq!(ps, vec!["/b", "/a"]);
        assert_eq!(
            finalize(
                &[Item { path: "/a".into(), at: 1.0, why: Why::Clipboard }],
                1
            )
            .len(),
            1
        );

        // save + reload round trip
        let root = tmp("save");
        let path = format!("{root}/paths.json");
        let f = format!("{root}/f.txt");
        write_file(&f, "x");
        let it = vec![Item { path: f.clone(), at: 2.0, why: Why::Clipboard }];
        save(&path, &it);
        let raw: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let raw_arr = raw.as_array().cloned().unwrap_or_default();
        let got = finalize(&load_candidates(&raw_arr), 25);
        assert_eq!(
            got,
            vec![Item { path: canonical(&f).unwrap().path, at: 2.0, why: Why::Clipboard }]
        );
    }

    #[test]
    fn load_candidates_activity_rules() {
        if !helper_ready() {
            eprintln!("skipping: no python 3.11+ or pylib/helper not found");
            return;
        }
        let root = tmp("load");
        let f = format!("{root}/f.txt");
        write_file(&f, "x");
        let raw = vec![
            json!({"path": f, "at": 1.0, "why": "created"}),
            json!({"path": root, "at": 2.0, "why": "modified"}),
            json!({"path": root, "at": 3.0, "why": "copied"}),
            json!({"path": format!("{root}/gone"), "at": 4.0, "why": "created"}),
            json!({"path": f, "at": 5.0, "why": "bogus"}),
            json!({"path": 7, "at": 6.0, "why": "created"}),
        ];
        let out = load_candidates(&raw);
        let got: Vec<(String, &str)> = out
            .iter()
            .map(|i| {
                (
                    Path::new(&i.path)
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    i.why.raw(),
                )
            })
            .collect();
        let fb = Path::new(&f).file_name().unwrap().to_string_lossy().into_owned();
        let rb = Path::new(&root).file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(
            got,
            vec![
                (fb.clone(), "created"),
                (rb, "copied"),
                (fb, "modified"),
            ]
        );
    }

    #[test]
    fn shelf_model() {
        if !helper_ready() {
            eprintln!("skipping: no python 3.11+ or pylib/helper not found");
            return;
        }
        let root = tmp("model");
        let store = format!("{root}/paths.json");
        let dir = format!("{root}/files");

        let mut rules = IgnoreRules::new(&root, None);
        rules.set_git_excludes(Some(&format!("{root}/no-such-global")));
        rules.set_recheck(0.0);
        let mut s = PathShelf::with_rules(store.clone(), rules);
        s.immediate = true;
        s.configure(99, None);
        assert_eq!(s.limit, 25, "limit clamps to 25");

        let mut files: Vec<String> = Vec::new();
        for i in 0..30 {
            let f = format!("{dir}/f{i}.txt");
            write_file(&f, "x");
            files.push(f.clone());
            s.add(&[f], Why::Clipboard);
        }
        s.sync();
        let mut e = s.entries();
        assert_eq!(e.len(), 25, "cap 25");
        assert_eq!(e.first().unwrap().path, files[29]);
        assert_eq!(e.last().unwrap().path, files[5], "newest first; oldest five fell off");

        s.add(&[files[10].clone()], Why::Filefast);
        s.sync();
        e = s.entries();
        assert_eq!(e[0].path, files[10]);
        assert_eq!(e[0].why, Why::Filefast, "re-add moves to the top, says why");
        let uniq: std::collections::HashSet<&String> = e.iter().map(|i| &i.path).collect();
        assert_eq!(uniq.len(), e.len(), "no duplicates");

        s.observe(&files[10], false, None);
        s.sync();
        assert_eq!(s.entries()[0].why, Why::Filefast, "an edit keeps 'filefast'");

        write_file(&format!("{dir}/junk.pyc"), "x");
        fs::write(format!("{root}/shelf.ignore"), "*.pyc\n").unwrap();
        s.rules.set_shelf(Some(&format!("{root}/shelf.ignore")));
        s.observe(&format!("{dir}/junk.pyc"), true, None);
        s.observe(&dir, true, None);
        s.add(&[format!("{dir}/nope.txt")], Why::Clipboard);
        s.sync();
        e = s.entries();
        assert!(!e.iter().any(|i| i.path.ends_with("junk.pyc")), "ignored activity is dropped");
        assert!(!e.iter().any(|i| i.path == dir), "folders from activity are dropped");
        assert!(!e.iter().any(|i| i.path.ends_with("nope.txt")), "a missing path is never added");

        s.add(&[format!("{dir}/junk.pyc")], Why::Clipboard);
        s.sync();
        assert_eq!(
            s.entries()[0].path,
            format!("{dir}/junk.pyc"),
            "a COPIED path skips the ignore rules"
        );

        let dl = format!("{dir}/report.pdf");
        write_file(&dl, "x");
        s.observe(&dl, true, Some("Safari · example.com"));
        s.sync();
        assert_eq!(s.entries()[0].why, Why::Downloaded, "activity with an origin = downloaded");

        let before: Vec<String> = s.entries().iter().map(|i| i.path.clone()).collect();
        let moved = format!("{dir}/renamed.pdf");
        fs::rename(&dl, &moved).unwrap();
        s.renamed(&dl, &moved);
        s.sync();
        let expect: Vec<String> = before
            .iter()
            .map(|p| if p == &dl { moved.clone() } else { p.clone() })
            .collect();
        assert_eq!(s.entries().iter().map(|i| i.path.clone()).collect::<Vec<_>>(), expect, "rename re-keys in place");

        fs::remove_file(&files[29]).unwrap();
        assert!(!s.entries().iter().any(|i| i.path == files[29]), "deleted files drop out");
        s.remove(&[files[28].clone()]);
        s.sync();
        assert!(!s.entries().iter().any(|i| i.path == files[28]), "remove = forget");

        let mut reloaded = PathShelf::with_store(&store);
        reloaded.configure(25, Some(&format!("{root}/shelf.ignore")));
        assert_eq!(
            reloaded.entries().iter().map(|i| i.path.clone()).collect::<Vec<_>>(),
            s.entries().iter().map(|i| i.path.clone()).collect::<Vec<_>>(),
            "paths.json round trip"
        );
        reloaded.configure(3, Some(&format!("{root}/shelf.ignore")));
        assert_eq!(reloaded.entries().len(), 3, "a smaller limit trims");

        let mut fresh = PathShelf::with_store(&format!("{root}/fresh.json"));
        fresh.immediate = true;
        fresh.configure(25, Some(&format!("{root}/shelf.ignore")));
        let t = next_stamp();
        fresh.seed(&[
            (format!("{dir}/junk.pyc"), t, None),
            (dir.clone(), t, None),
            (files[1].clone(), t, Some("AirDrop".into())),
        ]);
        fresh.sync();
        assert_eq!(
            fresh.entries().iter().map(|i| i.path.clone()).collect::<Vec<_>>(),
            vec![files[1].clone()]
        );
        assert_eq!(fresh.entries()[0].why, Why::Downloaded, "seed: rules + files only");

        assert_eq!(normalize("/tmp/a/../b"), "/private/tmp/b");

        let link = format!("{root}/link");
        std::os::unix::fs::symlink(&dir, &link).unwrap();
        s.observe(&format!("{link}/f3.txt"), false, None);
        s.add(&[format!("{link}/f4.txt")], Why::Clipboard);
        s.sync();
        let ps: Vec<String> = s.entries().iter().map(|i| i.path.clone()).collect();
        let sel: Vec<String> = ps
            .iter()
            .filter(|p| p.ends_with("/f3.txt") || p.ends_with("/f4.txt"))
            .cloned()
            .collect();
        assert_eq!(sel, vec![format!("{dir}/f4.txt"), format!("{dir}/f3.txt")]);
        assert!(!ps.iter().any(|p| p.starts_with(&link)), "no row under the symlink");

        let sock = format!("{root}/s.sock");
        let _l = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        s.observe(&sock, true, None);
        s.sync();
        assert!(
            Path::new(&sock).exists() && !s.entries().iter().any(|i| i.path == sock),
            "a socket is not a file"
        );
        assert_eq!(normalize("file:///Users/x/a%20b.txt"), "/Users/x/a b.txt");
    }

    #[derive(Default)]
    struct FakePasteboard {
        count: i64,
        types: Vec<String>,
        urls: Vec<String>,
        text: Option<String>,
    }

    impl FakePasteboard {
        fn set_text(&mut self, t: &str) {
            self.count += 1;
            self.types = vec![STRING_TYPE.to_string()];
            self.text = Some(t.to_string());
        }
    }

    struct SharedPasteboard(Rc<RefCell<FakePasteboard>>);

    impl Pasteboard for SharedPasteboard {
        fn change_count(&self) -> i64 {
            self.0.borrow().count
        }
        fn types(&self) -> Vec<String> {
            self.0.borrow().types.clone()
        }
        fn file_urls(&self) -> Vec<String> {
            self.0.borrow().urls.clone()
        }
        fn string(&self) -> Option<String> {
            self.0.borrow().text.clone()
        }
    }

    impl Pasteboard for FakePasteboard {
        fn change_count(&self) -> i64 {
            self.count
        }
        fn types(&self) -> Vec<String> {
            self.types.clone()
        }
        fn file_urls(&self) -> Vec<String> {
            self.urls.clone()
        }
        fn string(&self) -> Option<String> {
            self.text.clone()
        }
    }

    #[test]
    fn clipboard_text_and_change_count() {
        if !helper_ready() {
            eprintln!("skipping: no python 3.11+ or pylib/helper not found");
            return;
        }
        let root = tmp("clip");
        let a = format!("{root}/clip/a file.txt");
        let b = format!("{root}/clip/b.txt");
        write_file(&a, "x");
        write_file(&b, "x");

        let t = |s: &str| ClipboardPaths::paths_in_text(s);
        assert_eq!(t(&a), vec![a.clone()], "one path");
        assert_eq!(t(&format!("  {a}  \n")), vec![a.clone()], "whitespace / trailing newline");
        assert_eq!(t(&format!("'{a}'")), vec![a.clone()]);
        assert_eq!(t(&format!("\"{a}\"")), vec![a.clone()]);
        assert_eq!(t(&a.replace(' ', "\\ ")), vec![a.clone()], "shell-escaped spaces");
        let url = format!("file://{a}").replace(' ', "%20");
        assert_eq!(t(&url), vec![a.clone()], "file:// URL");
        assert_eq!(t(&format!("{a}\n{b}")), vec![a.clone(), b.clone()], "two lines");
        assert!(t(&vec![b.clone(); 6].join("\n")).is_empty(), "more than 5 lines");
        assert!(t(&format!("see {a} for details")).is_empty(), "prose ignored");
        assert!(t(&format!("{a}\nhello")).is_empty(), "one non-path rejects all");
        assert!(t(&format!("{root}/clip/missing.txt")).is_empty(), "missing ignored");
        assert!(t("relative/path.txt").is_empty(), "relative ignored");
        assert!(t(&"x".repeat(5000)).is_empty(), "huge text ignored");

        // pasteboard model
        let mut pbf = FakePasteboard::default();
        pbf.count = 1;
        pbf.types = vec![FILE_URL_TYPE.to_string()];
        pbf.urls = vec![a.clone(), b.clone()];
        assert_eq!(ClipboardPaths::paths_in(&pbf), vec![a.clone(), b.clone()], "file URLs");

        let mut pbs = FakePasteboard::default();
        pbs.set_text(&a);
        assert_eq!(ClipboardPaths::paths_in(&pbs), vec![a.clone()], "text path");

        let mut pbc = FakePasteboard::default();
        pbc.count = 1;
        pbc.types = vec![STRING_TYPE.to_string(), "org.nspasteboard.ConcealedType".to_string()];
        pbc.text = Some(a.clone());
        assert!(ClipboardPaths::paths_in(&pbc).is_empty(), "concealed copies skipped");

        let shared = Rc::new(RefCell::new(FakePasteboard::default()));
        let mut w = ClipboardPaths::new(Box::new(SharedPasteboard(shared.clone())));
        let got: Rc<RefCell<Vec<Vec<String>>>> = Rc::new(RefCell::new(Vec::new()));
        let g = got.clone();
        w.on_paths = Some(Box::new(move |p| g.borrow_mut().push(p)));
        shared.borrow_mut().set_text(&b);
        w.check();
        w.check();
        assert_eq!(*got.borrow(), vec![vec![b.clone()]], "one change → one report");
        shared.borrow_mut().set_text(&a);
        w.own_write();
        w.check();
        assert_eq!(got.borrow().len(), 1, "our own copy is not reported");
        shared.borrow_mut().set_text("not a path");
        w.check();
        assert_eq!(got.borrow().len(), 1, "junk is not reported");
    }

    #[test]
    fn feeds_from_recent_events() {
        if !helper_ready() {
            eprintln!("skipping: no python 3.11+ or pylib/helper not found");
            return;
        }
        let root = tmp("feed");
        let f = format!("{root}/a.txt");
        let g = format!("{root}/b.txt");
        write_file(&f, "x");

        let mut rules = IgnoreRules::new(&root, None);
        rules.set_recheck(0.0);
        let mut shelf = PathShelf::with_rules(format!("{root}/paths.json"), rules);
        shelf.immediate = true;
        shelf.configure(25, None);
        let shared = Rc::new(RefCell::new(shelf));

        let mut recent = RecentFiles::new(&root, &format!("{root}/recent.json"));
        recent.configure(false, 7, 200, &[], false);
        PathShelf::wire_recent(shared.clone(), &mut recent);

        let created = FSEVENT_ITEM_CREATED | FSEVENT_ITEM_IS_FILE;
        recent.handle(&[f.clone()], &[created], &[Some(1)]);
        assert_eq!(
            shared.borrow().entries().iter().map(|i| i.path.clone()).collect::<Vec<_>>(),
            vec![f.clone()],
            "onKept feeds the shelf"
        );

        fs::rename(&f, &g).unwrap();
        let renamed = FSEVENT_ITEM_RENAMED | FSEVENT_ITEM_IS_FILE;
        recent.handle(&[f.clone(), g.clone()], &[renamed, renamed], &[Some(1), Some(1)]);
        assert_eq!(
            shared.borrow().entries().iter().map(|i| i.path.clone()).collect::<Vec<_>>(),
            vec![g.clone()],
            "onRenamed re-keys the shelf"
        );
    }
}
