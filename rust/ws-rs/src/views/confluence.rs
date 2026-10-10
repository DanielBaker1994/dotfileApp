//! Port of `Confluence.swift` — the Confluence search / favorites view.
//!
//! The `WKWebView` preview and the `wsconf://` image scheme handler are real
//! (`macos`), as is the model — `[confluence]` config, the search/results +
//! favorites rows, the Spaces/Contributor pickers, the rate-limit cooldown,
//! `whereText` and the `do:confluence:*` hooks. The remaining AppKit surfaces
//! (the search strip, results table, splitter) are still deferred. The pure
//! decisions mirror `pylib/confluence_glue.py`
//! (`criteria` / `auth_header` / `rate_limit`) so they stay testable without
//! the python worker; the API itself runs `confluence/confluence_api.py`.

use std::cell::Cell;
use std::collections::HashMap;

use serde_json::{json, Value};

use crate::app::registry::{PaletteCommand, RectI, Registry, SlotMember, SlotView};
use crate::ui::chrome::Rect;
use crate::ui::theme::{PopupColors, PopupTone, Rgba};

// ---------------------------------------------------------------------------
// `[confluence]` config (`confluenceSetting` / `confluenceEnabled`)
// ---------------------------------------------------------------------------

/// `confluenceSetting` defaults (`confluenceSetting` never fails).
pub const DEFAULT_WIDTH: f64 = 1400.0;
pub const DEFAULT_HEIGHT: f64 = 900.0;
pub const DEFAULT_SPLIT: f64 = 0.42;
pub const DEFAULT_SIDEBAR_WIDTH: f64 = 210.0;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConfluenceConfig {
    pub entries: HashMap<String, String>,
}

impl ConfluenceConfig {
    pub fn from_entries(entries: HashMap<String, String>) -> Self {
        ConfluenceConfig { entries }
    }

    /// `configSectionValue("confluence", key)` decided from the section's
    /// decoded entries (the caller has already read commands.toml).
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
            .and_then(|v| tri_value(v))
            .unwrap_or(fallback)
    }

    /// `confluenceEnabled()` — `tri(...) == true`, so only an explicit truthy
    /// value enables it.
    pub fn enabled(&self) -> bool {
        self.entries.get("enabled").and_then(|v| tri_value(v)) == Some(true)
    }

    pub fn in_palette(&self) -> bool {
        self.entries
            .get("in-palette")
            .and_then(|v| tri_value(v))
            .unwrap_or(true)
    }

    pub fn label(&self) -> String {
        self.string("label", "Confluence Search")
    }

    pub fn width(&self) -> f64 {
        self.number("width", DEFAULT_WIDTH)
    }

    pub fn height(&self) -> f64 {
        self.number("height", DEFAULT_HEIGHT)
    }

    pub fn split(&self) -> f64 {
        self.number("split", DEFAULT_SPLIT)
    }

    pub fn sidebar_width(&self) -> f64 {
        self.number("sidebar-width", DEFAULT_SIDEBAR_WIDTH)
    }

    /// `split` clamped to the 0.2-0.8 range the splitter enforces.
    pub fn split_clamped(&self) -> f64 {
        clamp_split(self.split())
    }
}

/// The Swift `splitter` clamps the fraction into `0.2...0.8`.
pub fn clamp_split(f: f64) -> f64 {
    if !(0.2..=0.8).contains(&f) {
        DEFAULT_SPLIT
    } else {
        f
    }
}

/// `config.tri` for the common spellings (helper-free so config parsing is
/// deterministic in tests).
pub fn tri_value(v: &str) -> Option<bool> {
    match v.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Search model (`ConfluenceRow`, `Scope`, criteria)
// ---------------------------------------------------------------------------

/// `enum Scope` in `Confluence.swift`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Search,
    Favorites,
}

impl Scope {
    pub fn index(self) -> i64 {
        match self {
            Scope::Search => 0,
            Scope::Favorites => 1,
        }
    }
}

/// `NSRange` over the UTF-16 view the python ranges are expressed in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HitRange {
    pub location: i64,
    pub length: i64,
}

/// `struct ConfluenceRow`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConfluenceRow {
    pub id: String,
    /// `type` in Swift; `kind` avoids the Rust keyword.
    pub kind: String,
    pub title: String,
    pub excerpt: String,
    pub space: String,
    pub space_name: String,
    pub path: String,
    pub container: String,
    pub url: String,
    pub modified: String,
    pub modified_text: String,
    pub author: String,
    pub title_hits: Vec<HitRange>,
    pub hits: Vec<HitRange>,
    pub favorite: bool,
    pub missing: bool,
}

impl ConfluenceRow {
    fn str_of(j: &Value, k: &str) -> String {
        match j.get(k) {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Null) | None => String::new(),
            Some(other) => other.to_string(),
        }
    }

    fn ranges(j: &Value, k: &str) -> Vec<HitRange> {
        j.get(k)
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|r| {
                        let a = r.get(0)?.as_i64()?;
                        let n = r.get(1)?.as_i64()?;
                        Some(HitRange { location: a, length: n })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// `ConfluenceRow(_ j:)`.
    pub fn from_json(j: &Value) -> Self {
        ConfluenceRow {
            id: Self::str_of(j, "id"),
            kind: {
                let t = Self::str_of(j, "type");
                if t.is_empty() {
                    "page".to_string()
                } else {
                    t
                }
            },
            title: Self::str_of(j, "title"),
            excerpt: Self::str_of(j, "excerpt"),
            space: Self::str_of(j, "space"),
            space_name: Self::str_of(j, "spaceName"),
            path: Self::str_of(j, "path"),
            container: Self::str_of(j, "container"),
            url: Self::str_of(j, "url"),
            modified: Self::str_of(j, "modified"),
            modified_text: Self::str_of(j, "modifiedText"),
            author: Self::str_of(j, "author"),
            title_hits: Self::ranges(j, "titleHits"),
            hits: Self::ranges(j, "hits"),
            favorite: j.get("favorite").and_then(Value::as_bool).unwrap_or(false),
            missing: j.get("missing").and_then(Value::as_bool).unwrap_or(false),
        }
    }

    /// `var json` — the object round-tripped through `--favorite add`.
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "title": self.title,
            "space": self.space,
            "spaceName": self.space_name,
            "type": self.kind,
            "url": self.url,
            "path": self.path,
        })
    }

    /// `var typeLabel`.
    pub fn type_label(&self) -> &'static str {
        match self.kind.as_str() {
            "blogpost" => "Blog",
            "attachment" => "Attachment",
            "comment" => "Comment",
            _ => "",
        }
    }

    /// The meta line `ConfResultCell` draws (`spaceName · path/on container ·
    /// author · modifiedText`).
    pub fn meta(&self) -> String {
        let mut meta: Vec<String> = Vec::new();
        let head = if self.space_name.is_empty() {
            self.space.clone()
        } else {
            self.space_name.clone()
        };
        meta.push(head);
        if !self.path.is_empty() {
            meta.push(self.path.clone());
        } else if !self.container.is_empty() {
            meta.push(format!("on {}", self.container));
        }
        if !self.author.is_empty() {
            meta.push(self.author.clone());
        }
        if !self.modified_text.is_empty() {
            meta.push(self.modified_text.clone());
        }
        meta.into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("  ·  ")
    }
}

/// `static let modes` from `confluence/defaults.json`.
pub const MODES: [&str; 3] = ["all", "phrase", "any"];
/// `defaults.search.types`.
pub const DEFAULT_TYPES: [&str; 2] = ["page", "blogpost"];
pub const RATE_LIMIT_MIN_SECONDS: i64 = 3;
pub const RATE_LIMIT_FALLBACK_SECONDS: i64 = 30;

/// The search panel's live state (`criteria()` params in the glue call).
#[derive(Clone, Debug, PartialEq)]
pub struct CriteriaParams {
    pub query: String,
    pub mode_index: usize,
    pub title_only: bool,
    pub spaces_all: bool,
    pub spaces: Vec<String>,
    pub types: Option<String>,
    pub modified: String,
    pub contributors_all: bool,
    pub contributors: Vec<String>,
    pub sort: Option<String>,
    pub favorites: bool,
}

impl Default for CriteriaParams {
    fn default() -> Self {
        CriteriaParams {
            query: String::new(),
            mode_index: 0,
            title_only: false,
            spaces_all: true,
            spaces: Vec::new(),
            types: None,
            modified: String::new(),
            contributors_all: true,
            contributors: Vec::new(),
            sort: None,
            favorites: false,
        }
    }
}

/// `confluence_glue.criteria` — the panel state to the criteria JSON
/// `confluence_api.py --search` consumes.
pub fn criteria(p: &CriteriaParams) -> Value {
    let idx = if p.mode_index < MODES.len() { p.mode_index } else { 0 };
    let types_raw = p
        .types
        .clone()
        .unwrap_or_else(|| DEFAULT_TYPES.join(","));
    let types: Vec<String> = types_raw
        .split(',')
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect();
    // `mode.all` is the query; a quoted part stays a phrase server-side.
    let spaces: Vec<String> = if p.spaces_all {
        Vec::new()
    } else {
        p.spaces.iter().filter(|s| !s.is_empty()).cloned().collect()
    };
    let contributors: Vec<String> = if p.contributors_all {
        Vec::new()
    } else {
        p.contributors
            .iter()
            .filter(|c| !c.is_empty())
            .cloned()
            .collect()
    };
    let mut c = json!({
        "query": p.query.trim(),
        "mode": MODES[idx],
        "titleOnly": p.title_only,
        "spaces": spaces,
        "types": types,
        "modified": p.modified,
        "contributors": contributors,
        "sort": p.sort.clone().unwrap_or_else(|| "relevance".to_string()),
    });
    if p.favorites {
        c["favorites"] = json!(true);
    }
    c
}

/// `confluence_glue.auth_header` — the Authorization header for the image
/// loader's fetches. Empty when no token.
pub fn auth_header(config: &Value) -> String {
    let as_str = |k: &str| config.get(k).and_then(Value::as_str).unwrap_or("");
    let token = as_str("token");
    let email = as_str("email");
    let mut auth = as_str("auth").to_ascii_lowercase();
    if auth != "bearer" && auth != "basic" {
        auth = if email.is_empty() { "bearer" } else { "basic" }.to_string();
    }
    if token.is_empty() {
        return String::new();
    }
    if auth == "basic" {
        format!("Basic {}", base64_encode(format!("{email}:{token}").as_bytes(), false))
    } else {
        format!("Bearer {token}")
    }
}

/// `confluence_glue.rate_limit` — the cooldown decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RateLimitDecision {
    pub limited: bool,
    pub seconds: i64,
}

pub fn rate_limit(response: &Value) -> RateLimitDecision {
    if response.get("rateLimited").and_then(Value::as_bool) != Some(true) {
        return RateLimitDecision { limited: false, seconds: 0 };
    }
    let secs = response.get("retryIn").and_then(Value::as_i64).filter(|_| {
        // reject bools (as_i64 never returns for bools, but keep the guard
        // explicit for parity with the python `isinstance(int) and not bool`).
        !response.get("retryIn").map(Value::is_boolean).unwrap_or(false)
    });
    let secs = secs.unwrap_or(RATE_LIMIT_FALLBACK_SECONDS);
    RateLimitDecision {
        limited: true,
        seconds: secs.max(RATE_LIMIT_MIN_SECONDS),
    }
}

// ---------------------------------------------------------------------------
// Rate-limit cooldown (`cooldownUntil` / `tickCooldown` / ratelimit.json)
// ---------------------------------------------------------------------------

/// The python writer stores `{site, until, seconds, why, at}` in
/// `~/.cache/confluence/ratelimit.json`; the app-side cooldown is in-memory
/// and this reads the shared file when a fresh process starts.
pub fn cooldown_file_path() -> String {
    if let Ok(dir) = std::env::var("CONFLUENCE_CACHE_DIR") {
        if !dir.is_empty() {
            return format!("{dir}/ratelimit.json");
        }
    }
    let home = std::env::var("HOME").unwrap_or_default();
    format!("{home}/.cache/confluence/ratelimit.json")
}

/// `cooldown_left(site)`: the seconds left for `site` from the shared file.
pub fn cooldown_left_in(path: &str, site: &str, now: f64) -> i64 {
    let Ok(data) = std::fs::read(path) else {
        return 0;
    };
    let Ok(j) = serde_json::from_slice::<Value>(&data) else {
        return 0;
    };
    if j.get("site").and_then(Value::as_str) != Some(site) {
        return 0;
    }
    let until = j.get("until").and_then(Value::as_f64).unwrap_or(0.0);
    ((until - now).max(0.0)).round() as i64
}

pub fn cooldown_left(site: &str, now: f64) -> i64 {
    cooldown_left_in(&cooldown_file_path(), site, now)
}

/// `cooldownUntil` + `coolingDown`, driven by an injected clock.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cooldown {
    pub until: Option<f64>,
}

impl Cooldown {
    pub fn active(&self, now: f64) -> bool {
        self.until.map(|u| u > now).unwrap_or(false)
    }

    pub fn remaining(&self, now: f64) -> i64 {
        match self.until {
            Some(u) => (u - now).ceil().max(0.0) as i64,
            None => 0,
        }
    }

    pub fn activate(&mut self, seconds: i64, now: f64) {
        self.until = Some(now + seconds.max(0) as f64);
    }

    pub fn clear(&mut self) {
        self.until = None;
    }

    /// `tickCooldown()`'s decision: `true` when the cooldown just lifted.
    pub fn tick(&mut self, now: f64) -> bool {
        match self.until {
            Some(u) if u <= now => {
                self.until = None;
                true
            }
            _ => false,
        }
    }
}

/// `tickCooldown()`'s outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CooldownTick {
    Idle,
    Counting(i64),
    Lifted,
}

// ---------------------------------------------------------------------------
// Pickers (Spaces / Contributor) — `JiraMultiPicker`'s model
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct PickerOption {
    pub id: String,
    pub title: String,
    pub detail: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PickerModel {
    pub options: Vec<PickerOption>,
    pub selected: Vec<String>,
    pub all: bool,
    pub placeholder: String,
    pub all_title: String,
}

impl PickerModel {
    pub fn new(placeholder: &str, all_title: &str) -> Self {
        PickerModel {
            options: Vec::new(),
            selected: Vec::new(),
            all: true,
            placeholder: placeholder.to_string(),
            all_title: all_title.to_string(),
        }
    }

    /// `JiraMultiPicker.set(_:all:)`.
    pub fn set(&mut self, selected: Vec<String>, all: bool) {
        self.selected = selected;
        self.all = all;
    }

    pub fn is_all(&self) -> bool {
        self.all
    }

    /// `spaces.options = sp.compactMap { key -> Option }`.
    pub fn set_options(&mut self, options: Vec<PickerOption>) {
        self.options = options;
    }
}

// ---------------------------------------------------------------------------
// The window model
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusTone {
    Dim,
    Success,
    Warning,
    Danger,
}

/// `setRows` / `runSearch` result so the caller can decide what to do next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchOutcome {
    Ok,
    RateLimited,
    Failed,
}

/// `ConfluenceWindow`'s model without AppKit.
#[derive(Debug)]
pub struct ConfluenceModel {
    pub config: ConfluenceConfig,
    pub scope: Scope,
    pub rows: Vec<ConfluenceRow>,
    pub favorites: Vec<ConfluenceRow>,
    pub showing_favorites: bool,
    pub query: String,
    pub site: String,
    pub configured: bool,
    pub searching: bool,
    pub last_cql: String,
    pub last_curl: String,
    pub next_link: String,
    pub total: i64,
    pub terms: Vec<Value>,
    pub status: String,
    pub status_tone: StatusTone,
    pub cooldown: Cooldown,
    pub spaces: PickerModel,
    pub contributors: PickerModel,
    pub last_fav_refresh: Option<f64>,
    pub mode_index: usize,
    pub title_only: bool,
    pub type_choice: String,
    pub modified: String,
    pub sort: String,
    pub selection: usize,
    shown: Cell<bool>,
    key: Cell<bool>,
    frame: Cell<Option<RectI>>,
    level: Cell<i64>,
}

impl Default for ConfluenceModel {
    fn default() -> Self {
        Self::new(ConfluenceConfig::default())
    }
}

impl ConfluenceModel {
    pub fn new(config: ConfluenceConfig) -> Self {
        ConfluenceModel {
            config,
            scope: Scope::Search,
            rows: Vec::new(),
            favorites: Vec::new(),
            showing_favorites: false,
            query: String::new(),
            site: String::new(),
            configured: false,
            searching: false,
            last_cql: String::new(),
            last_curl: String::new(),
            next_link: String::new(),
            total: 0,
            terms: Vec::new(),
            status: String::new(),
            status_tone: StatusTone::Dim,
            cooldown: Cooldown::default(),
            spaces: PickerModel::new("All spaces", "All spaces"),
            contributors: PickerModel::new("Any contributor", "Any contributor"),
            last_fav_refresh: None,
            mode_index: 0,
            title_only: false,
            type_choice: "page,blogpost".to_string(),
            modified: String::new(),
            sort: "relevance".to_string(),
            selection: 0,
            shown: Cell::new(false),
            key: Cell::new(false),
            frame: Cell::new(None),
            level: Cell::new(0),
        }
    }

    /// `var whereText`.
    pub fn where_text(&self) -> String {
        let q = self.query.trim();
        if self.showing_favorites {
            "Favorites".to_string()
        } else if q.is_empty() {
            "Search".to_string()
        } else {
            format!("\u{201c}{q}\u{201d}")
        }
    }

    /// `var hasCriteria`.
    pub fn has_criteria(&self) -> bool {
        !self.query.trim().is_empty()
            || !self.modified.is_empty()
            || (!self.contributors.is_all() && !self.contributors.selected.is_empty())
            || (!self.spaces.is_all() && !self.spaces.selected.is_empty())
    }

    /// `criteria()` + `saveCriteria()` minus the `favorites` key.
    pub fn criteria_params(&self) -> CriteriaParams {
        CriteriaParams {
            query: self.query.clone(),
            mode_index: self.mode_index,
            title_only: self.title_only,
            spaces_all: self.spaces.is_all(),
            spaces: self.spaces.selected.clone(),
            types: Some(self.type_choice.clone()),
            modified: self.modified.clone(),
            contributors_all: self.contributors.is_all(),
            contributors: self.contributors.selected.clone(),
            sort: Some(self.sort.clone()),
            favorites: self.scope == Scope::Favorites,
        }
    }

    pub fn criteria(&self) -> Value {
        criteria(&self.criteria_params())
    }

    /// `saveCriteria()` — the persisted JSON drops `favorites`.
    pub fn saved_criteria(&self) -> Value {
        let mut c = self.criteria();
        if let Some(obj) = c.as_object_mut() {
            obj.remove("favorites");
        }
        c
    }

    pub fn set_status(&mut self, s: impl Into<String>, tone: StatusTone) {
        self.status = s.into();
        self.status_tone = tone;
    }

    /// `coolingDown`.
    pub fn cooling_down(&self, now: f64) -> bool {
        self.cooldown.active(now)
    }

    /// `rateLimited(_:retry:)` — start the cooldown when the response says so.
    pub fn handle_rate_limited(&mut self, j: &Value, now: f64) -> bool {
        let d = rate_limit(j);
        if !d.limited {
            return false;
        }
        self.cooldown.activate(d.seconds, now);
        self.tick_cooldown(now);
        true
    }

    /// `tickCooldown()` — updates the status and reports what the timer does.
    pub fn tick_cooldown(&mut self, now: f64) -> CooldownTick {
        if self.cooldown.until.is_none() {
            return CooldownTick::Idle;
        }
        let left = self.cooldown.remaining(now);
        if left > 0 {
            self.set_status(
                format!("Confluence rate limit \u{2014} requests paused, resuming in {left}s"),
                StatusTone::Warning,
            );
            return CooldownTick::Counting(left);
        }
        self.cooldown.clear();
        self.set_status("Resuming\u{2026}", StatusTone::Dim);
        CooldownTick::Lifted
    }

    /// `showSearchEmpty()`.
    pub fn show_search_empty(&mut self) {
        self.showing_favorites = false;
        self.last_cql.clear();
        self.next_link.clear();
        self.terms.clear();
        if self.configured {
            self.rows.clear();
            self.status = if self.cooldown.active(now_secs()) {
                self.status.clone()
            } else {
                "Ready".to_string()
            };
            self.status_tone = StatusTone::Dim;
        } else {
            self.rows.clear();
            self.status = "Connect a Confluence site to start searching.".to_string();
            self.status_tone = StatusTone::Dim;
        }
    }

    /// `setScope(_:)`.
    pub fn set_scope(&mut self, s: Scope, now: f64) {
        self.scope = s;
        if s == Scope::Favorites {
            self.show_favorites(false, now);
        } else {
            self.showing_favorites = false;
            if self.has_criteria() {
                self.set_status("Ready", StatusTone::Dim);
            } else {
                self.show_search_empty();
            }
        }
    }

    /// `showFavorites(keepSelection:)` — filter the favorites by the query.
    pub fn show_favorites(&mut self, keep_selection: bool, now: f64) {
        let words: Vec<String> = self
            .query
            .to_lowercase()
            .split(' ')
            .filter(|w| !w.is_empty())
            .map(str::to_string)
            .collect();
        let previous = if keep_selection {
            self.rows.get(self.selection).map(|r| r.id.clone())
        } else {
            None
        };
        let list: Vec<ConfluenceRow> = self
            .favorites
            .iter()
            .filter(|f| {
                let hay = format!("{} {} {} {}", f.title, f.space, f.space_name, f.path)
                    .to_lowercase();
                words.iter().all(|w| hay.contains(w.as_str()))
            })
            .cloned()
            .collect();
        self.showing_favorites = true;
        self.last_cql.clear();
        self.next_link.clear();
        self.terms = words
            .iter()
            .map(|w| json!({"text": w, "phrase": false, "prefix": true}))
            .collect();
        self.rows = list;
        if let Some(id) = previous {
            if let Some(i) = self.rows.iter().position(|r| r.id == id) {
                self.selection = i;
            }
        } else {
            self.selection = 0;
        }
        if !self.cooling_down(now) {
            let count = self.rows.len();
            let suffix = if words.is_empty() {
                String::new()
            } else {
                format!(" of {}", self.favorites.len())
            };
            let plural = if self.favorites.len() == 1 { "" } else { "s" };
            self.set_status(
                format!("\u{2605} {count}{suffix} favorite{plural} \u{2014} Return opens, type to filter"),
                StatusTone::Dim,
            );
        }
    }

    /// The empty-list hint `showFavorites` shows.
    pub fn favorites_empty_hint(&self) -> String {
        if !self.favorites.is_empty() {
            format!(
                "No favorites match \u{201c}{}\u{201d}.\nThe Search button looks inside their text.",
                self.query
            )
        } else {
            "No favorites yet.\nStar a search result (\u{2606} or Cmd+D) to pin it here for one-click opening."
                .to_string()
        }
    }

    /// `runSearch(_:)`'s response handling (the paging + rows model).
    pub fn apply_search_result(&mut self, j: &Value, more: bool, now: f64) -> SearchOutcome {
        self.searching = false;
        if let Some(cql) = j.get("cql").and_then(Value::as_str) {
            self.last_cql = cql.to_string();
        }
        if let Some(curl) = j.get("curl").and_then(Value::as_str) {
            self.last_curl = curl.to_string();
        }
        if j.get("ok").and_then(Value::as_bool) != Some(true) {
            if self.handle_rate_limited(j, now) {
                return SearchOutcome::RateLimited;
            }
            if j.get("setup").and_then(Value::as_bool) == Some(true) {
                self.configured = false;
            }
            let err = j
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("search failed")
                .to_string();
            self.set_status(err, StatusTone::Danger);
            if !more {
                self.rows.clear();
            }
            return SearchOutcome::Failed;
        }
        let new: Vec<ConfluenceRow> = j
            .get("results")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(ConfluenceRow::from_json).collect())
            .unwrap_or_default();
        self.showing_favorites = false;
        self.terms = j.get("terms").and_then(Value::as_array).cloned().unwrap_or_default();
        self.total = j
            .get("total")
            .and_then(Value::as_i64)
            .unwrap_or(new.len() as i64);
        self.next_link = j.get("next").and_then(Value::as_str).unwrap_or("").to_string();
        if more {
            self.rows.extend(new);
        } else {
            self.rows = new;
            self.selection = 0;
        }
        if self.rows.is_empty() {
            let within = if self.scope == Scope::Favorites { " in favorites" } else { "" };
            self.set_status(
                format!("No matches{within} \u{2014} try Any word, fewer filters, or prefix*"),
                StatusTone::Dim,
            );
        } else {
            let within = if self.scope == Scope::Favorites { " in favorites" } else { "" };
            let el = j
                .get("elapsed")
                .and_then(Value::as_f64)
                .map(|e| format!(" \u{00b7} {e:.1}s"))
                .unwrap_or_default();
            let fb = if j.get("fallback").and_then(Value::as_bool) == Some(true) {
                " \u{00b7} no excerpts (older server)"
            } else {
                ""
            };
            self.set_status(
                format!("{} of {}{within}{el}{fb}", self.rows.len(), self.total),
                StatusTone::Dim,
            );
        }
        SearchOutcome::Ok
    }

    /// `loadFavorites(show:)`'s pure half: adopt the favorites list.
    pub fn apply_favorites(&mut self, j: &Value, show: bool, now: f64) {
        self.favorites = j
            .get("results")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(ConfluenceRow::from_json).collect())
            .unwrap_or_default();
        if show {
            self.show_favorites(false, now);
        }
    }

    /// `toggleFavorite(row:)`'s pure half: flip the row, return the API add
    /// flag (`Some(add)`), or `None` when the index is bad.
    pub fn toggle_favorite(&mut self, i: usize) -> Option<bool> {
        let row = self.rows.get(i)?.clone();
        let add = !row.favorite;
        if let Some(k) = self.rows.iter().position(|r| r.id == row.id) {
            self.rows[k].favorite = add;
        }
        if self.showing_favorites && !add {
            let _ = self.show_favorites(true, now_secs());
        }
        Some(add)
    }

    /// `updatePreviewStar()`: whether the preview's page is favorited.
    pub fn preview_star(&self, preview_id: &str) -> bool {
        self.rows
            .iter()
            .find(|r| r.id == preview_id)
            .map(|r| r.favorite)
            .unwrap_or(false)
    }

    /// `escape()` — clear the query first, else hide (returns `true` when the
    /// caller should ask the host to `escapeAtTop`).
    pub fn escape(&mut self) -> bool {
        if !self.query.trim().is_empty() {
            self.query.clear();
            if self.scope == Scope::Favorites {
                self.show_favorites(false, now_secs());
            } else if !self.has_criteria() {
                self.show_search_empty();
            }
            false
        } else {
            true
        }
    }

    /// The `testQuery`-style `do:confluence:*` hooks.
    pub fn handle_do(&mut self, action: &str) -> Option<Value> {
        let rest = action.strip_prefix("confluence:")?;
        let now = now_secs();
        match rest {
            "state" => Some(self.test_state()),
            "search" => {
                self.set_scope(Scope::Search, now);
                Some(self.test_state())
            }
            "favorites" => {
                self.set_scope(Scope::Favorites, now);
                Some(self.test_state())
            }
            "clear" => {
                self.query.clear();
                self.show_search_empty();
                Some(self.test_state())
            }
            "escape" => {
                self.escape();
                Some(self.test_state())
            }
            _ => {
                if let Some(v) = rest.strip_prefix("query:") {
                    self.query = v.to_string();
                    if self.scope == Scope::Favorites {
                        self.show_favorites(false, now);
                    }
                    return Some(self.test_state());
                }
                if let Some(v) = rest.strip_prefix("select:") {
                    if let Ok(i) = v.parse::<usize>() {
                        if i < self.rows.len() {
                            self.selection = i;
                        }
                        return Some(self.test_state());
                    }
                    return None;
                }
                if let Some(v) = rest.strip_prefix("star:") {
                    if let Ok(i) = v.parse::<usize>() {
                        self.toggle_favorite(i);
                        return Some(self.test_state());
                    }
                    return None;
                }
                if let Some(v) = rest.strip_prefix("tick:") {
                    if let Ok(secs) = v.parse::<i64>() {
                        if let Some(u) = self.cooldown.until.as_mut() {
                            *u += secs as f64;
                        }
                        return Some(self.test_state());
                    }
                    return None;
                }
                None
            }
        }
    }

    pub fn test_state(&self) -> Value {
        let now = now_secs();
        json!({
            "shown": self.shown.get(),
            "key": self.key.get(),
            "level": self.level.get(),
            "frame": self.frame.get(),
            "scope": if self.scope == Scope::Favorites { "favorites" } else { "search" },
            "query": self.query,
            "site": self.site,
            "configured": self.configured,
            "searching": self.searching,
            "showingFavorites": self.showing_favorites,
            "where": self.where_text(),
            "total": self.total,
            "next": self.next_link,
            "canLoadMore": !self.next_link.is_empty(),
            "hasCriteria": self.has_criteria(),
            "coolingDown": self.cooling_down(now),
            "cooldownRemaining": self.cooldown.remaining(now),
            "selection": self.selection,
            "spaces": { "all": self.spaces.is_all(), "selected": self.spaces.selected,
                        "count": self.spaces.options.len() },
            "people": { "all": self.contributors.is_all(), "selected": self.contributors.selected,
                        "count": self.contributors.options.len() },
            "favorites": self.favorites.len(),
            "rows": self.rows.iter().map(|r| json!({
                "id": r.id, "title": r.title, "type": r.kind, "space": r.space,
                "url": r.url, "favorite": r.favorite, "missing": r.missing,
                "titleHits": r.title_hits.iter().map(|h| json!([h.location, h.length])).collect::<Vec<_>>(),
                "hits": r.hits.iter().map(|h| json!([h.location, h.length])).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "status": self.status,
            "statusTone": match self.status_tone {
                StatusTone::Dim => "dim",
                StatusTone::Success => "success",
                StatusTone::Warning => "warning",
                StatusTone::Danger => "danger",
            },
        })
    }
}

impl SlotMember for ConfluenceModel {
    fn view(&self) -> SlotView {
        SlotView::Confluence
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
        ConfluenceModel::test_state(self)
    }
}

/// Register the Confluence palette entry (only while enabled, mirroring
/// `paletteCommands()`).
pub fn register(reg: &mut Registry, enabled: bool, listed: bool) {
    if enabled && listed {
        reg.add_palette(PaletteCommand::new("confluence", "Confluence Search", "views"));
    }
}

// ---------------------------------------------------------------------------
// Preview (python + the WKWebView / wsconf:// scheme handler are real; the
// window's search strip / table / splitter surfaces are deferred)
// ---------------------------------------------------------------------------

fn helper_call(method: &str, params: Value, timeout_secs: u64) -> Option<Value> {
    crate::app::python_helper::PythonHelper::shared()
        .call(
            method,
            params,
            std::time::Duration::from_secs(timeout_secs),
            std::time::Duration::from_secs(5),
        )
        .ok()
}

/// `confluence.preview_html` — the page template + URL rewriting + highlight
/// script all live in `pylib/confluence_pages.py`.
pub fn preview_html(params: &Value) -> Option<String> {
    helper_call("confluence.preview_html", params.clone(), 60)?
        .get("html")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// `ConfluenceImageLoader.wrap` — the `wsconf://fetch/<b64url>` URL.
pub fn wsconf_wrap(real: &str) -> String {
    format!("wsconf://fetch/{}", base64_encode(real.as_bytes(), true))
}

/// `ConfluenceImageLoader.unwrap`.
pub fn wsconf_unwrap(url: &str) -> Option<String> {
    let b = url.rsplit('/').next()?;
    let bytes = base64_decode(b)?;
    String::from_utf8(bytes).ok()
}

/// `loadAuth(_:)` — the derivation lives in `pylib/confluence_glue.py`; this
/// mirrors it directly so the loader works without a worker round trip.
pub fn load_auth(config: &Value) -> Option<String> {
    let header = auth_header(config);
    if header.is_empty() {
        None
    } else {
        Some(header)
    }
}

/// The user script that turns an `<a href>` activation into a `ws` message (so
/// the preview opens links in the browser instead of navigating WebKit). The
/// message body is a JSON string, which keeps the `didReceive` side free of a
/// Foundation dictionary parse.
pub fn link_user_script() -> &'static str {
    r#"(function(){
  function send(m){ try { window.webkit.messageHandlers.ws.postMessage(m); } catch (e) {} }
  document.addEventListener('click', function(e){
    var a = e.target && e.target.closest ? e.target.closest('a[href]') : null;
    if (a) { e.preventDefault(); send(JSON.stringify({ href: a.href })); }
  }, true);
})();"#
}

/// A parsed `ws` script message (the highlighter's `{i, n, snippet}` payload).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HitPayload {
    pub index: i64,
    pub count: i64,
    pub snippet: String,
}

/// `ConfluenceImageLoader` — owns the `wsconf://` image scheme handler and
/// tracks the URLs WebKit asked for.
///
/// In `Confluence.swift` this object *is* the `WKURLSchemeHandler`; here the
/// Objective-C class lives in [`macos::ImageSchemeHandler`] (it needs a
/// `MainThreadMarker`) and this struct owns it. `start` / `stop` are the
/// bookkeeping half of the `webView(_:start:)` / `webView(_:stop:)` callbacks.
pub struct ConfluenceImageLoader {
    pub auth_header: Option<String>,
    /// The `wsconf://` URLs currently being loaded.
    pub inflight: Vec<String>,
    #[cfg(target_os = "macos")]
    handler: Option<objc2::rc::Retained<macos::ImageSchemeHandler>>,
}

impl ConfluenceImageLoader {
    pub fn new() -> Self {
        ConfluenceImageLoader {
            auth_header: None,
            inflight: Vec::new(),
            #[cfg(target_os = "macos")]
            handler: None,
        }
    }

    /// `loadAuth(_:)`'s result: adopt the Authorization header (and push it to
    /// a live handler).
    pub fn set_auth(&mut self, header: Option<String>) {
        #[cfg(target_os = "macos")]
        if let Some(h) = &self.handler {
            h.set_auth(header.clone());
        }
        self.auth_header = header;
    }

    /// The live `WKURLSchemeHandler` to install on a `WKWebViewConfiguration`
    /// (created on first use, with the current auth header).
    #[cfg(target_os = "macos")]
    pub fn handler(
        &mut self,
        mtm: objc2::MainThreadMarker,
    ) -> objc2::rc::Retained<macos::ImageSchemeHandler> {
        if self.handler.is_none() {
            let h = macos::ImageSchemeHandler::new(mtm);
            h.set_auth(self.auth_header.clone());
            self.handler = Some(h);
        }
        self.handler.clone().expect("handler")
    }

    /// `webView(_:start:)` — record the requested `wsconf://` URL.
    pub fn start(&mut self, url: &str) {
        if !self.inflight.iter().any(|u| u == url) {
            self.inflight.push(url.to_string());
        }
    }

    /// `webView(_:stop:)` — drop the URL from the in-flight set.
    pub fn stop(&mut self, url: &str) {
        self.inflight.retain(|u| u != url);
    }
}

impl Default for ConfluenceImageLoader {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// The embeddable content surface (`ConfluenceWindow.build` / `layoutAll`)
// ---------------------------------------------------------------------------

/// `JiraTheme.height`-derived strip height: `10 + height*2 + 8 + 10`.
pub const CONF_STRIP_ROW_HEIGHT: f64 = 24.0;
/// The footer height `layoutAll` calls `footH`.
pub const CONF_FOOTER_HEIGHT: f64 = 32.0;
/// `ConfResultCell.height` — one result row.
pub const CONF_RESULT_ROW_HEIGHT: f64 = 44.0;
/// The draggable splitter's width.
pub const CONF_SPLITTER_WIDTH: f64 = 6.0;
/// The sidebar's section width when `[confluence] sidebar-width` is unset.
pub const CONF_SIDEBAR_DEFAULT: f64 = DEFAULT_SIDEBAR_WIDTH;

/// The regions of the Confluence content surface, mirroring `layoutAll`. All
/// rects are root-relative except the preview-relative `p_*` / `hit_*` / `web`
/// / `hint` / `setup` fields.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ConfluenceLayout {
    pub sidebar: Rect,
    pub strip: Rect,
    pub table: Rect,
    pub rows_height: f64,
    pub empty_hint: Rect,
    pub spinner: Rect,
    pub status: Rect,
    pub more: Rect,
    pub cql: Rect,
    pub curl: Rect,
    pub splitter: Rect,
    pub preview: Rect,
    // preview-relative:
    pub p_title: Rect,
    pub p_meta: Rect,
    pub p_star: Rect,
    pub p_open: Rect,
    pub p_copy: Rect,
    pub hits_bar: Rect,
    pub hit_prev: Rect,
    pub hit_next: Rect,
    pub hit_label: Rect,
    pub web: Rect,
    pub hint: Rect,
    pub setup: Rect,
}

/// `ConfluenceWindow.layoutAll(_:)` — search strip across the top, results
/// list on the left, splitter, preview on the right. The sidebar occupies the
/// left `sidebar_width`; the content area starts after it.
pub fn content_layout(
    width: f64,
    height: f64,
    split: f64,
    sidebar_width: f64,
    row_count: usize,
) -> ConfluenceLayout {
    let width = width.max(0.0);
    let height = height.max(0.0);
    let sidebar_w = sidebar_width.max(0.0).min(width);
    let sidebar = Rect::new(0.0, 0.0, sidebar_w, height);
    // `b = NSRect(x: 0, y: 0, width: max(300, full.width - left), …)`, then the
    // `defer` shifts every column by `left`; fold that into `body.x`.
    let body = Rect::new(sidebar_w, 0.0, (width - sidebar_w).max(300.0), height);

    let strip_h = 10.0 + CONF_STRIP_ROW_HEIGHT * 2.0 + 8.0 + 10.0;
    let strip = Rect::new(body.x, 0.0, body.width, strip_h);
    let body_y = strip_h;
    let body_h = (body.height - strip_h).max(0.0);

    let split = clamp_split(split);
    let left_w = (body.width * split).round();
    let table_h = (body_h - CONF_FOOTER_HEIGHT - 4.0).max(0.0);
    let table = Rect::new(body.x, body_y + 4.0, left_w, table_h);
    let rows_height = (row_count as f64 * CONF_RESULT_ROW_HEIGHT).max(table_h);
    let empty_hint = Rect::new(
        body.x + 8.0,
        body_y + 12.0,
        (left_w - 16.0).max(0.0),
        40.0,
    );

    let fy = body.height - CONF_FOOTER_HEIGHT + 5.0;
    let spinner = Rect::new(body.x + 12.0, fy + 3.0, 16.0, 16.0);

    // The footer buttons lay out right-to-left: curl (rightmost), cql, more.
    let mut x = body.x + left_w - 10.0;
    x -= 56.0;
    let curl = Rect::new(x, fy, 56.0, 22.0);
    x -= 6.0;
    x -= 56.0;
    let cql = Rect::new(x, fy, 56.0, 22.0);
    x -= 6.0;
    x -= 64.0;
    let more = Rect::new(x, fy, 64.0, 22.0);
    let status = Rect::new(
        body.x + 32.0,
        fy + 3.0,
        (x - (body.x + 36.0)).max(40.0),
        16.0,
    );

    let splitter = Rect::new(body.x + left_w, body_y, CONF_SPLITTER_WIDTH, body_h);
    let preview = Rect::new(
        body.x + left_w + CONF_SPLITTER_WIDTH,
        body_y,
        (body.width - left_w - CONF_SPLITTER_WIDTH).max(0.0),
        body_h,
    );

    let pw = preview.width.max(1.0);
    let ph = preview.height.max(1.0);
    // Preview header: copy, open, star from the right edge inward.
    let mut bx = pw - 12.0;
    bx -= 84.0;
    let p_copy = Rect::new(bx, 12.0, 84.0, 22.0);
    bx -= 6.0;
    bx -= 64.0;
    let p_open = Rect::new(bx, 12.0, 64.0, 22.0);
    bx -= 6.0;
    bx -= 22.0;
    let p_star = Rect::new(bx, 12.0, 22.0, 22.0);
    bx -= 6.0;

    let p_title = Rect::new(16.0, 10.0, (bx - 22.0).max(40.0), 20.0);
    let p_meta = Rect::new(16.0, 32.0, (pw - 32.0).max(0.0), 16.0);
    let hits_bar = Rect::new(0.0, 56.0, pw, 28.0);
    let hit_prev = Rect::new(10.0, 3.0, 26.0, 22.0);
    let hit_next = Rect::new(38.0, 3.0, 26.0, 22.0);
    let hit_label = Rect::new(72.0, 6.0, (pw - 84.0).max(0.0), 16.0);
    let web = Rect::new(0.0, 84.0, pw, (ph - 84.0).max(0.0));
    let hint = Rect::new(40.0, ph / 2.0 - 60.0, (pw - 80.0).max(0.0), 60.0);
    let setup = Rect::new((pw - 180.0) / 2.0, ph / 2.0 + 8.0, 180.0, 30.0);

    ConfluenceLayout {
        sidebar,
        strip,
        table,
        rows_height,
        empty_hint,
        spinner,
        status,
        more,
        cql,
        curl,
        splitter,
        preview,
        p_title,
        p_meta,
        p_star,
        p_open,
        p_copy,
        hits_bar,
        hit_prev,
        hit_next,
        hit_label,
        web,
        hint,
        setup,
    }
}

/// `ConfluenceWindow.pageColors(_:)` — the preview template's `rgba()` color
/// strings, keyed by the keys `pylib/confluence_pages.py` substitutes.
pub fn page_colors(colors: &PopupColors) -> HashMap<&'static str, String> {
    fn rgba(c: Rgba, a: f64) -> String {
        let alpha = (c.a * a).clamp(0.0, 1.0);
        format!(
            "rgba({},{},{},{:.3})",
            (c.r * 255.0) as i64,
            (c.g * 255.0) as i64,
            (c.b * 255.0) as i64,
            alpha
        )
    }
    let mut m: HashMap<&'static str, String> = HashMap::new();
    m.insert(
        "light",
        if colors.is_light() { "light" } else { "dark" }.to_string(),
    );
    m.insert("text", rgba(colors.text, 1.0));
    m.insert("text92", rgba(colors.text, 0.92));
    m.insert("dim", rgba(colors.dim, 1.0));
    m.insert("accent", rgba(colors.accent_on(), 1.0));
    m.insert("mantle", rgba(colors.mantle(), 1.0));
    m.insert("hairline", rgba(colors.hairline(), 1.0));
    let warn = colors.tone(PopupTone::Warning);
    m.insert("warn", rgba(warn, 1.0));
    m.insert("warn35", rgba(warn, 0.35));
    m.insert("warn75", rgba(warn, 0.75));
    m.insert("info", rgba(colors.tone(PopupTone::Info), 1.0));
    m
}

/// A themed placeholder preview page (a reduced `_PAGE` from
/// `pylib/confluence_pages.py`) so the embeddable surface renders without the
/// python worker or network. The title is HTML-escaped; the body is trusted
/// caller markup.
pub fn placeholder_html(colors: &PopupColors, title: &str, body: &str) -> String {
    let c = page_colors(colors);
    let get = |k: &str| c.get(k).map(String::as_str).unwrap_or("");
    let esc = title
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\n<style>\n\
:root {{ color-scheme: {light}; }}\n\
html, body {{ background: transparent; }}\n\
body {{ font: 14px/1.6 -apple-system, \"SF Pro Text\", sans-serif; color: {text};\n\
  margin: 0; padding: 14px 22px 80px; overflow-wrap: anywhere; }}\n\
h1, h2, h3 {{ line-height: 1.3; margin: 1.2em 0 .4em; }}\n\
a {{ color: {accent}; }}\n\
p, li {{ color: {text92}; }}\n\
.dim {{ color: {dim}; }}\n\
code {{ font: 12.5px ui-monospace, \"SF Mono\", monospace; background: {mantle};\n\
  padding: 1px 4px; border-radius: 4px; }}\n\
pre {{ font: 12.5px/1.45 ui-monospace, \"SF Mono\", monospace; background: {mantle};\n\
  padding: 10px 12px; border-radius: 6px; overflow: auto; border: 1px solid {hairline}; }}\n\
.ws-title {{ font-size: 1.6em; font-weight: 650; margin: .2em 0 .6em; }}\n\
</style></head><body><div class=\"ws-title\">{title}</div>{body}</body></html>",
        light = get("light"),
        text = get("text"),
        text92 = get("text92"),
        accent = get("accent"),
        dim = get("dim"),
        mantle = get("mantle"),
        hairline = get("hairline"),
        title = esc,
        body = body,
    )
}

/// The `WKWebView` surface of the Confluence preview and the `wsconf://`
/// scheme handler, plus `ConfluenceWindow`'s AppKit shell.
#[cfg(target_os = "macos")]
pub mod macos {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::ptr::NonNull;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread;

    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, ProtocolObject};
    use objc2::{
        define_class, msg_send, AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly,
    };
    use objc2_app_kit::{
        NSAutoresizingMaskOptions, NSButton, NSControlSize, NSFont, NSLineBreakMode,
        NSProgressIndicator, NSProgressIndicatorStyle, NSScrollView, NSTextAlignment, NSTextField,
        NSView, NSWorkspace,
    };
    use objc2_foundation::{
        ns_string, NSData, NSDictionary, NSError, NSHTTPURLResponse, NSMutableURLRequest, NSNumber,
        NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSURL, NSURLConnection, NSURLResponse,
        NSString,
    };
    use objc2_web_kit::{
        WKScriptMessage, WKScriptMessageHandler, WKURLSchemeHandler, WKURLSchemeTask,
        WKUserContentController, WKUserScript, WKUserScriptInjectionTime, WKWebView,
        WKWebViewConfiguration,
    };

    use super::{
        content_layout, placeholder_html, ConfluenceConfig, ConfluenceImageLoader,
        ConfluenceModel, HitPayload, Scope, StatusTone, CONF_RESULT_ROW_HEIGHT,
        CONF_STRIP_ROW_HEIGHT,
    };
    use crate::ui::chrome::Rect;
    use crate::ui::theme::{PopupColors, PopupThemeDefaults, PopupTone, Rgba};

    fn as_any<T: objc2::Message + ?Sized>(obj: &T) -> &AnyObject {
        unsafe { &*(obj as *const T as *const AnyObject) }
    }

    fn task_key(task: &ProtocolObject<dyn WKURLSchemeTask>) -> usize {
        NonNull::from(task).as_ptr() as usize
    }

    fn nserror(code: isize) -> Retained<NSError> {
        unsafe {
            NSError::errorWithDomain_code_userInfo(ns_string!("NSURLErrorDomain"), code, None)
        }
    }

    fn open_external(url: &str) {
        if let Some(ns) = NSURL::URLWithString(&NSString::from_str(url)) {
            NSWorkspace::sharedWorkspace().openURL(&ns);
        }
    }

    fn dict_str(dict: &NSDictionary<AnyObject, AnyObject>, key: &str) -> Option<String> {
        let k = NSString::from_str(key);
        let v = dict.objectForKey(as_any(&*k))?;
        as_any(&*v).downcast_ref::<NSString>().map(|s| s.to_string())
    }

    fn dict_i64(dict: &NSDictionary<AnyObject, AnyObject>, key: &str) -> Option<i64> {
        let k = NSString::from_str(key);
        let v = dict.objectForKey(as_any(&*k))?;
        as_any(&*v)
            .downcast_ref::<NSNumber>()
            .map(|n| n.integerValue() as i64)
    }

    // -----------------------------------------------------------------------
    // `wsconf://` scheme handler (`ConfluenceImageLoader` in Swift).
    // -----------------------------------------------------------------------

    pub struct ImageSchemeHandlerIvars {
        auth: RefCell<Option<String>>,
        inflight: RefCell<HashMap<usize, Arc<AtomicBool>>>,
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSConfluenceImageSchemeHandler"]
        #[ivars = ImageSchemeHandlerIvars]
        pub struct ImageSchemeHandler;

        impl ImageSchemeHandler {
            #[unsafe(method(webView:startURLSchemeTask:))]
            fn start_task(
                &self,
                _web_view: &WKWebView,
                url_scheme_task: &ProtocolObject<dyn WKURLSchemeTask>,
            ) {
                self.begin(url_scheme_task);
            }

            #[unsafe(method(webView:stopURLSchemeTask:))]
            fn stop_task(
                &self,
                _web_view: &WKWebView,
                url_scheme_task: &ProtocolObject<dyn WKURLSchemeTask>,
            ) {
                self.cancel(url_scheme_task);
            }
        }

        unsafe impl NSObjectProtocol for ImageSchemeHandler {}
        unsafe impl WKURLSchemeHandler for ImageSchemeHandler {}
    );

    /// A retained scheme task moved to a worker thread for a blocking fetch.
    struct SendTask(Retained<ProtocolObject<dyn WKURLSchemeTask>>);
    unsafe impl Send for SendTask {}

    impl SendTask {
        fn task(&self) -> &ProtocolObject<dyn WKURLSchemeTask> {
            &self.0
        }
    }

    #[allow(deprecated)]
    fn fetch_sync(
        real: &str,
        auth: Option<&str>,
    ) -> Result<(Retained<NSData>, Retained<NSURLResponse>), Retained<NSError>> {
        let Some(ns_url) = NSURL::URLWithString(&NSString::from_str(real)) else {
            return Err(nserror(-1000));
        };
        let req = NSMutableURLRequest::initWithURL(NSMutableURLRequest::alloc(), &ns_url);
        req.setTimeoutInterval(20.0);
        if let Some(a) = auth {
            req.setValue_forHTTPHeaderField(Some(&NSString::from_str(a)), ns_string!("Authorization"));
        }
        let mut resp: Option<Retained<NSURLResponse>> = None;
        let data =
            NSURLConnection::sendSynchronousRequest_returningResponse_error(&req, Some(&mut resp))?;
        let response = resp.ok_or_else(|| nserror(-1000))?;
        if let Some(http) = as_any(&*response).downcast_ref::<NSHTTPURLResponse>() {
            if http.statusCode() >= 400 {
                return Err(nserror(-1000));
            }
        }
        Ok((data, response))
    }

    fn deliver(
        task: &ProtocolObject<dyn WKURLSchemeTask>,
        result: Result<(Retained<NSData>, Retained<NSURLResponse>), Retained<NSError>>,
    ) {
        // WKURLSchemeTask callbacks may be made from any thread; the common
        // URLSession-delegate pattern calls back off the main thread too.
        match result {
            Ok((data, response)) => unsafe {
                task.didReceiveResponse(&response);
                task.didReceiveData(&data);
                task.didFinish();
            },
            Err(err) => unsafe { task.didFailWithError(&err) },
        }
    }

    impl ImageSchemeHandler {
        pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(ImageSchemeHandlerIvars {
                auth: RefCell::new(None),
                inflight: RefCell::new(HashMap::new()),
            });
            unsafe { msg_send![super(this), init] }
        }

        pub fn set_auth(&self, header: Option<String>) {
            *self.ivars().auth.borrow_mut() = header;
        }

        fn begin(&self, task: &ProtocolObject<dyn WKURLSchemeTask>) {
            let request = unsafe { task.request() };
            let Some(url) = request.URL().and_then(|u| u.absoluteString()) else {
                deliver(task, Err(nserror(-1000)));
                return;
            };
            let Some(real) = super::wsconf_unwrap(&url.to_string()) else {
                deliver(task, Err(nserror(-1000)));
                return;
            };
            let auth = self.ivars().auth.borrow().clone();
            let key = task_key(task);
            let flag = Arc::new(AtomicBool::new(false));
            self.ivars()
                .inflight
                .borrow_mut()
                .insert(key, flag.clone());
            let owned = SendTask(
                unsafe { Retained::retain(NonNull::from(task).as_ptr()) }.expect("scheme task"),
            );
            thread::spawn(move || {
                let result = fetch_sync(&real, auth.as_deref());
                if flag.load(Ordering::SeqCst) {
                    return;
                }
                deliver(owned.task(), result);
            });
        }

        fn cancel(&self, task: &ProtocolObject<dyn WKURLSchemeTask>) {
            if let Some(flag) = self.ivars().inflight.borrow_mut().remove(&task_key(task)) {
                flag.store(true, Ordering::SeqCst);
            }
        }
    }

    // -----------------------------------------------------------------------
    // `userContentController(_:didReceive:)` — the JSON highlighter hook.
    // -----------------------------------------------------------------------

    pub struct ScriptHandlerIvars {
        last: RefCell<Option<HitPayload>>,
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSConfluenceScriptHandler"]
        #[ivars = ScriptHandlerIvars]
        pub struct ScriptHandler;

        impl ScriptHandler {
            #[unsafe(method(userContentController:didReceiveScriptMessage:))]
            fn did_receive(
                &self,
                _controller: &WKUserContentController,
                message: &WKScriptMessage,
            ) {
                self.handle_body(unsafe { &message.body() });
            }
        }

        unsafe impl NSObjectProtocol for ScriptHandler {}
        unsafe impl WKScriptMessageHandler for ScriptHandler {}
    );

    impl ScriptHandler {
        pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(ScriptHandlerIvars {
                last: RefCell::new(None),
            });
            unsafe { msg_send![super(this), init] }
        }

        pub fn last_hit(&self) -> Option<HitPayload> {
            self.ivars().last.borrow().clone()
        }

        fn handle_body(&self, body: &AnyObject) {
            // The injected link script posts a JSON string.
            if let Some(s) = as_any(body).downcast_ref::<NSString>() {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s.to_string()) {
                    if let Some(href) = v.get("href").and_then(|h| h.as_str()) {
                        open_external(href);
                    }
                }
                return;
            }
            // The python highlighter posts `{i, n, snippet}` (and `href`).
            if let Some(dict) = as_any(body).downcast_ref::<NSDictionary<AnyObject, AnyObject>>() {
                if let Some(href) = dict_str(dict, "href") {
                    open_external(&href);
                    return;
                }
                *self.ivars().last.borrow_mut() = Some(HitPayload {
                    index: dict_i64(dict, "i").unwrap_or(-1),
                    count: dict_i64(dict, "n").unwrap_or(0),
                    snippet: dict_str(dict, "snippet").unwrap_or_default(),
                });
            }
        }
    }

    // -----------------------------------------------------------------------
    // The preview web view + the deferred `ConfluenceWindow` shell.
    // -----------------------------------------------------------------------

    pub struct ConfluencePreview {
        pub web: Retained<WKWebView>,
        pub script: Retained<ScriptHandler>,
        pub images: Retained<ImageSchemeHandler>,
    }

    impl ConfluencePreview {
        pub fn new(mtm: MainThreadMarker, loader: &mut ConfluenceImageLoader) -> Self {
            let images = loader.handler(mtm);
            let config = unsafe { WKWebViewConfiguration::new(mtm) };
            unsafe {
                config.setURLSchemeHandler_forURLScheme(
                    Some(ProtocolObject::from_ref(&*images)),
                    ns_string!("wsconf"),
                );
            }
            let ucc = unsafe { config.userContentController() };
            let script = ScriptHandler::new(mtm);
            unsafe {
                ucc.addScriptMessageHandler_name(ProtocolObject::from_ref(&*script), ns_string!("ws"));
            }
            let src = NSString::from_str(super::link_user_script());
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
            let no = NSNumber::numberWithBool(false);
            let _: () = unsafe {
                msg_send![&*web, setValue: &*no, forKey: ns_string!("drawsBackground")]
            };
            web.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            ConfluencePreview { web, script, images }
        }

        /// `render(_:_:)` — `web.loadHTMLString(html, baseURL: URL(string: base))`.
        pub fn load_html(&self, html: &str, base: Option<&str>) {
            let base_url = base.and_then(|b| NSURL::URLWithString(&NSString::from_str(b)));
            unsafe {
                self.web.loadHTMLString_baseURL(
                    &NSString::from_str(html),
                    base_url.as_deref(),
                );
            }
        }

        /// `nextHit` / `prevHit` — `web.evaluateJavaScript("window.wsNext && wsNext()")`.
        pub fn next_hit(&self) {
            self.eval("window.wsNext && wsNext()");
        }

        pub fn prev_hit(&self) {
            self.eval("window.wsPrev && wsPrev()");
        }

        fn eval(&self, js: &str) {
            unsafe {
                self.web
                    .evaluateJavaScript_completionHandler(&NSString::from_str(js), None);
            }
        }
    }

    // -----------------------------------------------------------------------
    // The embeddable content view (`ConfluenceWindow.build` + `layoutAll`).
    // -----------------------------------------------------------------------

    pub struct ConfRootIvars;

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSConfluenceFlippedView"]
        #[ivars = ConfRootIvars]
        pub struct ConfFlippedView;

        impl ConfFlippedView {
            #[unsafe(method(isFlipped))]
            fn is_flipped(&self) -> bool {
                true
            }
        }

        unsafe impl NSObjectProtocol for ConfFlippedView {}
    );

    impl ConfFlippedView {
        fn new(mtm: MainThreadMarker) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(ConfRootIvars);
            unsafe {
                msg_send![
                    super(this),
                    initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0))
                ]
            }
        }
    }

    fn nsrect(r: Rect) -> NSRect {
        NSRect::new(NSPoint::new(r.x, r.y), NSSize::new(r.width, r.height))
    }

    fn set_background(view: &NSView, color: Rgba) {
        view.setWantsLayer(true);
        if let Some(layer) = view.layer() {
            layer.setBackgroundColor(Some(&color.to_nscolor().CGColor()));
        }
    }

    fn label(
        mtm: MainThreadMarker,
        s: &str,
        size: f64,
        bold: bool,
        color: Rgba,
    ) -> Retained<NSTextField> {
        let l = NSTextField::labelWithString(&NSString::from_str(s), mtm);
        let font = if bold {
            NSFont::boldSystemFontOfSize(size)
        } else {
            NSFont::systemFontOfSize(size)
        };
        l.setFont(Some(&font));
        l.setTextColor(Some(&color.to_nscolor()));
        l.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        l
    }

    fn button(mtm: MainThreadMarker, title: &str) -> Retained<NSButton> {
        let b = unsafe {
            NSButton::buttonWithTitle_target_action(&NSString::from_str(title), None, None, mtm)
        };
        b.setControlSize(NSControlSize::Small);
        b.setFont(Some(&NSFont::systemFontOfSize(11.5)));
        b
    }

    fn tone_color(colors: &PopupColors, tone: StatusTone) -> Rgba {
        match tone {
            StatusTone::Dim => colors.dim,
            StatusTone::Success => colors.tone(PopupTone::Success),
            StatusTone::Warning => colors.tone(PopupTone::Warning),
            StatusTone::Danger => colors.tone(PopupTone::Danger),
        }
    }

    thread_local! {
        static LIVE_WINDOWS: RefCell<Vec<(Retained<crate::ui::card::CardNSWindow>, ConfluenceImageLoader, ConfluencePreview)>>
            = const { RefCell::new(Vec::new()) };
        /// Keeps the embeddable surface's preview + scheme/script handlers alive
        /// for the life of the main thread (the host retains the root view; the
        /// `WKWebView` configuration does not retain its handlers reliably).
        static LIVE_CONTENT: RefCell<Vec<ConfluencePreview>> = const { RefCell::new(Vec::new()) };
    }

    /// `ConfluenceWindow`'s AppKit build: a themed card hosting the preview.
    /// The full search strip / table / splitter are separate surfaces; the
    /// preview + scheme handler are the part this module owns. The window is
    /// kept alive for the life of the main thread (a real controller would own
    /// it; the model here is headless).
    pub fn build_confluence_window() {
        use crate::ui::card::{create_card_window, CardConfig};
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let mut loader = ConfluenceImageLoader::new();
        let preview = ConfluencePreview::new(mtm, &mut loader);
        let config = CardConfig {
            title: "Confluence".to_string(),
            min_size: (480.0, 360.0),
            ..Default::default()
        };
        let window = create_card_window(mtm, &config);
        window.setContentView(Some(&preview.web));
        window.center();
        window.makeKeyAndOrderFront(None);
        LIVE_WINDOWS.with(|w| w.borrow_mut().push((window, loader, preview)));
    }

    /// `ConfluenceWindow.build()` + `layoutAll(_:)` as an embeddable content
    /// view: search strip on top, results list (left), a splitter, and the
    /// live `WKWebView` preview (right) rendering a themed placeholder page.
    /// Built without the python worker or network — the table shows the
    /// model's rows (empty for a fresh process).
    pub fn build_content(mtm: MainThreadMarker) -> Option<Retained<NSView>> {
        let colors = PopupThemeDefaults::colors();
        let config = ConfluenceConfig::default();
        let mut model = ConfluenceModel::default();
        model.show_search_empty();

        let split = config.split_clamped();
        let sidebar_w = config.sidebar_width();
        let width = config.width();
        let height = config.height();

        let mut loader = ConfluenceImageLoader::new();
        let preview = ConfluencePreview::new(mtm, &mut loader);
        let html = placeholder_html(
            &colors,
            "Confluence",
            "<p class=dim>No page loaded yet \u{2014} search above and pick a result.</p>",
        );
        preview.load_html(&html, None);

        let root = ConfFlippedView::new(mtm);
        root.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, height)));
        root.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        set_background(&root, colors.mantle());

        let layout = content_layout(width, height, split, sidebar_w, model.rows.len());

        // --- sidebar ------------------------------------------------------
        let sidebar = ConfFlippedView::new(mtm);
        sidebar.setFrame(nsrect(layout.sidebar));
        sidebar.setAutoresizingMask(NSAutoresizingMaskOptions::ViewHeightSizable);
        set_background(&sidebar, colors.crust());
        let side_w = layout.sidebar.width;
        let side_title = label(mtm, "Confluence", 13.0, true, colors.text);
        side_title.setFrame(NSRect::new(
            NSPoint::new(12.0, 10.0),
            NSSize::new((side_w - 24.0).max(0.0), 18.0),
        ));
        sidebar.addSubview(&side_title);
        let scopes = [
            ("Search", model.scope == Scope::Search),
            ("Favorites", model.scope == Scope::Favorites),
        ];
        for (i, (name, active)) in scopes.iter().enumerate() {
            let color = if *active { colors.text } else { colors.dim };
            let row = label(mtm, name, 12.5, *active, color);
            row.setFrame(NSRect::new(
                NSPoint::new(12.0, 40.0 + i as f64 * 26.0),
                NSSize::new((side_w - 24.0).max(0.0), 18.0),
            ));
            sidebar.addSubview(&row);
        }
        root.addSubview(&sidebar);

        // --- search strip -------------------------------------------------
        let strip = ConfFlippedView::new(mtm);
        strip.setFrame(nsrect(layout.strip));
        strip.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        set_background(&strip, colors.mantle().with_alpha(0.55));
        let strip_w = layout.strip.width;
        let field_w = (strip_w - 24.0 - 360.0).max(80.0);

        let search = NSTextField::textFieldWithString(&NSString::from_str(""), mtm);
        search.setPlaceholderString(Some(&NSString::from_str("Search Confluence")));
        search.setFont(Some(&NSFont::systemFontOfSize(13.0)));
        search.setBezeled(true);
        search.setEditable(true);
        search.setSelectable(true);
        search.setFrame(NSRect::new(
            NSPoint::new(12.0, 10.0),
            NSSize::new(field_w, CONF_STRIP_ROW_HEIGHT),
        ));
        search.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        strip.addSubview(&search);

        let mut chip_x = 12.0 + field_w + 8.0;
        for (text, w) in [("Every word", 84.0), ("Title only", 78.0)] {
            let chip = label(mtm, text, 12.0, false, colors.text);
            chip.setFrame(NSRect::new(
                NSPoint::new(chip_x, 10.0),
                NSSize::new(w, CONF_STRIP_ROW_HEIGHT),
            ));
            chip.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
            strip.addSubview(&chip);
            chip_x += w + 8.0;
        }
        let search_btn = button(mtm, "Search");
        search_btn.setFrame(NSRect::new(
            NSPoint::new((strip_w - 12.0 - 84.0).max(0.0), 10.0),
            NSSize::new(84.0, CONF_STRIP_ROW_HEIGHT),
        ));
        search_btn.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
        strip.addSubview(&search_btn);

        let bottom_y = 10.0 + CONF_STRIP_ROW_HEIGHT + 8.0;
        let mut bx2 = 12.0;
        for (text, w) in [
            ("All spaces", 190.0),
            ("Any contributor", 190.0),
            ("Type: Pages + blogs", 150.0),
            ("Modified: any time", 140.0),
            ("Sort: relevance", 120.0),
        ] {
            let chip = label(mtm, text, 12.0, false, colors.dim);
            chip.setFrame(NSRect::new(
                NSPoint::new(bx2, bottom_y),
                NSSize::new(w, CONF_STRIP_ROW_HEIGHT),
            ));
            strip.addSubview(&chip);
            bx2 += w + 8.0;
        }
        root.addSubview(&strip);

        // --- results table ------------------------------------------------
        let scroll = NSScrollView::new(mtm);
        scroll.setFrame(nsrect(layout.table));
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setDrawsBackground(false);
        scroll.setAutoresizingMask(NSAutoresizingMaskOptions::ViewHeightSizable);
        let rows_view = ConfFlippedView::new(mtm);
        rows_view.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        let row_w = layout.table.width.max(1.0);
        for (i, r) in model.rows.iter().enumerate() {
            let row = ConfFlippedView::new(mtm);
            row.setFrame(NSRect::new(
                NSPoint::new(0.0, i as f64 * CONF_RESULT_ROW_HEIGHT),
                NSSize::new(row_w, CONF_RESULT_ROW_HEIGHT),
            ));
            row.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
            let title = label(mtm, &r.title, 13.0, false, colors.text);
            title.setFrame(NSRect::new(
                NSPoint::new(10.0, 6.0),
                NSSize::new((row_w - 20.0).max(1.0), 18.0),
            ));
            row.addSubview(&title);
            let meta = label(mtm, &r.meta(), 11.0, false, colors.dim);
            meta.setFrame(NSRect::new(
                NSPoint::new(10.0, 24.0),
                NSSize::new((row_w - 20.0).max(1.0), 14.0),
            ));
            row.addSubview(&meta);
            rows_view.addSubview(&row);
        }
        rows_view.setFrame(NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(row_w, layout.rows_height),
        ));
        scroll.setDocumentView(Some(&rows_view));
        root.addSubview(&scroll);

        let empty = label(mtm, &model.status, 12.0, false, colors.dim);
        empty.setFrame(nsrect(layout.empty_hint));
        empty.setHidden(!model.rows.is_empty());
        root.addSubview(&empty);

        // --- footer (spinner + status + more/cql/curl) --------------------
        let spinner = NSProgressIndicator::new(mtm);
        spinner.setStyle(NSProgressIndicatorStyle::Spinning);
        spinner.setControlSize(NSControlSize::Small);
        spinner.setDisplayedWhenStopped(false);
        spinner.setFrame(nsrect(layout.spinner));
        root.addSubview(&spinner);

        let status = label(mtm, &model.status, 11.0, false, tone_color(&colors, model.status_tone));
        status.setFrame(nsrect(layout.status));
        status.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        root.addSubview(&status);

        let more = button(mtm, "More\u{2026}");
        more.setEnabled(!model.next_link.is_empty());
        more.setFrame(nsrect(layout.more));
        more.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
        root.addSubview(&more);
        let cql = button(mtm, "CQL");
        cql.setEnabled(!model.last_cql.is_empty());
        cql.setFrame(nsrect(layout.cql));
        cql.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
        root.addSubview(&cql);
        let curl = button(mtm, "curl");
        curl.setEnabled(!model.last_curl.is_empty());
        curl.setFrame(nsrect(layout.curl));
        curl.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
        root.addSubview(&curl);

        // --- splitter -----------------------------------------------------
        let splitter = ConfFlippedView::new(mtm);
        splitter.setFrame(nsrect(layout.splitter));
        splitter.setAutoresizingMask(NSAutoresizingMaskOptions::ViewHeightSizable);
        set_background(&splitter, colors.border.with_alpha(0.6));
        root.addSubview(&splitter);

        // --- preview pane -------------------------------------------------
        let pane = ConfFlippedView::new(mtm);
        pane.setFrame(nsrect(layout.preview));
        pane.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        set_background(&pane, colors.mantle().with_alpha(0.35));

        let p_title = label(mtm, &model.where_text(), 15.0, true, colors.text);
        p_title.setFrame(nsrect(layout.p_title));
        pane.addSubview(&p_title);

        let p_meta = label(mtm, "", 11.0, false, colors.dim);
        p_meta.setFrame(nsrect(layout.p_meta));
        pane.addSubview(&p_meta);

        let p_star = button(mtm, "\u{2606}");
        p_star.setFrame(nsrect(layout.p_star));
        p_star.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
        pane.addSubview(&p_star);
        let p_open = button(mtm, "Open");
        p_open.setFrame(nsrect(layout.p_open));
        p_open.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
        pane.addSubview(&p_open);
        let p_copy = button(mtm, "Copy Link");
        p_copy.setFrame(nsrect(layout.p_copy));
        p_copy.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
        pane.addSubview(&p_copy);

        let hits_bar = ConfFlippedView::new(mtm);
        hits_bar.setFrame(nsrect(layout.hits_bar));
        hits_bar.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        set_background(&hits_bar, colors.mantle().with_alpha(0.7));
        let hit_prev = button(mtm, "\u{2039}");
        hit_prev.setFrame(nsrect(layout.hit_prev));
        hits_bar.addSubview(&hit_prev);
        let hit_next = button(mtm, "\u{203a}");
        hit_next.setFrame(nsrect(layout.hit_next));
        hits_bar.addSubview(&hit_next);
        let hit_label = label(mtm, "No matches", 12.0, false, colors.text.with_alpha(0.85));
        hit_label.setFrame(nsrect(layout.hit_label));
        hit_label.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        hits_bar.addSubview(&hit_label);
        pane.addSubview(&hits_bar);

        preview.web.setFrame(nsrect(layout.web));
        pane.addSubview(&preview.web);

        let hint = label(
            mtm,
            "No page loaded \u{2014} results appear here",
            13.0,
            false,
            colors.dim,
        );
        hint.setAlignment(NSTextAlignment::Center);
        hint.setFrame(nsrect(layout.hint));
        hint.setHidden(true);
        pane.addSubview(&hint);

        let setup = button(mtm, "Set Up Confluence\u{2026}");
        setup.setFrame(nsrect(layout.setup));
        setup.setHidden(true);
        pane.addSubview(&setup);

        root.addSubview(&pane);

        LIVE_CONTENT.with(|v| v.borrow_mut().push(preview));
        Some(root.into_super())
    }
}

/// The raw `confluence_api.py` runner (via `run_process`, mirroring
/// `ConfluenceAPI.run`).
pub fn run_api(args: &[&str], stdin: Option<&[u8]>) -> Result<Value, String> {
    let dir = env!("CARGO_MANIFEST_DIR");
    let script = format!("{dir}/../../confluence/confluence_api.py");
    let python = find_python3().ok_or("confluence: python 3.11+ not found")?;
    let mut argv = vec![script];
    argv.extend(args.iter().map(|s| s.to_string()));
    let out = crate::app::process_run::run_process(&python, &argv, stdin, None, false)
        .map_err(|e| format!("confluence_api.py {args:?}: {e}"))?;
    let line = out.out.lines().last().unwrap_or("").trim();
    if line.is_empty() {
        return Err(format!(
            "confluence_api.py {args:?} exited {}: {}",
            out.code,
            out.err.trim()
        ));
    }
    serde_json::from_str(line).map_err(|e| format!("confluence_api.py {args:?}: bad JSON: {e}"))
}

fn find_python3() -> Option<String> {
    let mut candidates: Vec<String> = Vec::new();
    if let Ok(p) = std::env::var("WS_PYTHON") {
        if !p.is_empty() {
            candidates.push(p);
        }
    }
    candidates.push("/opt/homebrew/bin/python3".into());
    candidates.push("/usr/local/bin/python3".into());
    candidates.push("python3".into());
    for cand in candidates {
        let ok = crate::app::process_run::run_process(
            &cand,
            &[
                "-c".to_string(),
                "import sys; sys.exit(0 if sys.version_info >= (3, 11) else 1)".to_string(),
            ],
            None,
            None,
            false,
        )
        .map(|o| o.code == 0)
        .unwrap_or(false);
        if ok {
            return Some(cand);
        }
    }
    None
}

/// `ConfluenceWindow` — host the `WKWebView` preview + `wsconf://` handler.
pub fn build_window() {
    #[cfg(target_os = "macos")]
    macos::build_confluence_window();
}

/// The embeddable Confluence content surface for the shared host window.
///
/// A flipped root with the search strip (search field + criteria chips), the
/// results list on the left, a splitter, and the live `WKWebView` preview on
/// the right (a themed placeholder page). `None` off the main thread.
#[cfg(target_os = "macos")]
pub fn build_content(
    mtm: objc2::MainThreadMarker,
) -> Option<objc2::rc::Retained<objc2_app_kit::NSView>> {
    macos::build_content(mtm)
}

// ---------------------------------------------------------------------------
// base64 (std-only; matches NSData.base64EncodedString + the url-safe wrap)
// ---------------------------------------------------------------------------

const B64_STD: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const B64_URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

pub fn base64_encode(data: &[u8], urlsafe: bool) -> String {
    let table = if urlsafe { B64_URL } else { B64_STD };
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(table[((triple >> 18) & 0x3F) as usize] as char);
        out.push(table[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(table[((triple >> 6) & 0x3F) as usize] as char);
        } else if !urlsafe {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(table[(triple & 0x3F) as usize] as char);
        } else if !urlsafe {
            out.push('=');
        }
    }
    out
}

pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut vals: Vec<u8> = Vec::new();
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => continue,
            _ => return None,
        };
        vals.push(v);
    }
    let mut out = Vec::with_capacity(vals.len() * 3 / 4);
    for chunk in vals.chunks(4) {
        if chunk.len() < 2 {
            return None;
        }
        let mut triple = 0u32;
        for (i, v) in chunk.iter().enumerate() {
            triple |= (*v as u32) << (18 - 6 * i);
        }
        out.push(((triple >> 16) & 0xFF) as u8);
        if chunk.len() > 2 {
            out.push(((triple >> 8) & 0xFF) as u8);
        }
        if chunk.len() > 3 {
            out.push((triple & 0xFF) as u8);
        }
    }
    Some(out)
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
    use serde_json::json;

    fn entries(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn config_parse_defaults_and_overrides() {
        let empty = ConfluenceConfig::default();
        assert_eq!(empty.width(), DEFAULT_WIDTH);
        assert_eq!(empty.height(), DEFAULT_HEIGHT);
        assert_eq!(empty.split(), DEFAULT_SPLIT);
        assert_eq!(empty.sidebar_width(), DEFAULT_SIDEBAR_WIDTH);
        assert!(!empty.enabled());
        assert!(empty.in_palette(), "in-palette defaults true");
        assert_eq!(empty.label(), "Confluence Search");

        let c = ConfluenceConfig::from_entries(entries(&[
            ("enabled", "true"),
            ("width", "1000"),
            ("height", "700"),
            ("split", "0.7"),
            ("sidebar-width", "180"),
            ("label", "Wiki"),
            ("in-palette", "no"),
        ]));
        assert!(c.enabled());
        assert_eq!(c.width(), 1000.0);
        assert_eq!(c.height(), 700.0);
        assert_eq!(c.split(), 0.7);
        assert_eq!(c.sidebar_width(), 180.0);
        assert_eq!(c.label(), "Wiki");
        assert!(!c.in_palette());
    }

    #[test]
    fn split_is_clamped_like_the_splitter() {
        // UserDefaults garbage resets to the section default.
        assert_eq!(clamp_split(0.5), 0.5);
        assert_eq!(clamp_split(0.1), DEFAULT_SPLIT);
        assert_eq!(clamp_split(0.9), DEFAULT_SPLIT);
        assert_eq!(
            ConfluenceConfig::from_entries(entries(&[("split", "0.05")])).split_clamped(),
            DEFAULT_SPLIT
        );
    }

    #[test]
    fn row_parses_json_and_hits() {
        // hits are UTF-16 ranges as [location, length] pairs.
        let j = json!({
            "id": "42", "type": "blogpost", "title": "Deploy \u{1f680}",
            "excerpt": "deploy notes", "space": "ENG", "spaceName": "Engineering",
            "path": "Home/Deploy", "author": "Ada", "modifiedText": "yesterday",
            "url": "https://x/wiki/p", "favorite": true, "missing": false,
            "titleHits": [[0, 6]], "hits": [[0, 6], [7, 5]]
        });
        let r = ConfluenceRow::from_json(&j);
        assert_eq!(r.id, "42");
        assert_eq!(r.type_label(), "Blog");
        assert!(r.favorite);
        assert_eq!(r.hits.len(), 2);
        assert_eq!(r.hits[1], HitRange { location: 7, length: 5 });
        assert_eq!(r.to_json()["type"], "blogpost");
        assert_eq!(r.meta(), "Engineering  \u{00b7}  Home/Deploy  \u{00b7}  Ada  \u{00b7}  yesterday");

        // missing entries fall back / stringify numbers.
        let sparse = ConfluenceRow::from_json(&json!({"id": 7, "hits": [[1]]}));
        assert_eq!(sparse.id, "7");
        assert_eq!(sparse.kind, "page");
        assert!(sparse.hits.is_empty(), "ragged ranges dropped");
        assert_eq!(sparse.type_label(), "");
    }

    #[test]
    fn criteria_matches_the_glue() {
        // mirrors Tests/test_confluence_glue.py Cases.
        let base = CriteriaParams {
            query: " retry logic ".to_string(),
            ..Default::default()
        };
        assert_eq!(
            criteria(&base),
            json!({
                "query": "retry logic", "mode": "all", "titleOnly": false,
                "spaces": [], "types": ["page", "blogpost"],
                "modified": "", "contributors": [], "sort": "relevance"
            })
        );

        let mut p = base.clone();
        p.mode_index = 1;
        assert_eq!(criteria(&p)["mode"], "phrase");
        p.mode_index = 2;
        assert_eq!(criteria(&p)["mode"], "any");
        p.mode_index = 9;
        assert_eq!(criteria(&p)["mode"], "all", "out of range clamps to all");

        let mut p = base.clone();
        p.spaces_all = false;
        p.spaces = vec!["DEV".into()];
        p.contributors_all = false;
        p.contributors = vec!["me".into()];
        p.title_only = true;
        let out = criteria(&p);
        assert_eq!(out["spaces"], json!(["DEV"]));
        assert_eq!(out["contributors"], json!(["me"]));
        assert_eq!(out["titleOnly"], true);

        let mut p = base.clone();
        p.spaces_all = true;
        p.spaces = vec!["DEV".into()];
        assert_eq!(criteria(&p)["spaces"], json!([]), "all empties the list");

        let mut p = base.clone();
        p.types = Some("page,,comment".into());
        assert_eq!(criteria(&p)["types"], json!(["page", "comment"]));
        p.types = Some(String::new());
        assert_eq!(criteria(&p)["types"], json!([]));

        let mut p = base.clone();
        p.favorites = true;
        assert!(criteria(&p).get("favorites").is_some());
        assert!(criteria(&base).get("favorites").is_none());

        let mut p = base.clone();
        p.modified = "-2w".into();
        p.sort = Some("modified".into());
        let out = criteria(&p);
        assert_eq!(out["modified"], "-2w");
        assert_eq!(out["sort"], "modified");
    }

    #[test]
    fn auth_header_matches_the_glue() {
        // mirrors Tests/test_confluence_glue.py Auth cases.
        assert_eq!(
            auth_header(&json!({"email": "a@x", "token": "tok"})),
            "Basic YUB4OnRvaw=="
        );
        assert_eq!(auth_header(&json!({"token": "tok"})), "Bearer tok");
        assert_eq!(
            auth_header(&json!({"auth": "bearer", "email": "a@x", "token": "t"})),
            "Bearer t"
        );
        assert_eq!(
            auth_header(&json!({"auth": "BASIC", "email": "", "token": "t"})),
            "Basic OnQ="
        );
        assert_eq!(
            auth_header(&json!({"auth": "oauth", "email": "a@x", "token": "t"}))
                .split(' ')
                .next(),
            Some("Basic")
        );
        assert_eq!(auth_header(&json!({"email": "a@x", "token": ""})), "");
        assert_eq!(auth_header(&json!({})), "");
        assert_eq!(auth_header(&Value::Null), "");
    }

    #[test]
    fn rate_limit_decision_matches_the_glue() {
        assert_eq!(rate_limit(&json!({})), RateLimitDecision { limited: false, seconds: 0 });
        assert_eq!(
            rate_limit(&Value::Null),
            RateLimitDecision { limited: false, seconds: 0 }
        );
        assert!(!rate_limit(&json!({"rateLimited": false})).limited);
        assert_eq!(
            rate_limit(&json!({"rateLimited": true, "retryIn": 45})),
            RateLimitDecision { limited: true, seconds: 45 }
        );
        assert_eq!(
            rate_limit(&json!({"rateLimited": true, "retryIn": 1})),
            RateLimitDecision { limited: true, seconds: 3 },
            "floor"
        );
        assert_eq!(
            rate_limit(&json!({"rateLimited": true})),
            RateLimitDecision { limited: true, seconds: 30 },
            "fallback"
        );
        assert_eq!(
            rate_limit(&json!({"rateLimited": true, "retryIn": "soon"})).seconds,
            30
        );
    }

    #[test]
    fn cooldown_lifecycle() {
        let mut c = Cooldown::default();
        assert!(!c.active(1000.0));
        assert!(!c.tick(1000.0), "idle tick never lifts");

        c.activate(45, 1000.0);
        assert!(c.active(1000.0));
        assert_eq!(c.remaining(1000.0), 45);
        assert_eq!(c.remaining(1030.0), 15);
        assert!(!c.tick(1030.0), "still counting");
        assert!(c.active(1044.0));
        assert!(c.tick(1045.0), "lifts at/after the deadline");
        assert!(!c.active(1045.0));
        assert!(!c.tick(1045.0));
    }

    #[test]
    fn model_rate_limited_starts_and_ticks() {
        let mut m = ConfluenceModel::default();
        assert!(!m.handle_rate_limited(&json!({"ok": false}), 1000.0));
        assert!(m.handle_rate_limited(
            &json!({"ok": false, "rateLimited": true, "retryIn": 90}),
            1000.0
        ));
        assert!(m.cooling_down(1000.0));
        assert_eq!(m.cooldown.remaining(1000.0), 90);
        assert!(m.status.starts_with("Confluence rate limit"));
        m.tick_cooldown(1091.0);
        assert!(!m.cooling_down(1091.0));
        assert_eq!(m.status, "Resuming\u{2026}");
    }

    #[test]
    fn cooldown_file_left_for_site() {
        let dir = std::env::temp_dir().join(format!("ws-conf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("ratelimit.json");
        std::fs::write(
            &file,
            serde_json::to_vec(&json!({"site": "https://x/wiki", "until": 1100.0})).unwrap(),
        )
        .unwrap();
        let f = file.to_string_lossy().into_owned();
        assert_eq!(cooldown_left_in(&f, "https://x/wiki", 1000.0), 100);
        assert_eq!(cooldown_left_in(&f, "https://other", 1000.0), 0, "site mismatch");
        assert_eq!(cooldown_left_in(&f, "https://x/wiki", 1200.0), 0, "expired");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn where_text_and_criteria_gate() {
        let mut m = ConfluenceModel::default();
        assert_eq!(m.where_text(), "Search");
        assert!(!m.has_criteria());
        m.query = "deploy".into();
        assert_eq!(m.where_text(), "\u{201c}deploy\u{201d}");
        assert!(m.has_criteria());
        m.showing_favorites = true;
        assert_eq!(m.where_text(), "Favorites");

        m.query.clear();
        m.showing_favorites = false;
        assert!(!m.has_criteria());
        m.query = "x".into();
        assert!(m.has_criteria());
        m.query.clear();
        m.modified = "7d".into();
        assert!(m.has_criteria());
    }

    #[test]
    fn favorites_filter_and_status() {
        let mut m = ConfluenceModel::default();
        m.favorites = vec![
            ConfluenceRow::from_json(&json!({"id": "1", "title": "Deploy runbook", "space": "OPS"})),
            ConfluenceRow::from_json(&json!({"id": "2", "title": "Team handbook", "spaceName": "People"})),
        ];
        m.query = "deploy".into();
        m.show_favorites(false, 1000.0);
        assert!(m.showing_favorites);
        assert_eq!(m.rows.len(), 1);
        assert_eq!(m.rows[0].id, "1");
        assert_eq!(m.terms, vec![json!({"text": "deploy", "phrase": false, "prefix": true})]);
        assert!(m.status.starts_with("\u{2605} 1 of 2 favorite"));
        assert_eq!(m.favorites_empty_hint(), "No favorites match \u{201c}deploy\u{201d}.\nThe Search button looks inside their text.");

        m.query.clear();
        m.show_favorites(false, 1000.0);
        assert_eq!(m.rows.len(), 2);
        assert!(m.status.starts_with("\u{2605} 2 favorite"));

        let empty = ConfluenceModel::default();
        assert!(empty.favorites_empty_hint().starts_with("No favorites yet."));
    }

    #[test]
    fn search_result_applies_rows_and_paging() {
        let mut m = ConfluenceModel::default();
        let page1 = json!({
            "ok": true, "cql": "text ~ \"x\"", "curl": "curl ...",
            "terms": [{"text": "x", "phrase": false, "prefix": false}],
            "total": 3, "next": "TOKEN",
            "results": [
                {"id": "1", "title": "a"}, {"id": "2", "title": "b"}
            ],
            "elapsed": 0.4
        });
        assert_eq!(m.apply_search_result(&page1, false, 1000.0), SearchOutcome::Ok);
        assert_eq!(m.rows.len(), 2);
        assert_eq!(m.total, 3);
        assert_eq!(m.next_link, "TOKEN");
        assert_eq!(m.last_cql, "text ~ \"x\"");
        assert_eq!(m.selection, 0);
        assert!(m.status.contains("2 of 3"));

        let page2 = json!({"ok": true, "results": [{"id": "3", "title": "c"}], "total": 3});
        m.apply_search_result(&page2, true, 1000.0);
        assert_eq!(m.rows.len(), 3);
        assert_eq!(m.rows[2].id, "3");

        let fail = json!({"ok": false, "error": "boom"});
        assert_eq!(m.apply_search_result(&fail, false, 1000.0), SearchOutcome::Failed);
        assert!(m.rows.is_empty(), "a failed full search clears the rows");
        assert_eq!(m.status, "boom");
        assert_eq!(m.status_tone, StatusTone::Danger);

        let limited = json!({"ok": false, "rateLimited": true, "retryIn": 90});
        assert_eq!(m.apply_search_result(&limited, true, 1000.0), SearchOutcome::RateLimited);
        assert!(m.cooling_down(1000.0));
    }

    #[test]
    fn toggle_favorite_and_preview_star() {
        let mut m = ConfluenceModel::default();
        m.rows = vec![ConfluenceRow::from_json(&json!({"id": "1", "title": "a"}))];
        assert_eq!(m.toggle_favorite(0), Some(true));
        assert!(m.rows[0].favorite);
        assert!(m.preview_star("1"));
        assert!(!m.preview_star("missing"));
        assert_eq!(m.toggle_favorite(0), Some(false));
        assert!(!m.rows[0].favorite);
        assert_eq!(m.toggle_favorite(9), None);
    }

    #[test]
    fn model_do_hooks_and_state() {
        let mut m = ConfluenceModel::default();
        m.favorites = vec![ConfluenceRow::from_json(&json!({"id": "1", "title": "a"}))];
        let st = m.handle_do("confluence:favorites").unwrap();
        assert_eq!(st["scope"], "favorites");
        assert_eq!(st["showingFavorites"], true);

        m.handle_do("confluence:search");
        let st = m.handle_do("confluence:query:deploy").unwrap();
        assert_eq!(st["query"], "deploy");
        assert_eq!(st["where"], "\u{201c}deploy\u{201d}");

        assert!(m.handle_do("confluence:nope").is_none());
        assert!(m.handle_do("confluence:select:x").is_none());
        assert!(m.handle_do("other:thing").is_none());

        let st = m.handle_do("confluence:state").unwrap();
        assert_eq!(st["configured"], false);
        assert_eq!(st["rows"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn wsconf_round_trip_and_base64() {
        let url = "https://x.atlassian.net/wiki/images/a.png?a=1&b=2";
        let wrapped = wsconf_wrap(url);
        assert!(wrapped.starts_with("wsconf://fetch/"));
        assert_eq!(wsconf_unwrap(&wrapped).as_deref(), Some(url));
        assert_eq!(base64_encode(b"a\"b\\c", false), "YSJiXGM=");
        assert_eq!(base64_encode("\u{1f680}".as_bytes(), false), "8J+agA==");
        // url-safe: no padding, - _ instead of + /
        assert_eq!(wsconf_wrap("a?b").contains('='), false);
    }

    #[test]
    fn link_user_script_posts_to_the_ws_handler() {
        let js = link_user_script();
        assert!(js.contains("messageHandlers.ws.postMessage"));
        assert!(js.contains("closest('a[href]')"));
        assert!(js.contains("JSON.stringify"));
    }

    #[test]
    fn loader_tracks_inflight_wsconf_urls() {
        let mut l = ConfluenceImageLoader::new();
        assert!(l.inflight.is_empty());
        let u = wsconf_wrap("https://x/a.png");
        l.start(&u);
        l.start(&u);
        assert_eq!(l.inflight, vec![u.clone()], "duplicate starts dedupe");
        l.stop(&u);
        assert!(l.inflight.is_empty());
        l.set_auth(Some("Bearer t".to_string()));
        assert_eq!(l.auth_header.as_deref(), Some("Bearer t"));
    }

    #[test]
    fn slot_view_identity() {
        use crate::app::registry::SlotMember;
        let m = ConfluenceModel::default();
        assert_eq!(m.view(), SlotView::Confluence);
        assert!(!m.shown());
        m.slot_show(None);
        assert!(m.shown());
        m.slot_park(false);
        assert!(!m.shown());
    }

    #[test]
    fn content_layout_regions() {
        let l = content_layout(1400.0, 900.0, 0.42, 210.0, 0);
        // The sidebar owns the left column; the content area starts after it.
        assert_eq!(l.sidebar.width, 210.0);
        assert_eq!(l.strip.x, 210.0);
        assert_eq!(l.strip.y, 0.0);
        assert!(l.strip.height > CONF_STRIP_ROW_HEIGHT * 2.0);
        // table -> splitter -> preview left to right.
        assert!(l.table.max_x() <= l.splitter.x);
        assert_eq!(l.splitter.x, l.table.max_x());
        assert!(l.preview.x >= l.splitter.max_x());
        assert!(l.preview.width > 0.0);
        assert!(l.splitter.height > 0.0);
        // the footer buttons sit along the bottom of the left column.
        assert!(l.cql.max_x() <= l.curl.x);
        assert!(l.more.max_x() <= l.cql.x);
        assert!(l.more.max_x() <= l.table.max_x());
        assert!(l.status.max_x() <= l.more.x);
        // preview children: header, hits bar, then the web view.
        assert!(l.p_title.y < l.p_meta.y);
        assert!(l.p_meta.y < l.hits_bar.y);
        assert!(l.hits_bar.y < l.web.y.max(0.0) || l.hits_bar.max_y() <= l.web.y);
        assert_eq!(l.web.width, l.preview.width);
        assert_eq!(l.web.x, 0.0);
        assert!(l.p_title.max_x() <= l.p_open.x);
        // a short listing fills the visible table.
        assert_eq!(l.rows_height, l.table.height);
        let tall = content_layout(1400.0, 900.0, 0.42, 210.0, 50);
        assert!(tall.rows_height >= 50.0 * CONF_RESULT_ROW_HEIGHT);
        // an out-of-range split clamps to the section default.
        let clamped = content_layout(1400.0, 900.0, 0.95, 210.0, 0);
        let default = content_layout(1400.0, 900.0, DEFAULT_SPLIT, 210.0, 0);
        assert_eq!(clamped.table.width, default.table.width);
    }

    #[test]
    fn page_colors_cover_template_keys() {
        let c = crate::ui::theme::PopupThemeDefaults::colors();
        let m = page_colors(&c);
        for k in [
            "light", "text", "text92", "dim", "accent", "mantle", "hairline", "warn", "warn35",
            "warn75", "info",
        ] {
            assert!(m.contains_key(k), "missing {k}");
        }
        assert!(m["text"].starts_with("rgba("));
        assert!(m["text"].ends_with(")"));
        assert_eq!(m["light"], "dark", "the default palette is dark");
    }

    #[test]
    fn placeholder_html_themes_and_escapes() {
        let c = crate::ui::theme::PopupThemeDefaults::colors();
        let html = placeholder_html(&c, "A & B <x>", "<p class=dim>hi</p>");
        assert!(html.contains("<!doctype html>"));
        assert!(html.contains("color-scheme: dark"));
        assert!(html.contains("<div class=\"ws-title\">"));
        assert!(html.contains("A &amp; B &lt;x&gt;"), "title is escaped");
        assert!(html.contains("<p class=dim>hi</p>"), "body is trusted markup");
        assert!(html.contains("rgba("), "colors are inlined");
    }
}
