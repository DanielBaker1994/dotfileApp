//! The link bank (`PLAN-link-bank.md`): the `links.v1` row contract shared
//! with herdr-pick, and the collapsed "other app's places" group the Ctrl+B W
//! switcher draws under its own rows. Pure model — no AppKit, no I/O.

use crate::engines::list_filter::FuzzyIndex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// The envelope version this build reads and writes.
pub const VERSION: i64 = 1;

/// One row of the contract (§1). Every field is always serialised (`null`
/// for an absent optional) so both sides read one fixed shape.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LinkRow {
    /// `herdr` | `kitchen-sink`.
    pub source: String,
    /// herdr `workspace`; kitchen-sink `view` | `note` | `folder`.
    pub kind: String,
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub subtitle: String,
    /// `working` | `waiting` | `idle` | `unknown`.
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    /// Provider rank, 0 = most recent.
    #[serde(default)]
    pub recent: i64,
    #[serde(default)]
    pub preview: Option<String>,
}

impl LinkRow {
    pub fn new(source: &str, kind: &str, id: &str, title: &str) -> Self {
        LinkRow {
            source: source.into(),
            kind: kind.into(),
            id: id.into(),
            title: title.into(),
            ..LinkRow::default()
        }
    }

    /// An agent in this place is busy or needs the user.
    pub fn active(&self) -> bool {
        matches!(self.status.as_deref(), Some("working") | Some("waiting"))
    }

    fn search_text(&self) -> String {
        format!("{} {} {}", self.title, self.subtitle, self.cwd.as_deref().unwrap_or(""))
    }
}

/// `{"version":1,"rows":[…]}`.
pub fn envelope(rows: &[LinkRow]) -> Value {
    json!({ "version": VERSION, "rows": rows })
}

/// Why a provider gave no rows.
#[derive(Clone, Debug, PartialEq)]
pub enum LinkError {
    /// The other app is not running (`"offline": true` in the error).
    Offline,
    /// Anything else: bad JSON, a timeout, an `{"error"}` reply.
    Unavailable(String),
}

/// Read a provider's reply: the envelope, or `{"error":…[,"offline":true]}`.
pub fn parse_envelope(text: &str) -> Result<Vec<LinkRow>, LinkError> {
    let v: Value = serde_json::from_str(text.trim())
        .map_err(|e| LinkError::Unavailable(format!("bad links reply: {e}")))?;
    if let Some(err) = v.get("error") {
        if v.get("offline").and_then(Value::as_bool) == Some(true) {
            return Err(LinkError::Offline);
        }
        return Err(LinkError::Unavailable(err.as_str().unwrap_or("error").to_string()));
    }
    let version = v.get("version").and_then(Value::as_i64).unwrap_or(0);
    if version != VERSION {
        return Err(LinkError::Unavailable(format!("links version {version} (want {VERSION})")));
    }
    serde_json::from_value(v.get("rows").cloned().unwrap_or_else(|| json!([])))
        .map_err(|e| LinkError::Unavailable(format!("bad links rows: {e}")))
}

/// §3 "Order inside": the provider's most recent place first, places with a
/// working / waiting agent next, then the rest by `recent`.
pub fn order_rows(mut rows: Vec<LinkRow>) -> Vec<LinkRow> {
    rows.sort_by_key(|r| (r.recent != 0, !r.active(), r.recent));
    rows
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkStatus {
    /// Fetching in the background (rows may be the last good, stale set).
    Loading,
    Ready,
    Offline,
    Unavailable,
}

impl LinkStatus {
    pub fn raw(self) -> &'static str {
        match self {
            LinkStatus::Loading => "loading",
            LinkStatus::Ready => "ready",
            LinkStatus::Offline => "offline",
            LinkStatus::Unavailable => "unavailable",
        }
    }

    /// The header draws dim (and Return launches, when offline).
    pub fn dim(self) -> bool {
        matches!(self, LinkStatus::Offline | LinkStatus::Unavailable)
    }
}

/// Header strings (`[links]` in commands.toml).
#[derive(Clone, Debug, PartialEq)]
pub struct LinkTexts {
    pub label: String,
    pub loading: String,
    pub offline: String,
    pub unavailable: String,
}

impl Default for LinkTexts {
    fn default() -> Self {
        LinkTexts {
            label: "Terminal spaces".into(),
            loading: "…".into(),
            offline: "not running".into(),
            unavailable: "unavailable".into(),
        }
    }
}

/// How the group opens and what searches it (`[links]` in commands.toml).
#[derive(Clone, Debug, PartialEq)]
pub struct LinkOpts {
    /// A fresh open puts the cursor on the header, so Return switches apps.
    pub start_on_group: bool,
    /// A fresh open (and a cleared query) lists the rows under the header.
    pub start_expanded: bool,
    /// Words that search the group itself: a query whose first word is one
    /// of them (or a 3+ letter prefix) keeps the header and every row, and
    /// the rest of the query narrows the rows (`herdr dot`).
    pub names: Vec<String>,
}

impl Default for LinkOpts {
    fn default() -> Self {
        LinkOpts {
            start_on_group: true,
            start_expanded: true,
            names: ["terminal", "herdr", "ghostty"].map(String::from).to_vec(),
        }
    }
}

/// The group: the other app's rows, its fetch status and whether it is
/// expanded. A user's expand / collapse lives for one picker open only.
#[derive(Clone, Debug, PartialEq)]
pub struct LinkGroup {
    pub rows: Vec<LinkRow>,
    pub status: LinkStatus,
    pub expanded: bool,
    pub texts: LinkTexts,
    /// False when `[links] enabled = false`: no header at all.
    pub enabled: bool,
    pub opts: LinkOpts,
}

impl Default for LinkGroup {
    fn default() -> Self {
        LinkGroup {
            rows: Vec::new(),
            status: LinkStatus::Loading,
            expanded: false,
            texts: LinkTexts::default(),
            enabled: true,
            opts: LinkOpts::default(),
        }
    }
}

impl LinkGroup {
    /// A fresh open: the last good rows (stale-while-revalidate), expanded
    /// or collapsed per `start_expanded`.
    pub fn reopen(&mut self) {
        self.expanded = self.opts.start_expanded;
        self.status = LinkStatus::Loading;
    }

    /// A provider reply arrived. A failure keeps the last good rows only
    /// while loading; offline / unavailable show none.
    pub fn apply(&mut self, result: Result<Vec<LinkRow>, LinkError>) {
        match result {
            Ok(rows) => {
                self.rows = order_rows(rows);
                self.status = LinkStatus::Ready;
            }
            Err(LinkError::Offline) => {
                self.rows.clear();
                self.status = LinkStatus::Offline;
            }
            Err(LinkError::Unavailable(_)) => {
                self.rows.clear();
                self.status = LinkStatus::Unavailable;
            }
        }
    }

    /// `Some(rest)` when the query's first word names the group (see
    /// [`LinkOpts::names`]): `herdr dot` → `dot`, `terminal` → ``.
    pub fn scope<'a>(&self, query: &'a str) -> Option<&'a str> {
        let q = query.trim_start();
        let (first, rest) = q.split_once(char::is_whitespace).unwrap_or((q, ""));
        let first = first.to_lowercase();
        let hit = !first.is_empty()
            && self.opts.names.iter().any(|n| {
                let n = n.to_lowercase();
                n == first || (first.chars().count() >= 3 && n.starts_with(&first))
            });
        hit.then(|| rest.trim())
    }

    /// Indices into [`Self::rows`] to draw under the header: a query shows
    /// just its matches (auto-expanded, ranked), a query naming the group
    /// shows every row (or the matches of the words after the name); no
    /// query shows all rows when expanded, none when collapsed.
    pub fn visible(&self, query: &str) -> Vec<usize> {
        let q = query.trim();
        if q.is_empty() {
            return if self.expanded { (0..self.rows.len()).collect() } else { Vec::new() };
        }
        let q = self.scope(q).unwrap_or(q);
        if q.is_empty() {
            return (0..self.rows.len()).collect();
        }
        let texts: Vec<String> = self.rows.iter().map(LinkRow::search_text).collect();
        FuzzyIndex::new(&texts).ranked(q)
    }

    /// The header is always there with no query or a query naming the
    /// group; any other query hides it when nothing in it matches.
    pub fn header_shown(&self, query: &str) -> bool {
        self.enabled
            && (query.trim().is_empty() || self.scope(query).is_some() || !self.visible(query).is_empty())
    }

    /// Open with no query and expanded, or opened by a query.
    pub fn open(&self, query: &str) -> bool {
        self.expanded || !query.trim().is_empty()
    }

    /// `Terminal spaces · 6` / `· …` / `· not running` / `· unavailable`.
    pub fn header_text(&self) -> String {
        let t = &self.texts;
        let tail = match self.status {
            LinkStatus::Offline => t.offline.clone(),
            LinkStatus::Unavailable => t.unavailable.clone(),
            LinkStatus::Loading if self.rows.is_empty() => t.loading.clone(),
            _ => self.rows.len().to_string(),
        };
        format!("{} · {tail}", t.label)
    }

    /// `state.viewSwitcher.linkGroup` (the ui-check probe).
    pub fn test_state(&self, query: &str) -> Value {
        let visible: Vec<&str> = self.visible(query).into_iter().map(|i| self.rows[i].id.as_str()).collect();
        json!({
            "enabled": self.enabled,
            "expanded": self.open(query),
            "status": self.status.raw(),
            "header": self.header_text(),
            "rows": self.rows.iter().map(|r| json!({
                "id": r.id,
                "title": r.title,
                "subtitle": r.subtitle,
                "status": r.status,
            })).collect::<Vec<_>>(),
            "visible": visible,
        })
    }
}

/// One entry of a picker list that carries a link group: a local row (an
/// index into the picker's own rows), the group header, or a link row (an
/// index into [`LinkGroup::rows`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    Local(usize),
    Header,
    Link(usize),
}

impl Entry {
    pub fn in_group(self) -> bool {
        !matches!(self, Entry::Local(_))
    }
}

/// The full list: the group pinned on top (its header, then its visible
/// rows), then the local matches.
pub fn entries(local: &[usize], group: &LinkGroup, query: &str) -> Vec<Entry> {
    // The views first, the group below them: a long list of places reads
    // from the bottom without pushing the views out of sight.
    let mut out: Vec<Entry> = local.iter().map(|i| Entry::Local(*i)).collect();
    if group.header_shown(query) {
        out.push(Entry::Header);
        out.extend(group.visible(query).into_iter().map(Entry::Link));
    }
    out
}

/// Where the cursor starts: no query → the header (when `start_on_group`)
/// else the first local row; a query naming the group → the header
/// (`terminal`, Return switches apps) or its best row (`herdr dot`); any
/// other query → the best local match, else the best group match.
pub fn start_pos(list: &[Entry], group: &LinkGroup, query: &str) -> usize {
    let find = |f: fn(&Entry) -> bool| list.iter().position(f);
    if let (Some(rest), Some(h)) = (group.scope(query), find(|e| *e == Entry::Header)) {
        return if rest.is_empty() { h } else { find(|e| matches!(e, Entry::Link(_))).unwrap_or(h) };
    }
    if query.trim().is_empty() {
        if group.opts.start_on_group {
            if let Some(i) = find(|e| *e == Entry::Header) {
                return i;
            }
        }
        return find(|e| matches!(e, Entry::Local(_))).unwrap_or(0);
    }
    find(|e| matches!(e, Entry::Local(_)))
        .or_else(|| find(|e| matches!(e, Entry::Link(_))))
        .unwrap_or(0)
}

/// ↑ / ↓ through the whole list, wrapping at both ends.
pub fn step(list: &[Entry], pos: usize, delta: i64) -> usize {
    if list.is_empty() {
        return 0;
    }
    let n = list.len() as i64;
    (pos.min(list.len() - 1) as i64 + delta.signum()).rem_euclid(n) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    const KS_FIXTURE: &str = include_str!("../../../../Tests/fixtures/links/kitchen-sink.json");
    const HERDR_FIXTURE: &str = include_str!("../../../../Tests/fixtures/links/herdr.json");

    fn row(id: &str, recent: i64, status: Option<&str>) -> LinkRow {
        LinkRow {
            status: status.map(str::to_string),
            recent,
            ..LinkRow::new("herdr", "workspace", id, id)
        }
    }

    fn group(n: usize) -> LinkGroup {
        let mut g = LinkGroup::default();
        g.apply(Ok((0..n).map(|i| row(&format!("w{i}"), i as i64, None)).collect()));
        g
    }

    #[test]
    fn fixtures_round_trip() {
        for text in [KS_FIXTURE, HERDR_FIXTURE] {
            let rows = parse_envelope(text).expect("fixture parses");
            assert!(!rows.is_empty());
            let again = parse_envelope(&envelope(&rows).to_string()).unwrap();
            assert_eq!(rows, again);
        }
        let herdr = parse_envelope(HERDR_FIXTURE).unwrap();
        assert_eq!(herdr[0].source, "herdr");
        assert_eq!(herdr[0].kind, "workspace");
        let ks = parse_envelope(KS_FIXTURE).unwrap();
        assert!(ks.iter().any(|r| r.kind == "view"));
        assert!(ks.iter().any(|r| r.kind == "note"));
        assert!(ks.iter().any(|r| r.kind == "folder"));
    }

    #[test]
    fn envelope_always_carries_every_field() {
        let v = envelope(&[LinkRow::new("kitchen-sink", "view", "view:notes", "Notes")]);
        let r = &v["rows"][0];
        for k in ["source", "kind", "id", "title", "subtitle", "status", "cwd", "recent", "preview"] {
            assert!(r.get(k).is_some(), "missing {k}");
        }
        assert_eq!(v["version"], json!(1));
    }

    #[test]
    fn errors_split_offline_from_unavailable() {
        assert_eq!(parse_envelope(r#"{"error":"down","offline":true}"#), Err(LinkError::Offline));
        assert!(matches!(parse_envelope(r#"{"error":"boom"}"#), Err(LinkError::Unavailable(_))));
        assert!(matches!(parse_envelope("not json"), Err(LinkError::Unavailable(_))));
        assert!(matches!(parse_envelope(r#"{"version":2,"rows":[]}"#), Err(LinkError::Unavailable(_))));
    }

    #[test]
    fn order_puts_recent_then_busy_agents_first() {
        let rows = order_rows(vec![
            row("idle3", 3, Some("idle")),
            row("work4", 4, Some("working")),
            row("first", 0, None),
            row("wait5", 5, Some("waiting")),
            row("two", 2, None),
        ]);
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["first", "work4", "wait5", "two", "idle3"]);
    }

    #[test]
    fn header_states() {
        let mut g = LinkGroup::default();
        assert_eq!(g.header_text(), "Terminal spaces · …");
        g.apply(Ok(vec![row("a", 0, None), row("b", 1, None)]));
        assert_eq!(g.header_text(), "Terminal spaces · 2");
        // Reopening keeps the stale rows while the refresh runs.
        g.reopen();
        assert_eq!(g.header_text(), "Terminal spaces · 2");
        g.apply(Err(LinkError::Offline));
        assert_eq!(g.header_text(), "Terminal spaces · not running");
        assert!(g.status.dim());
        g.apply(Err(LinkError::Unavailable("timeout".into())));
        assert_eq!(g.header_text(), "Terminal spaces · unavailable");
    }

    #[test]
    fn collapsed_by_default_query_auto_expands_to_matches() {
        let mut g = group(3);
        g.opts.start_expanded = false;
        g.rows[1].title = "kitchen-sink".into();
        assert!(g.visible("").is_empty());
        assert!(!g.open(""));
        assert_eq!(g.visible("kitchen"), vec![1]);
        assert!(g.open("kitchen"));
        assert!(g.header_shown("kitchen"));
        assert!(!g.header_shown("zzzz"));
        g.expanded = true;
        assert_eq!(g.visible(""), vec![0, 1, 2]);
        g.reopen();
        assert!(!g.expanded);
    }

    #[test]
    fn expanded_by_default_and_its_names_keep_the_group() {
        let mut g = group(3);
        g.rows[1].title = "dotfiles".into();
        g.reopen();
        assert_eq!(g.visible(""), vec![0, 1, 2], "start_expanded: open on a fresh show");
        for q in ["terminal", "herdr", "Ghost", "ter"] {
            assert_eq!(g.scope(q), Some(""), "{q}");
            assert!(g.header_shown(q));
            assert_eq!(g.visible(q), vec![0, 1, 2]);
        }
        assert_eq!(g.scope("he"), None, "two letters is not a name");
        assert_eq!(g.visible("herdr dot"), vec![1]);
        assert!(g.header_shown("herdr zzzz"), "the name alone keeps the header");
        let list = entries(&[0], &g, "terminal");
        assert_eq!(start_pos(&list, &g, "terminal"), 1, "the header: Return raises the terminal");
        let list = entries(&[], &g, "herdr dot");
        assert_eq!(list, vec![Entry::Header, Entry::Link(1)]);
        assert_eq!(start_pos(&list, &g, "herdr dot"), 1);
    }

    #[test]
    fn entries_list_the_group_below_the_views() {
        let mut g = group(2);
        assert_eq!(entries(&[0, 1], &g, ""), vec![Entry::Local(0), Entry::Local(1), Entry::Header]);
        g.expanded = true;
        assert_eq!(
            entries(&[0], &g, ""),
            vec![Entry::Local(0), Entry::Header, Entry::Link(0), Entry::Link(1)]
        );
        g.enabled = false;
        assert_eq!(entries(&[0], &g, ""), vec![Entry::Local(0)]);
    }

    #[test]
    fn cursor_starts_on_the_header_and_a_query_prefers_local_matches() {
        let mut g = group(2);
        let list = entries(&[0, 1], &g, "");
        assert_eq!(start_pos(&list, &g, ""), 2, "the header: Return switches apps");
        g.opts.start_on_group = false;
        assert_eq!(start_pos(&list, &g, ""), 0);
        g.rows[1].title = "notes-ish".into();
        let list = entries(&[0], &g, "notes");
        assert_eq!(list, vec![Entry::Local(0), Entry::Header, Entry::Link(1)]);
        assert_eq!(start_pos(&list, &g, "notes"), 0, "the local match first");
        let list = entries(&[], &g, "notes");
        assert_eq!(start_pos(&list, &g, "notes"), 1, "else the group match, not the header");
    }

    #[test]
    fn step_wraps_through_the_whole_list() {
        let g = group(2);
        let list = entries(&[0, 1], &g, "");
        assert_eq!(step(&list, 0, 1), 1);
        assert_eq!(step(&list, 2, 1), 0);
        assert_eq!(step(&list, 0, -1), 2);
        assert_eq!(step(&[], 0, 1), 0);
    }
}
