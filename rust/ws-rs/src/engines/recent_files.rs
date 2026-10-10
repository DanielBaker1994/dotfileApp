//! Port of `RecentFiles.swift` — the model half first (the pure, unit-tested
//! behavior): `canonical` (folder realpath + name), `present` (exact-case
//! check), the in-scope prefix filter, `keep`, the `entries()` snapshot,
//! inode rename pairing (`departed`), `ownChange`'s patch and `recent.json`
//! load/save.
//!
//! The live FSEvents stream is [`RecentFiles::start`] / [`RecentFiles::stop`]:
//! the CoreServices C API through raw `extern "C"` (no new crate dependency).
//! The callback forwards paths / flags / inode ids into the same [`handle`]
//! entry point the Swift suite drives with synthetic events.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::ffi::{c_void, CString};
use std::fs;
use std::os::raw::c_char;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// `FSEventStreamEventFlags` bits (CoreServices `FSEvents.h`). Only the ones
/// `handle` reads are declared; the Swift suite's synthetic events use these.
pub const FSEVENT_MUST_SCAN_SUBDIRS: u32 = 0x0000_0001;
pub const FSEVENT_ITEM_CREATED: u32 = 0x0000_0100;
pub const FSEVENT_ITEM_REMOVED: u32 = 0x0000_0200;
pub const FSEVENT_ITEM_INODE_META_MOD: u32 = 0x0000_0400;
pub const FSEVENT_ITEM_RENAMED: u32 = 0x0000_0800;
pub const FSEVENT_ITEM_MODIFIED: u32 = 0x0000_1000;
pub const FSEVENT_ITEM_IS_FILE: u32 = 0x0001_0000;
pub const FSEVENT_ITEM_IS_DIR: u32 = 0x0002_0000;
pub const FSEVENT_ITEM_IS_SYMLINK: u32 = 0x0004_0000;
pub const FSEVENT_OWN_EVENT: u32 = 0x0008_0000;

// --- CoreServices FSEvents / CoreFoundation FFI (the live watcher) ----------
//
// Raw declarations so this crate keeps its dependency set. `FSEventStreamCreate`
// gets the Swift flags (`UseCFTypes`+`UseExtendedData` for the CFDictionary
// extended data, `IgnoreSelf` for self-events, `FileEvents`); the callback
// forwards into `RecentFiles::handle` exactly as the Swift callback does.

const K_FSEVENT_STREAM_CREATE_USE_CF_TYPES: u32 = 0x0000_0001;
const K_FSEVENT_STREAM_CREATE_IGNORE_SELF: u32 = 0x0000_0008;
const K_FSEVENT_STREAM_CREATE_FILE_EVENTS: u32 = 0x0000_0010;
const K_FSEVENT_STREAM_CREATE_USE_EXTENDED_DATA: u32 = 0x0000_0040;
const K_FSEVENT_STREAM_EVENT_ID_SINCE_NOW: u64 = u64::MAX;
const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
const K_CF_NUMBER_SINT64_TYPE: i64 = 4;

#[repr(C)]
struct FSEventStreamContext {
    version: isize,
    info: *mut c_void,
    // Callback pointers are never invoked; data pointers keep the ABI layout.
    retain: *const c_void,
    release: *const c_void,
    copy_description: *const c_void,
}

#[repr(C)]
struct CFArrayCallBacks {
    version: isize,
    retain: *const c_void,
    release: *const c_void,
    copy_description: *const c_void,
    equal: *const c_void,
}

type FSEventStreamCallback = unsafe extern "C" fn(
    stream_ref: *const c_void,
    client_info: *mut c_void,
    num_events: usize,
    event_paths: *mut c_void,
    event_flags: *const u32,
    event_ids: *const u64,
);

#[link(name = "CoreServices", kind = "framework")]
extern "C" {
    fn FSEventStreamCreate(
        allocator: *const c_void,
        callback: FSEventStreamCallback,
        context: *const FSEventStreamContext,
        paths_to_watch: *const c_void,
        since_when: u64,
        latency: f64,
        flags: u32,
    ) -> *mut c_void;
    fn FSEventStreamScheduleWithRunLoop(
        stream: *mut c_void,
        run_loop: *mut c_void,
        run_loop_mode: *const c_void,
    );
    fn FSEventStreamStart(stream: *mut c_void) -> u8;
    fn FSEventStreamStop(stream: *mut c_void);
    fn FSEventStreamInvalidate(stream: *mut c_void);
    fn FSEventStreamRelease(stream: *mut c_void);
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFArrayCreate(
        allocator: *const c_void,
        values: *const *const c_void,
        num_values: isize,
        callbacks: *const c_void,
    ) -> *const c_void;
    fn CFArrayGetValueAtIndex(array: *const c_void, index: isize) -> *const c_void;
    fn CFDictionaryGetValue(dict: *const c_void, key: *const c_void) -> *const c_void;
    fn CFStringCreateWithCString(
        allocator: *const c_void,
        c_str: *const c_char,
        encoding: u32,
    ) -> *const c_void;
    fn CFStringGetLength(string: *const c_void) -> isize;
    fn CFStringGetMaximumSizeForEncoding(length: isize, encoding: u32) -> isize;
    fn CFStringGetCString(
        string: *const c_void,
        buffer: *mut c_char,
        buffer_size: isize,
        encoding: u32,
    ) -> u8;
    fn CFNumberGetValue(number: *const c_void, the_type: i64, value_ptr: *mut c_void) -> u8;
    fn CFRelease(cf: *const c_void);
    fn CFRunLoopGetMain() -> *mut c_void;
    static kCFRunLoopDefaultMode: *const c_void;
    static kCFTypeArrayCallBacks: CFArrayCallBacks;
}

/// The Swift `FSEventStreamCreate` flags, verbatim.
fn create_flags() -> u32 {
    K_FSEVENT_STREAM_CREATE_FILE_EVENTS
        | K_FSEVENT_STREAM_CREATE_USE_CF_TYPES
        | K_FSEVENT_STREAM_CREATE_USE_EXTENDED_DATA
        | K_FSEVENT_STREAM_CREATE_IGNORE_SELF
}

unsafe fn make_cfstring(s: &str) -> *const c_void {
    let Ok(c) = CString::new(s) else {
        return std::ptr::null();
    };
    unsafe { CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), K_CF_STRING_ENCODING_UTF8) }
}

unsafe fn cfstring_to_string(s: *const c_void) -> Option<String> {
    if s.is_null() {
        return None;
    }
    let len = unsafe { CFStringGetLength(s) };
    let max = unsafe { CFStringGetMaximumSizeForEncoding(len, K_CF_STRING_ENCODING_UTF8) } + 1;
    if max <= 1 {
        return None;
    }
    let mut buf = vec![0u8; max as usize];
    let ok = unsafe {
        CFStringGetCString(
            s,
            buf.as_mut_ptr() as *mut c_char,
            max,
            K_CF_STRING_ENCODING_UTF8,
        )
    };
    if ok == 0 {
        return None;
    }
    let nul = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    Some(String::from_utf8_lossy(&buf[..nul]).into_owned())
}

unsafe fn cfnumber_to_u64(n: *const c_void) -> Option<u64> {
    if n.is_null() {
        return None;
    }
    let mut v: i64 = 0;
    let ok = unsafe { CFNumberGetValue(n, K_CF_NUMBER_SINT64_TYPE, &mut v as *mut _ as *mut c_void) };
    if ok != 0 {
        Some(v as u64)
    } else {
        None
    }
}

unsafe fn build_paths_array(roots: &[String]) -> *const c_void {
    let strs: Vec<*const c_void> = roots.iter().map(|r| unsafe { make_cfstring(r) }).collect();
    let array = unsafe {
        CFArrayCreate(
            std::ptr::null(),
            strs.as_ptr(),
            strs.len() as isize,
            &kCFTypeArrayCallBacks as *const _ as *const c_void,
        )
    };
    for s in strs {
        unsafe { CFRelease(s) };
    }
    array
}

/// The `FSEventStreamCallback`: extended-data CFDictionary → `handle`.
unsafe extern "C" fn fsevents_callback(
    _stream: *const c_void,
    info: *mut c_void,
    count: usize,
    paths: *mut c_void,
    flags: *const u32,
    _event_ids: *const u64,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        forward_events(info, count, paths, flags);
    }));
}

unsafe fn forward_events(info: *mut c_void, count: usize, paths: *mut c_void, flags: *const u32) {
    if info.is_null() || paths.is_null() || flags.is_null() || count == 0 {
        return;
    }
    // The extended-data keys are `CFSTR("path")` / `CFSTR("fileID")`; a
    // value-equal CFString resolves them in the CFDictionary.
    let path_key = unsafe { make_cfstring("path") };
    let fileid_key = unsafe { make_cfstring("fileID") };
    let mut path_vec: Vec<String> = Vec::with_capacity(count);
    let mut id_vec: Vec<Option<u64>> = Vec::with_capacity(count);
    for i in 0..count {
        let dict = unsafe { CFArrayGetValueAtIndex(paths, i as isize) };
        let p = unsafe { CFDictionaryGetValue(dict, path_key) };
        path_vec.push(unsafe { cfstring_to_string(p) }.unwrap_or_default());
        let fid = unsafe { CFDictionaryGetValue(dict, fileid_key) };
        id_vec.push(unsafe { cfnumber_to_u64(fid) });
    }
    unsafe {
        CFRelease(path_key);
        CFRelease(fileid_key);
    }
    let flag_slice = unsafe { std::slice::from_raw_parts(flags, count) };
    let me = unsafe { &mut *(info as *mut RecentFiles) };
    me.handle(&path_vec, flag_slice, &id_vec);
}

struct StreamHandle {
    stream: *mut c_void,
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub at: f64,
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub path: String,
    pub at: f64,
    pub source: Option<String>,
}

struct Departed {
    at: f64,
    items: HashMap<String, Item>,
    path: String,
}

const SYSTEM_ROOTS: &[&str] = &[
    "/System/",
    "/Library/",
    "/private/var/",
    "/private/etc/",
    "/var/",
    "/etc/",
    "/usr/",
    "/bin/",
    "/sbin/",
    "/opt/",
    "/cores/",
    "/dev/",
    "/Volumes/",
    "/Applications/",
    "/nix/",
    "/private/preboot/",
    "/private/xarts/",
    "/.",
];

const NOISE_DIRS: &[&str] = &[
    "node_modules",
    "DerivedData",
    "__pycache__",
    "site-packages",
    "Pods",
    "venv",
    "Caches",
    "CachedData",
    "logs",
    "xcuserdata",
];

const PACKAGE_EXTS: &[&str] = &[
    "photoslibrary",
    "photolibrary",
    "migratedphotolibrary",
    "aplibrary",
    "musiclibrary",
    "tvlibrary",
    "app",
    "bundle",
    "framework",
    "plugin",
    "kext",
    "xcarchive",
    "xcodeproj",
    "xcworkspace",
    "playground",
    "sparsebundle",
    "photobooth",
];

const PACKAGE_NAMES: &[&str] = &["Photo Booth Library"];

const NOISE_EXTS: &[&str] = &[
    "crdownload",
    "part",
    "download",
    "partial",
    "swp",
    "swo",
    "swx",
    "tmp",
    "lock",
    "pid",
    "sock",
    "socket",
    "db-journal",
    "db-wal",
    "db-shm",
    "sqlite-journal",
    "sqlite-wal",
    "sqlite-shm",
];

/// `NSString.pathComponents` (leading "/" is its own component).
fn path_components(p: &str) -> Vec<String> {
    let mut comps: Vec<String> = Vec::new();
    if p.starts_with('/') {
        comps.push("/".to_string());
    }
    for part in p.split('/') {
        if !part.is_empty() {
            comps.push(part.to_string());
        }
    }
    comps
}

/// `NSString.pathExtension.lowercased()` (a leading dot is not an extension).
fn path_extension(name: &str) -> String {
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() => name[i + 1..].to_lowercase(),
        _ => String::new(),
    }
}

fn expand_tilde(p: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    if p == "~" {
        return home;
    }
    if let Some(rest) = p.strip_prefix("~/") {
        return format!("{home}/{rest}");
    }
    p.to_string()
}

fn fnmatch_str(pattern: &str, name: &str) -> bool {
    let (Ok(cp), Ok(cn)) = (CString::new(pattern), CString::new(name)) else {
        return false;
    };
    unsafe { libc::fnmatch(cp.as_ptr(), cn.as_ptr(), 0) == 0 }
}

fn in_scope(home: &str, everywhere: bool, p: &str) -> bool {
    if p.starts_with(&format!("{home}/")) {
        return !p.starts_with(&format!("{home}/Library/"));
    }
    if p.starts_with("/private/tmp/") {
        return true;
    }
    if !everywhere {
        return false;
    }
    if p.starts_with("/Users/") {
        return p.starts_with("/Users/Shared/");
    }
    for r in SYSTEM_ROOTS {
        if p.starts_with(r) {
            return false;
        }
    }
    true
}

fn keep(home: &str, everywhere: bool, excludes: &[String], p: &str) -> bool {
    if !in_scope(home, everywhere, p) {
        return false;
    }
    let comps = path_components(p);
    for c in comps.iter().skip(1) {
        if c.starts_with('.') || NOISE_DIRS.contains(&c.as_str()) || PACKAGE_NAMES.contains(&c.as_str())
        {
            return false;
        }
        if PACKAGE_EXTS.contains(&path_extension(c).as_str()) {
            return false;
        }
    }
    if p.starts_with(&format!("{home}/Music/Music/")) {
        return false;
    }
    let name = comps.last().cloned().unwrap_or_default();
    let ext = path_extension(&name);
    if NOISE_EXTS.contains(&ext.as_str()) || name.ends_with('~') || name == "4913" {
        return false;
    }
    if p.starts_with("/private/tmp/") && !p.starts_with(&format!("{home}/")) {
        let top = comps.get(3).cloned().unwrap_or_default();
        if top.starts_with("com.apple")
            || top.starts_with("claude")
            || top.starts_with("tmp")
            || top.starts_with("ws-")
            || top.starts_with("kitchen-sink")
            || top.starts_with("jira-poll")
        {
            return false;
        }
    }
    for g in excludes {
        if g.is_empty() {
            continue;
        }
        if fnmatch_str(g, p) || fnmatch_str(g, &name) || p.starts_with(g.as_str()) {
            return false;
        }
    }
    true
}

fn take(items: &mut HashMap<String, Item>, p: &str) -> HashMap<String, Item> {
    let prefix = format!("{p}/");
    let keys: Vec<String> = items
        .keys()
        .filter(|k| k.as_str() == p || k.starts_with(&prefix))
        .cloned()
        .collect();
    let mut gone = HashMap::new();
    for k in keys {
        if let Some(it) = items.remove(&k) {
            gone.insert(k[p.len()..].to_string(), it);
        }
    }
    gone
}

fn put(items: &mut HashMap<String, Item>, p: &str, it: Item) {
    match items.get_mut(p) {
        Some(have) => {
            have.at = have.at.max(it.at);
            if have.source.is_none() {
                have.source = it.source;
            }
        }
        None => {
            items.insert(p.to_string(), it);
        }
    }
}

fn stamp(p: &str) -> Option<f64> {
    let c = CString::new(p).ok()?;
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::stat(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    Some((st.st_birthtime as f64).max(st.st_mtime as f64))
}

fn run_lines(exe: &str, args: &[String]) -> Vec<String> {
    crate::app::process_run::run_process(exe, args, None, None, false)
        .map(|o| o.out.split('\n').map(str::to_string).collect())
        .unwrap_or_default()
}

fn scan_dir(
    home: &str,
    everywhere: bool,
    excludes: &[String],
    dir: &str,
    depth: i32,
    since: f64,
    found: &mut HashMap<String, f64>,
) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for n in rd.flatten() {
        let p = n.path().to_string_lossy().into_owned();
        if !keep(home, everywhere, excludes, &p) {
            continue;
        }
        let Ok(meta) = fs::symlink_metadata(&p) else { continue };
        if meta.file_type().is_symlink() {
            continue;
        }
        let is_dir = meta.is_dir();
        if let Some(t) = stamp(&p) {
            if t >= since && !is_dir {
                found.insert(p.clone(), t);
            }
        }
        if is_dir && depth > 0 {
            scan_dir(home, everywhere, excludes, &p, depth - 1, since, found);
        }
    }
}

fn xattr_data(p: &str, name: &str) -> Option<Vec<u8>> {
    let cp = CString::new(p).ok()?;
    let cn = CString::new(name).ok()?;
    let n = unsafe {
        libc::getxattr(
            cp.as_ptr(),
            cn.as_ptr(),
            std::ptr::null_mut(),
            0,
            0,
            libc::XATTR_NOFOLLOW,
        )
    };
    if n <= 0 {
        return None;
    }
    let mut buf = vec![0u8; n as usize];
    let got = unsafe {
        libc::getxattr(
            cp.as_ptr(),
            cn.as_ptr(),
            buf.as_mut_ptr() as *mut libc::c_void,
            n as usize,
            0,
            libc::XATTR_NOFOLLOW,
        )
    };
    if got != n {
        return None;
    }
    Some(buf)
}

fn xattr_string(p: &str, name: &str) -> Option<String> {
    xattr_data(p, name).map(|d| String::from_utf8_lossy(&d).into_owned())
}

/// Coarse host extraction from `com.apple.metadata:kMDItemWhereFroms`.
/// The real key is a plist (often binary); this scans the bytes for an
/// `http(s)://` URL and returns its host. Good enough for the origin label.
fn where_from_host(p: &str) -> Option<String> {
    let data = xattr_data(p, "com.apple.metadata:kMDItemWhereFroms")?;
    let text = String::from_utf8_lossy(&data);
    for scheme in ["https://", "http://"] {
        if let Some(idx) = text.find(scheme) {
            let rest = &text[idx + scheme.len()..];
            let end = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || ".-_".contains(c)))
                .unwrap_or(rest.len());
            let host = &rest[..end];
            if !host.is_empty() {
                return Some(host.strip_prefix("www.").unwrap_or(host).to_string());
            }
        }
    }
    None
}

pub struct RecentFiles {
    pub enabled: bool,
    limit: usize,
    days: i64,
    everywhere: bool,
    excludes: Vec<String>,
    items: HashMap<String, Item>,
    snapshot: Vec<Entry>,
    departed: HashMap<u64, Departed>,
    home: String,
    store: String,
    stream: Option<StreamHandle>,
    pub on_kept: Option<Box<dyn FnMut(&str, bool, Option<&str>)>>,
    pub on_renamed: Option<Box<dyn FnMut(&str, &str)>>,
}

impl RecentFiles {
    pub fn new(home: &str, store: &str) -> Self {
        RecentFiles {
            enabled: false,
            limit: 200,
            days: 7,
            everywhere: true,
            excludes: Vec::new(),
            items: HashMap::new(),
            snapshot: Vec::new(),
            departed: HashMap::new(),
            home: home.to_string(),
            store: store.to_string(),
            stream: None,
            on_kept: None,
            on_renamed: None,
        }
    }

    /// Mirror of `configure(...)`. The live FSEvents watcher is not started
    /// here (see [`RecentFiles::start`]); enabling loads `recent.json` and
    /// publishes so `entries()` works.
    pub fn configure(
        &mut self,
        enabled: bool,
        days: i64,
        limit: usize,
        excludes: &[String],
        everywhere: bool,
    ) {
        self.days = days.max(1);
        self.limit = limit.max(20);
        self.everywhere = everywhere;
        self.excludes = excludes.iter().map(|e| expand_tilde(e)).collect();
        let was = self.enabled;
        self.enabled = enabled;
        if enabled && !was {
            self.load();
            self.publish();
        }
    }

    pub fn entries(&self, arrived_only: bool) -> Vec<Entry> {
        self.snapshot
            .iter()
            .filter(|e| !arrived_only || e.source.is_some())
            .take(self.limit)
            .cloned()
            .collect()
    }

    pub fn own_change(&mut self, old: Option<&str>, new: &str) {
        if !self.enabled {
            return;
        }
        match old {
            Some(o) => self.fire_renamed(o, new),
            None => self.fire_kept(new, true, None),
        }
        if let Some(o) = old {
            let (o, n) = (o.to_string(), new.to_string());
            let home = self.home.clone();
            let everywhere = self.everywhere;
            let excludes = self.excludes.clone();
            self.snapshot = self
                .snapshot
                .iter()
                .filter_map(|e| match Self::rekeyed(&e.path, &o, &n) {
                    None => Some(e.clone()),
                    Some(p) => {
                        if keep(&home, everywhere, &excludes, &p) {
                            Some(Entry {
                                path: p,
                                at: e.at,
                                source: e.source.clone(),
                            })
                        } else {
                            None
                        }
                    }
                })
                .collect();
        }
        let carried = match old {
            Some(o) => self.rekey(o, new),
            None => false,
        };
        if !carried && keep(&self.home, self.everywhere, &self.excludes, new) && Path::new(new).exists()
        {
            self.items.insert(
                new.to_string(),
                Item {
                    at: now(),
                    source: Self::origin(new),
                },
            );
        }
        self.trim();
        self.publish();
    }

    /// `URL(...).nameKey == lastPathComponent` — the case-only-rename check.
    pub fn present(p: &str) -> bool {
        if !Path::new(p).exists() {
            return false;
        }
        let path = Path::new(p);
        let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
            return true;
        };
        let name = name.to_string_lossy();
        let Ok(rd) = fs::read_dir(parent) else {
            return true;
        };
        for e in rd.flatten() {
            if e.file_name().to_string_lossy() == name {
                return true;
            }
        }
        false
    }

    pub fn rekeyed(p: &str, old: &str, new: &str) -> Option<String> {
        if p == old {
            return Some(new.to_string());
        }
        if let Some(rest) = p.strip_prefix(&format!("{old}/")) {
            return Some(format!("{new}/{rest}"));
        }
        None
    }

    fn rekey(&mut self, old: &str, new: &str) -> bool {
        let gone = take(&mut self.items, old);
        for (suffix, it) in gone.iter() {
            put(&mut self.items, &format!("{new}{suffix}"), it.clone());
        }
        !gone.is_empty()
    }

    pub fn handle(&mut self, paths: &[String], flags: &[u32], ids: &[Option<u64>]) {
        let t = now();
        self.departed.retain(|_, d| t - d.at < 10.0);
        let mut touched = false;
        let mut kept_events: Vec<(String, bool, Option<String>)> = Vec::new();
        let mut renamed_events: Vec<(String, String)> = Vec::new();
        for (i, raw) in paths.iter().enumerate() {
            if i >= flags.len() {
                break;
            }
            let p = if raw.starts_with("/tmp/") {
                format!("/private{raw}")
            } else {
                raw.clone()
            };
            if !in_scope(&self.home, self.everywhere, &p) {
                continue;
            }
            let f = flags[i];
            let id = ids.get(i).copied().flatten();
            if f & (FSEVENT_ITEM_REMOVED | FSEVENT_ITEM_RENAMED) != 0 && !Self::present(&p) {
                let gone = take(&mut self.items, &p);
                if !gone.is_empty() {
                    touched = true;
                }
                if let (Some(id), true) = (id, f & FSEVENT_ITEM_RENAMED != 0) {
                    self.departed.insert(
                        id,
                        Departed {
                            at: t,
                            items: gone,
                            path: p.clone(),
                        },
                    );
                }
                continue;
            }
            let is_file = f & FSEVENT_ITEM_IS_FILE != 0;
            let is_dir = f & FSEVENT_ITEM_IS_DIR != 0;
            let created = f & FSEVENT_ITEM_CREATED != 0;
            let renamed = f & FSEVENT_ITEM_RENAMED != 0;
            let modified = f & FSEVENT_ITEM_MODIFIED != 0;
            let ok = ((is_file && (created || renamed || modified)) || (is_dir && (created || renamed)))
                && keep(&self.home, self.everywhere, &self.excludes, &p)
                && Path::new(&p).exists();
            if !ok {
                continue;
            }
            let mut item = self
                .items
                .get(&p)
                .cloned()
                .unwrap_or(Item { at: t, source: None });
            item.at = t;
            if created || renamed || item.source.is_none() {
                if let Some(o) = Self::origin(&p) {
                    item.source = Some(o);
                }
            }
            let mut renamed_from: Option<String> = None;
            if renamed {
                if let Some(id) = id {
                    if let Some(was) = self.departed.remove(&id) {
                        if item.source.is_none() {
                            item.source = was.items.get("").and_then(|x| x.source.clone());
                        }
                        for (suffix, it) in was.items.iter() {
                            if !suffix.is_empty() {
                                put(&mut self.items, &format!("{p}{suffix}"), it.clone());
                            }
                        }
                        renamed_from = Some(was.path);
                    }
                }
            }
            let source = item.source.clone();
            self.items.insert(p.clone(), item);
            touched = true;
            if let Some(from) = renamed_from {
                renamed_events.push((from, p.clone()));
            }
            if is_file {
                kept_events.push((p.clone(), created, source));
            }
        }
        if touched {
            self.trim();
            self.publish();
        }
        for (from, to) in renamed_events {
            self.fire_renamed(&from, &to);
        }
        for (p, c, s) in kept_events {
            let sr = s.as_deref();
            self.fire_kept(&p, c, sr);
        }
    }

    pub fn origin(p: &str) -> Option<String> {
        let q = xattr_string(p, "com.apple.quarantine")?;
        let parts: Vec<&str> = q.split(';').collect();
        let mut agent = if parts.len() > 2 {
            parts[2].to_string()
        } else {
            String::new()
        };
        match agent.to_lowercase().as_str() {
            "sharingd" | "airdrop" => agent = "AirDrop".to_string(),
            "" => agent = "downloaded".to_string(),
            _ => {}
        }
        if let Some(host) = where_from_host(p) {
            if agent != "AirDrop" {
                return Some(format!("{agent} · {host}"));
            }
        }
        Some(agent)
    }

    fn trim(&mut self) {
        if self.items.len() <= self.limit * 2 {
            return;
        }
        let mut v: Vec<(String, Item)> = self.items.drain().collect();
        v.sort_by(|a, b| b.1.at.partial_cmp(&a.1.at).unwrap_or(std::cmp::Ordering::Equal));
        v.truncate(self.limit * 2);
        self.items = v.into_iter().collect();
    }

    pub fn load(&mut self) {
        let Ok(data) = fs::read(&self.store) else { return };
        let Ok(Value::Array(arr)) = serde_json::from_slice::<Value>(&data) else {
            return;
        };
        let since = now() - self.days as f64 * 86400.0;
        for d in arr {
            let Some(raw) = d.get("path").and_then(Value::as_str) else {
                continue;
            };
            let p = Self::canonical(raw);
            let Some(t) = d.get("at").and_then(Value::as_f64) else {
                continue;
            };
            if t < since || !keep(&self.home, self.everywhere, &self.excludes, &p) {
                continue;
            }
            if t > self.items.get(&p).map(|i| i.at).unwrap_or(0.0) {
                self.items.insert(
                    p,
                    Item {
                        at: t,
                        source: d.get("source").and_then(Value::as_str).map(str::to_string),
                    },
                );
            }
        }
    }

    pub fn save(&self) {
        let mut v: Vec<(&String, &Item)> = self.items.iter().collect();
        v.sort_by(|a, b| b.1.at.partial_cmp(&a.1.at).unwrap_or(std::cmp::Ordering::Equal));
        let arr: Vec<Value> = v
            .into_iter()
            .take(self.limit)
            .map(|(k, it)| {
                let mut o = json!({"path": k, "at": it.at});
                if let Some(s) = &it.source {
                    o["source"] = json!(s);
                }
                o
            })
            .collect();
        if let Some(dir) = Path::new(&self.store).parent() {
            let _ = fs::create_dir_all(dir);
        }
        if let Ok(data) = serde_json::to_vec(&Value::Array(arr)) {
            let _ = fs::write(&self.store, data);
        }
    }

    /// `seed()` — mdfind + a shallow `/private/tmp` scan. Not run by
    /// `configure` in this first cut (the watcher is unwired).
    pub fn seed(&mut self) {
        let since = now() - self.days as f64 * 86400.0;
        let secs = self.days * 86400;
        let mut found: HashMap<String, f64> = HashMap::new();
        let changed = format!(
            "kMDItemFSContentChangeDate >= $time.now(-{secs}) || kMDItemDateAdded >= $time.now(-{secs})"
        );
        for p in run_lines("/usr/bin/mdfind", &["-onlyin".into(), self.home.clone(), changed]) {
            if !keep(&self.home, self.everywhere, &self.excludes, &p) {
                continue;
            }
            if let Some(t) = stamp(&p) {
                if t >= since {
                    found.insert(Self::canonical(&p), t);
                }
            }
        }
        if self.everywhere {
            let downloaded =
                format!("kMDItemDateAdded >= $time.now(-{secs}) && kMDItemWhereFroms == \"*\"");
            for p in run_lines("/usr/bin/mdfind", &[downloaded]) {
                if !in_scope(&self.home, self.everywhere, &p)
                    || !keep(&self.home, self.everywhere, &self.excludes, &p)
                {
                    continue;
                }
                if let Some(t) = stamp(&p) {
                    if t >= since {
                        found.insert(Self::canonical(&p), t);
                    }
                }
            }
        }
        scan_dir(
            &self.home,
            self.everywhere,
            &self.excludes,
            "/private/tmp",
            1,
            since,
            &mut found,
        );
        for (p, t) in found {
            if self.items.get(&p).map(|i| i.at).unwrap_or(0.0) < t {
                let source = self
                    .items
                    .get(&p)
                    .and_then(|i| i.source.clone())
                    .or_else(|| Self::origin(&p));
                self.items.insert(p, Item { at: t, source });
            }
        }
        self.trim();
    }

    fn publish(&mut self) {
        let home = self.home.clone();
        let everywhere = self.everywhere;
        let excludes = self.excludes.clone();
        let mut v: Vec<(String, Item)> = self.items.iter().map(|(k, i)| (k.clone(), i.clone())).collect();
        v.sort_by(|a, b| b.1.at.partial_cmp(&a.1.at).unwrap_or(std::cmp::Ordering::Equal));
        self.snapshot = v
            .into_iter()
            .filter(|(p, _)| keep(&home, everywhere, &excludes, p) && Path::new(p).exists())
            .map(|(p, i)| Entry {
                path: p,
                at: i.at,
                source: i.source,
            })
            .collect();
        self.save();
    }

    pub fn canonical(p: &str) -> String {
        let path = Path::new(p);
        let dir = match path.parent() {
            Some(d) => d.to_string_lossy().into_owned(),
            None => String::new(),
        };
        if dir.is_empty() {
            return p.to_string();
        }
        match fs::canonicalize(&dir) {
            Ok(real) => {
                let real = real.to_string_lossy().into_owned();
                if real == dir {
                    p.to_string()
                } else {
                    match path.file_name() {
                        Some(n) => format!("{}/{}", real, n.to_string_lossy()),
                        None => p.to_string(),
                    }
                }
            }
            Err(_) => p.to_string(),
        }
    }

    fn fire_kept(&mut self, path: &str, created: bool, source: Option<&str>) {
        if let Some(mut cb) = self.on_kept.take() {
            cb(path, created, source);
            self.on_kept = Some(cb);
        }
    }

    fn fire_renamed(&mut self, from: &str, to: &str) {
        if let Some(mut cb) = self.on_renamed.take() {
            cb(from, to);
            self.on_renamed = Some(cb);
        }
    }

    /// Live FSEvents watcher. Loads + seeds the store first (as the Swift
    /// `start` does on its queue), then creates the stream over `/` (or the
    /// scoped roots) with the Swift flags and schedules it on the MAIN run
    /// loop, so callbacks run on the thread that owns this struct.
    ///
    /// Safety: the callback holds a raw pointer to `self`; the owner must not
    /// move or drop it while the stream runs (`stop`/`Drop` tear it down).
    pub fn start(&mut self) {
        if self.stream.is_some() {
            return;
        }
        self.enabled = true;
        self.load();
        self.seed();
        let keys: Vec<String> = self.items.keys().cloned().collect();
        for p in keys {
            if self.items.get(&p).map(|i| i.source.is_none()).unwrap_or(false) {
                if let Some(o) = Self::origin(&p) {
                    if let Some(it) = self.items.get_mut(&p) {
                        it.source = Some(o);
                    }
                }
            }
        }
        self.publish();

        let roots = if self.everywhere {
            vec!["/".to_string()]
        } else {
            vec![self.home.clone(), "/private/tmp".to_string()]
        };
        let ctx = FSEventStreamContext {
            version: 0,
            info: self as *mut RecentFiles as *mut c_void,
            retain: std::ptr::null(),
            release: std::ptr::null(),
            copy_description: std::ptr::null(),
        };
        let stream = unsafe {
            let paths = build_paths_array(&roots);
            let s = FSEventStreamCreate(
                std::ptr::null(),
                fsevents_callback,
                &ctx,
                paths,
                K_FSEVENT_STREAM_EVENT_ID_SINCE_NOW,
                1.0,
                create_flags(),
            );
            CFRelease(paths);
            if s.is_null() {
                return;
            }
            FSEventStreamScheduleWithRunLoop(s, CFRunLoopGetMain(), kCFRunLoopDefaultMode);
            FSEventStreamStart(s);
            s
        };
        self.stream = Some(StreamHandle { stream });
    }

    pub fn stop(&mut self) {
        self.enabled = false;
        if let Some(h) = self.stream.take() {
            unsafe {
                FSEventStreamStop(h.stream);
                FSEventStreamInvalidate(h.stream);
                FSEventStreamRelease(h.stream);
            }
        }
    }
}

impl Drop for RecentFiles {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CREATED: u32 = FSEVENT_ITEM_CREATED | FSEVENT_ITEM_IS_FILE;
    const RENAMED: u32 = FSEVENT_ITEM_RENAMED | FSEVENT_ITEM_IS_FILE;
    const RENAMED_DIR: u32 = FSEVENT_ITEM_RENAMED | FSEVENT_ITEM_IS_DIR;
    const REMOVED_DIR: u32 = FSEVENT_ITEM_REMOVED | FSEVENT_ITEM_IS_DIR;

    fn tmp_root() -> String {
        fs::canonicalize(std::env::temp_dir())
            .unwrap_or_else(|_| std::env::temp_dir())
            .to_string_lossy()
            .into_owned()
    }

    fn make_home(tag: &str) -> String {
        let dir = format!("{}/recent-test-{}-{}", tmp_root(), tag, std::process::id());
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_file(p: &str, text: &str) {
        if let Some(dir) = Path::new(p).parent() {
            let _ = fs::create_dir_all(dir);
        }
        fs::write(p, text).unwrap();
    }

    fn mv(a: &str, b: &str) {
        fs::rename(a, b).unwrap();
    }

    fn new_unit(tag: &str) -> (RecentFiles, String) {
        let home = make_home(tag);
        let mut r = RecentFiles::new(&home, &format!("{home}/store/recent.json"));
        r.configure(false, 7, 200, &[], false);
        (r, home)
    }

    fn paths(r: &RecentFiles) -> Vec<String> {
        r.entries(false).into_iter().map(|e| e.path).collect()
    }

    fn check(cond: bool, msg: &str) {
        assert!(cond, "{msg}");
    }

    #[test]
    fn created() {
        let (mut r, home) = new_unit("created");
        let a = format!("{home}/a.txt");
        write_file(&a, "x");
        r.handle(&[a.clone()], &[CREATED], &[Some(1)]);
        check(paths(&r) == [a.clone()], "created file is listed");

        let hidden = format!("{home}/.hidden");
        let dl = format!("{home}/x.crdownload");
        write_file(&hidden, "x");
        write_file(&dl, "x");
        r.handle(&[hidden, dl], &[CREATED, CREATED], &[Some(2), Some(3)]);
        check(paths(&r) == [a], "hidden files / partial downloads are not");
    }

    #[test]
    fn rename_file() {
        let (mut r, home) = new_unit("rename");
        let a = format!("{home}/a.txt");
        let b = format!("{home}/b.txt");
        write_file(&a, "x");
        r.handle(&[a.clone()], &[CREATED], &[Some(1)]);
        mv(&a, &b);
        r.handle(&[a.clone(), b.clone()], &[RENAMED, RENAMED], &[Some(1), Some(1)]);
        check(paths(&r) == [b.clone()], "listed under its new name only");

        let c = format!("{home}/c.txt");
        mv(&b, &c);
        r.handle(&[b], &[RENAMED], &[Some(1)]);
        check(paths(&r).is_empty(), "old name leaves at once");
        r.handle(&[c.clone()], &[RENAMED], &[Some(1)]);
        check(paths(&r) == [c.clone()], "new name arrives with the second callback");

        let d = format!("{home}/d.txt");
        mv(&c, &d);
        r.handle(&[c, d.clone()], &[RENAMED, RENAMED], &[]);
        check(paths(&r) == [d], "works without inodes");
    }

    #[test]
    fn case_only_rename() {
        let (mut r, home) = new_unit("case");
        let a = format!("{home}/note.txt");
        let b = format!("{home}/Note.txt");
        write_file(&a, "x");
        r.handle(&[a.clone()], &[CREATED], &[Some(1)]);
        if !Path::new(&format!("{home}/NOTE.TXT")).exists() {
            eprintln!("case-sensitive volume: skipped");
            return;
        }
        mv(&a, &b);
        r.handle(&[a, b.clone()], &[RENAMED, RENAMED], &[Some(1), Some(1)]);
        check(paths(&r) == [b], "listed once, under the new spelling");
    }

    #[test]
    fn rename_folder() {
        let (mut r, home) = new_unit("folder");
        let d = format!("{home}/proj");
        let e = format!("{home}/project");
        write_file(&format!("{d}/one.txt"), "x");
        write_file(&format!("{d}/sub/two.txt"), "x");
        r.handle(
            &[format!("{d}/one.txt"), format!("{d}/sub/two.txt")],
            &[CREATED, CREATED],
            &[Some(1), Some(2)],
        );
        write_file(&format!("{home}/proj2/three.txt"), "x");
        r.handle(&[format!("{home}/proj2/three.txt")], &[CREATED], &[Some(3)]);
        mv(&d, &e);
        r.handle(&[d.clone(), e.clone()], &[RENAMED_DIR, RENAMED_DIR], &[Some(9), Some(9)]);
        let got: std::collections::HashSet<String> = paths(&r).into_iter().collect();
        check(got.contains(&format!("{e}/one.txt")), "file follows the folder");
        check(got.contains(&format!("{e}/sub/two.txt")), "nested file follows the folder");
        check(got.contains(&format!("{home}/proj2/three.txt")), "sibling untouched");
        check(!got.iter().any(|x| x.starts_with(&format!("{d}/"))), "nothing under the old name");
        check(got.contains(&e), "the renamed folder itself is listed");
    }

    #[test]
    fn rename_out_of_scope() {
        let (mut r, home) = new_unit("scope");
        let d = format!("{home}/keep");
        write_file(&format!("{d}/one.txt"), "x");
        r.handle(&[format!("{d}/one.txt")], &[CREATED], &[Some(1)]);
        mv(&d, &format!("{home}/.trash"));
        r.handle(
            &[d.clone(), format!("{home}/.trash")],
            &[RENAMED_DIR, RENAMED_DIR],
            &[Some(9), Some(9)],
        );
        check(paths(&r).is_empty(), "hidden → gone from the list");

        write_file(&format!("{home}/other/x.txt"), "x");
        mv(&format!("{home}/other"), &format!("{home}/other2"));
        r.handle(
            &[format!("{home}/other"), format!("{home}/other2")],
            &[RENAMED_DIR, RENAMED_DIR],
            &[Some(7), Some(7)],
        );
        check(paths(&r) == [format!("{home}/other2")], "unrelated rename inherits nothing");
    }

    #[test]
    fn removed_folder() {
        let (mut r, home) = new_unit("removed");
        let d = format!("{home}/gone");
        write_file(&format!("{d}/one.txt"), "x");
        write_file(&format!("{home}/stay.txt"), "x");
        r.handle(
            &[format!("{d}/one.txt"), format!("{home}/stay.txt")],
            &[CREATED, CREATED],
            &[Some(1), Some(2)],
        );
        let _ = fs::remove_dir_all(&d);
        r.handle(&[d], &[REMOVED_DIR], &[Some(9)]);
        check(paths(&r) == [format!("{home}/stay.txt")], "its files leave, others stay");
    }

    #[test]
    fn atomic_save() {
        let (mut r, home) = new_unit("atomic");
        let a = format!("{home}/doc.md");
        let t = format!("{home}/doc.md.sb-1234");
        write_file(&a, "x");
        r.handle(&[a.clone()], &[CREATED], &[Some(1)]);
        write_file(&t, "new");
        r.handle(&[t.clone()], &[CREATED], &[Some(2)]);
        // replaceItemAt(a, t): t becomes a
        fs::rename(&t, &a).unwrap();
        r.handle(&[t.clone(), a.clone()], &[RENAMED, RENAMED], &[Some(2), Some(2)]);
        check(paths(&r) == [a.clone()], "the document stays, the temp never lingers");

        mv(&a, &format!("{a}~"));
        write_file(&a, "newer");
        r.handle(
            &[a.clone(), format!("{a}~")],
            &[RENAMED | FSEVENT_ITEM_CREATED, RENAMED],
            &[Some(3), Some(1)],
        );
        check(paths(&r) == [a], "backup-then-rewrite keeps the document");
    }

    #[test]
    fn own_change_keeps_its_place() {
        let (mut r, home) = new_unit("own");
        let a = format!("{home}/a.txt");
        let b = format!("{home}/b.txt");
        write_file(&a, "x");
        r.handle(&[a.clone()], &[CREATED], &[Some(1)]);
        r.configure(true, 7, 200, &[], false);
        let before = paths(&r);
        let at = before.iter().position(|p| p == &a).unwrap();
        mv(&a, &b);
        r.own_change(Some(&a), &b);
        check(paths(&r).contains(&b), "new name is there at once");
        check(!paths(&r).contains(&a), "old name is gone at once");
        check(paths(&r).iter().position(|p| p == &b) == Some(at), "the row keeps its place");
    }

    #[test]
    fn canonical_symlinks_and_tmp() {
        let home = make_home("symlink");
        write_file(&format!("{home}/real/a.txt"), "x");
        std::os::unix::fs::symlink(format!("{home}/real"), format!("{home}/link")).unwrap();
        check(
            RecentFiles::canonical(&format!("{home}/link/a.txt")) == format!("{home}/real/a.txt"),
            "canonical = the real folder",
        );
        check(
            RecentFiles::canonical(&format!("{home}/real/a.txt")) == format!("{home}/real/a.txt"),
            "a real path is itself",
        );
        check(RecentFiles::canonical("/tmp/x") == "/private/tmp/x", "/tmp → /private/tmp");
    }

    #[test]
    fn stored_entries_load_and_canonicalize() {
        let home = make_home("symlink2");
        write_file(&format!("{home}/real/a.txt"), "x");
        std::os::unix::fs::symlink(format!("{home}/real"), format!("{home}/link")).unwrap();
        let store = format!("{home}/store/recent.json");
        let t = now();
        let raw = json!([
            {"path": format!("{home}/link/a.txt"), "at": t},
            {"path": format!("{home}/real/a.txt"), "at": t - 5.0},
        ]);
        fs::create_dir_all(format!("{home}/store")).unwrap();
        fs::write(&store, serde_json::to_vec(&raw).unwrap()).unwrap();
        let mut r = RecentFiles::new(&home, &store);
        r.configure(true, 7, 200, &[], false);
        check(
            paths(&r).contains(&format!("{home}/real/a.txt")),
            "the stored file is listed",
        );
        let a: Vec<String> = paths(&r).into_iter().filter(|p| p.ends_with("/a.txt")).collect();
        check(a == [format!("{home}/real/a.txt")], "once, under its real folder");
    }

    #[test]
    fn in_scope_and_keep_rules() {
        let home = "/Users/tester";
        check(in_scope(home, false, &format!("{home}/x.txt")), "home is in scope");
        check(!in_scope(home, false, &format!("{home}/Library/x")), "home Library is not");
        check(in_scope(home, false, "/private/tmp/x"), "/private/tmp always");
        check(!in_scope(home, false, "/Users/other/x"), "another user is out when not everywhere");
        check(!in_scope(home, true, "/System/x"), "system roots are out");
        check(!keep(home, false, &[], &format!("{home}/node_modules/x")), "noise dir");
        check(!keep(home, false, &[], &format!("{home}/x.crdownload")), "noise ext");
        check(!keep(home, false, &[], &format!("{home}/.hidden")), "dotfile");
        check(keep(home, false, &[], &format!("{home}/x.txt")), "plain file kept");
        check(
            !keep(home, false, &["*.log".into()], &format!("{home}/a.log")),
            "exclude glob",
        );
    }

    #[test]
    fn present_is_exact_case() {
        let home = make_home("present");
        let a = format!("{home}/Note.txt");
        write_file(&a, "x");
        check(RecentFiles::present(&a), "exact name present");
        if Path::new(&format!("{home}/note.txt")).exists() {
            check(!RecentFiles::present(&format!("{home}/note.txt")), "wrong case absent");
        }
        check(!RecentFiles::present(&format!("{home}/missing.txt")), "missing absent");
    }

    #[test]
    fn create_flags_match_fsevents_header() {
        // FileEvents(0x10) | UseCFTypes(0x01) | UseExtendedData(0x40) | IgnoreSelf(0x08)
        assert_eq!(create_flags(), 0x59);
    }

    #[test]
    fn stop_without_start_is_a_noop() {
        let (mut r, _home) = new_unit("stop-noop");
        r.stop();
        check(!r.enabled, "no stream → still stopped");
    }
}
