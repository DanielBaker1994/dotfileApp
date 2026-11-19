//! Shared registries + the `SlotView`/`SlotMember` contract.
//!
//! This is the frozen hub every view plugs into (mirrors `SharedWindow.swift`'s
//! `SlotView`/`SlotMember` and `PopupChrome.navIcons`). View modules register
//! here instead of editing the host, which keeps parallel work conflict-free.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Header nav-button ids (mirrors `SharedWindow.navNotes` … in `SharedWindow.swift`).
pub mod nav {
    pub const JIRA: i64 = 61;
    pub const HOME: i64 = 62;
    pub const BACK: i64 = 63;
    pub const FILES: i64 = 64;
    pub const CONFLUENCE: i64 = 65;
    pub const AI: i64 = 66;
    pub const ALL: [i64; 6] = [JIRA, HOME, BACK, FILES, CONFLUENCE, AI];

    pub fn is_nav_id(id: i64) -> bool {
        ALL.contains(&id)
    }
}

/// Mirrors `enum SlotView: String` in `SharedWindow.swift`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum SlotView {
    Files,
    Jira,
    Detail,
    Releases,
    Config,
    Output,
    Confluence,
    Ai,
}

impl SlotView {
    pub fn raw(self) -> &'static str {
        match self {
            SlotView::Files => "files",
            SlotView::Jira => "jira",
            SlotView::Detail => "detail",
            SlotView::Releases => "releases",
            SlotView::Config => "config",
            SlotView::Output => "output",
            SlotView::Confluence => "confluence",
            SlotView::Ai => "ai",
        }
    }

    pub fn from_raw(s: &str) -> Option<SlotView> {
        Some(match s {
            "files" => SlotView::Files,
            "jira" => SlotView::Jira,
            "detail" => SlotView::Detail,
            "releases" => SlotView::Releases,
            "config" => SlotView::Config,
            "output" => SlotView::Output,
            "confluence" => SlotView::Confluence,
            "ai" => SlotView::Ai,
            _ => return None,
        })
    }

    pub fn is_jira(self) -> bool {
        matches!(
            self,
            SlotView::Jira | SlotView::Detail | SlotView::Releases | SlotView::Config
        )
    }

    pub fn is_sub(self) -> bool {
        matches!(
            self,
            SlotView::Detail | SlotView::Releases | SlotView::Config | SlotView::Output
        )
    }

    /// The header nav-button id this view lights up (mirrors `SharedWindow.navID`).
    pub fn nav_id(self) -> i64 {
        match self {
            SlotView::Files => nav::FILES,
            SlotView::Confluence => nav::CONFLUENCE,
            SlotView::Ai => nav::AI,
            v if v.is_jira() => nav::JIRA,
            _ => nav::FILES,
        }
    }

    /// The `[app]` icon setting key, if any (mirrors `parseAppConfig`).
    pub fn icon_key(self) -> Option<&'static str> {
        Some(match self {
            SlotView::Files => "files-icon",
            SlotView::Jira => "jira-icon",
            SlotView::Confluence => "confluence-icon",
            SlotView::Ai => "ai-icon",
            _ => return None,
        })
    }

    pub fn title(self) -> &'static str {
        match self {
            SlotView::Files => "Files",
            SlotView::Confluence => "Confluence",
            SlotView::Ai => "AI",
            v if v.is_jira() => "Jira",
            _ => "Output",
        }
    }
}

/// Serde-friendly rect (points).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RectI {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl RectI {
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        RectI { x, y, w, h }
    }
}

/// Mirrors `protocol SlotMember` in `SharedWindow.swift`.
pub trait SlotMember {
    fn view(&self) -> SlotView;
    fn shown(&self) -> bool;
    fn is_key(&self) -> bool {
        false
    }
    fn frame(&self) -> Option<RectI> {
        None
    }
    fn base_frame(&self) -> Option<RectI> {
        self.frame()
    }
    /// Show/unpark the view. `frame` = an explicit frame, else its base frame.
    fn slot_show(&self, frame: Option<RectI>);
    /// Park (detach + order out) the view.
    fn slot_park(&self, stop_voice: bool);
    fn test_state(&self) -> Value {
        Value::Null
    }
}

/// One entry in the header view switcher (mirrors `PopupChrome.navIcons`).
#[derive(Clone, Debug, Serialize)]
pub struct NavIcon {
    pub id: i64,
    pub view: SlotView,
    pub tip: String,
    pub image_key: Option<String>,
}

/// One palette command (mirrors the `paletteCommands` rows).
#[derive(Clone, Debug)]
pub struct PaletteCommand {
    pub id: String,
    pub title: String,
    pub section: String,
    pub in_palette: bool,
}

impl PaletteCommand {
    pub fn new(id: impl Into<String>, title: impl Into<String>, section: impl Into<String>) -> Self {
        PaletteCommand {
            id: id.into(),
            title: title.into(),
            section: section.into(),
            in_palette: true,
        }
    }
    pub fn hidden(mut self) -> Self {
        self.in_palette = false;
        self
    }
}

type TestDo = Box<dyn Fn(&str) -> Option<Value> + Send + Sync>;

/// The cross-view registry. The host owns one; view modules register into it.
#[derive(Default)]
pub struct Registry {
    nav_icons: Vec<NavIcon>,
    palette: Vec<PaletteCommand>,
    test_tables: Vec<TestDo>,
}

impl Registry {
    pub fn new() -> Self {
        Registry::default()
    }

    /// Rebuild the header switcher from the enabled views, files first then the
    /// default view (mirrors `SharedWindow.refreshNav`).
    pub fn set_nav(&mut self, entries: Vec<(SlotView, String, Option<String>)>) {
        self.nav_icons = entries
            .into_iter()
            .map(|(view, tip, image_key)| NavIcon {
                id: view.nav_id(),
                view,
                tip,
                image_key,
            })
            .collect();
    }

    pub fn nav_icons(&self) -> &[NavIcon] {
        &self.nav_icons
    }

    pub fn nav_on_id_for(&self, view: SlotView) -> Option<i64> {
        self.nav_icons
            .iter()
            .find(|n| n.view == view || (n.view.is_jira() && view.is_jira()))
            .map(|n| n.id)
    }

    pub fn add_palette(&mut self, cmd: PaletteCommand) {
        self.palette.push(cmd);
    }

    /// Replace the palette rows (the host populates them from commands.toml,
    /// mirroring `SwitcherController.paletteCommands()`).
    pub fn set_palette(&mut self, commands: Vec<PaletteCommand>) {
        self.palette = commands;
    }

    /// Palette rows for the switcher, preserving registration order.
    pub fn palette_commands(&self) -> Vec<&PaletteCommand> {
        self.palette.iter().filter(|c| c.in_palette).collect()
    }

    /// Register a `do:ACTION` table. Each is asked in turn; first `Some` wins,
    /// matching the Swift `testQuery` delegation (`screenshot:`…).
    pub fn register_test_do<F>(&mut self, f: F)
    where
        F: Fn(&str) -> Option<Value> + Send + Sync + 'static,
    {
        self.test_tables.push(Box::new(f));
    }

    pub fn dispatch_test_do(&self, action: &str) -> Option<Value> {
        for t in &self.test_tables {
            if let Some(v) = t(action) {
                return Some(v);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn nav_ids_match_swift() {
        assert_eq!(SlotView::Jira.nav_id(), 61);
        assert_eq!(SlotView::Files.nav_id(), 64);
        assert_eq!(SlotView::Confluence.nav_id(), 65);
        assert_eq!(SlotView::Ai.nav_id(), 66);
        assert_eq!(SlotView::Detail.nav_id(), 61);
        assert_eq!(SlotView::Releases.nav_id(), 61);
        assert_eq!(SlotView::Config.nav_id(), 61);
        assert!(nav::is_nav_id(62) && nav::is_nav_id(63));
        assert!(!nav::is_nav_id(59) && !nav::is_nav_id(60));
    }

    #[test]
    fn view_predicates_match_swift() {
        for v in [SlotView::Jira, SlotView::Detail, SlotView::Releases, SlotView::Config] {
            assert!(v.is_jira());
        }
        assert!(!SlotView::Files.is_jira());
        for v in [
            SlotView::Detail,
            SlotView::Releases,
            SlotView::Config,
            SlotView::Output,
        ] {
            assert!(v.is_sub(), "{v:?} should be sub");
        }
        assert!(!SlotView::Files.is_sub());
    }

    #[test]
    fn raw_round_trips() {
        for v in [
            SlotView::Files,
            SlotView::Jira,
            SlotView::Detail,
            SlotView::Releases,
            SlotView::Config,
            SlotView::Output,
            SlotView::Confluence,
            SlotView::Ai,
        ] {
            assert_eq!(SlotView::from_raw(v.raw()), Some(v));
        }
        assert_eq!(SlotView::from_raw("nope"), None);
        assert_eq!(SlotView::from_raw("notes"), None, "the notes view is gone");
        assert_eq!(SlotView::from_raw("compare"), None, "the compare view is gone");
    }

    #[test]
    fn icon_keys_match_settings() {
        assert_eq!(SlotView::Files.icon_key(), Some("files-icon"));
        assert_eq!(SlotView::Jira.icon_key(), Some("jira-icon"));
        assert_eq!(SlotView::Confluence.icon_key(), Some("confluence-icon"));
        assert_eq!(SlotView::Ai.icon_key(), Some("ai-icon"));
        assert_eq!(SlotView::Detail.icon_key(), None);
    }

    #[test]
    fn registry_nav_and_palette() {
        let mut r = Registry::new();
        r.set_nav(vec![
            (SlotView::Files, "Files".into(), None),
            (SlotView::Ai, "AI".into(), Some("ai.png".into())),
            (SlotView::Jira, "Jira".into(), None),
        ]);
        assert_eq!(r.nav_icons().len(), 3);
        assert_eq!(r.nav_on_id_for(SlotView::Detail), Some(61));
        assert_eq!(r.nav_on_id_for(SlotView::Ai), Some(66));

        r.add_palette(PaletteCommand::new("ai", "AI", "views"));
        r.add_palette(PaletteCommand::new("jira-cfg", "Jira Config", "views").hidden());
        assert_eq!(r.palette_commands().len(), 1);
    }

    #[test]
    fn test_do_first_registered_wins() {
        let mut r = Registry::new();
        r.register_test_do(|a| (a == "files:open").then(|| json!({"view": "files"})));
        r.register_test_do(|a| (a == "files:open").then(|| json!({"never": true})));
        assert_eq!(r.dispatch_test_do("files:open"), Some(json!({"view": "files"})));
        assert_eq!(r.dispatch_test_do("unknown"), None);
    }
}
