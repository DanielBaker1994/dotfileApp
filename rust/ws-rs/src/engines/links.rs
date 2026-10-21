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

/// The collapsed group: the other app's rows, its fetch status and whether
/// the user expanded it. Expansion lives for one picker open only.
#[derive(Clone, Debug, PartialEq)]
pub struct LinkGroup {
    pub rows: Vec<LinkRow>,
    pub status: LinkStatus,
    pub expanded: bool,
    pub texts: LinkTexts,
    /// False when `[links] enabled = false`: no header at all.
    pub enabled: bool,
}

impl Default for LinkGroup {
    fn default() -> Self {
        LinkGroup {
            rows: Vec::new(),
            status: LinkStatus::Loading,
            expanded: false,
            texts: LinkTexts::default(),
            enabled: true,
        }
    }
}

impl LinkGroup {
    /// A fresh open: the last good rows (stale-while-revalidate), collapsed.
    pub fn reopen(&mut self) {
        self.expanded = false;
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

    /// Indices into [`Self::rows`] to draw under the header: a query shows
    /// just its matches (auto-expanded, ranked); no query shows all rows
    /// when expanded, none when collapsed.
    pub fn visible(&self, query: &str) -> Vec<usize> {
        let q = query.trim();
        if q.is_empty() {
            return if self.expanded { (0..self.rows.len()).collect() } else { Vec::new() };
        }
        let texts: Vec<String> = self.rows.iter().map(LinkRow::search_text).collect();
        FuzzyIndex::new(&texts).ranked(q)
    }

    /// The header is always there with no query; a query hides the group
    /// when nothing in it matches.
    pub fn header_shown(&self, query: &str) -> bool {
        self.enabled && (query.trim().is_empty() || !self.visible(query).is_empty())
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

/// The full list: the local matches, then the header and its visible rows.
pub fn entries(local: &[usize], group: &LinkGroup, query: &str) -> Vec<Entry> {
    let mut out: Vec<Entry> = local.iter().map(|i| Entry::Local(*i)).collect();
    if group.header_shown(query) {
        out.push(Entry::Header);
        out.extend(group.visible(query).into_iter().map(Entry::Link));
    }
    out
}

/// §3 navigation: ↓ walks local rows → header → link rows; ↓ from the very
/// bottom wraps to the top of the local list and ↑ from the top wraps to
/// the last local row (the wrap skips the group). Returns the new position.
pub fn step(list: &[Entry], pos: usize, delta: i64) -> usize {
    if list.is_empty() {
        return 0;
    }
    let locals = list.iter().filter(|e| !e.in_group()).count();
    let pos = pos.min(list.len() - 1);
    if delta > 0 {
        if pos + 1 < list.len() {
            pos + 1
        } else {
            0
        }
    } else if delta < 0 {
        if pos > 0 {
            pos - 1
        } else if locals > 0 {
            locals - 1
        } else {
            list.len() - 1
        }
    } else {
        pos
    }
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
    fn entries_put_the_group_last() {
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
    fn step_lands_on_header_and_wrap_skips_the_group() {
        let mut g = group(2);
        let list = entries(&[0, 1], &g, "");
        assert_eq!(step(&list, 1, 1), 2, "↓ past the last local row lands on the header");
        assert_eq!(step(&list, 2, 1), 0, "↓ from the header wraps to the top");
        assert_eq!(step(&list, 0, -1), 1, "↑ from the top wraps to the last local row");
        g.expanded = true;
        let list = entries(&[0, 1], &g, "");
        assert_eq!(step(&list, 2, 1), 3, "↓ from the header enters the open group");
        assert_eq!(step(&list, 4, 1), 0);
        assert_eq!(step(&list, 0, -1), 1);
        // No local rows (a query only the group matches).
        let list = vec![Entry::Header, Entry::Link(0)];
        assert_eq!(step(&list, 0, -1), 1);
        assert_eq!(step(&[], 0, 1), 0);
    }
}
