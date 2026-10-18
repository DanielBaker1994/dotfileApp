//! The Cmd+/ keyboard-shortcuts sheet (`ShortcutsOverlay` +
//! `ShortcutsListView`, PopupWindow.swift): a dimmed backdrop over the
//! window, a centered card titled "Keyboard Shortcuts" with key caps on the
//! left and what they do on the right, grouped by view. Rows come from the
//! `[shortcuts]` section of commands.toml (`"view: keys" = "what"`).
//!
//! The sheet is a human-maintained description, not the source of truth for
//! the bindings: nothing checks it against the keys that actually respond. The
//! bindings live elsewhere — the in-window key router (`route_key_event` /
//! `ui::popup::route_key`), the global hotkey modes (`hotkey::HOTKEY_MODES`,
//! launched via `bin/kitchen_sink.sh`), and the Hyper+KEY bindings in
//! `config/aerospace/aerospace.toml` — and are not derived from these rows.

/// One `[shortcuts]` row: `"view: keys" = "what"`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShortcutEntry {
    pub view: String,
    pub keys: String,
    pub what: String,
}

/// A titled group of `(keys, what)` rows (`ShortcutRows`).
pub type ShortcutGroup = (String, Vec<(String, String)>);

/// Read one TOML basic/literal string at the start of `s`; returns the value
/// and the rest after the closing quote.
fn read_string(s: &str) -> Option<(String, &str)> {
    let s = s.trim_start();
    let quote = s.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let mut out = String::new();
    let mut chars = s[1..].char_indices();
    while let Some((i, c)) = chars.next() {
        if c == quote {
            return Some((out, &s[1 + i + 1..]));
        }
        if c == '\\' && quote == '"' {
            match chars.next() {
                Some((_, 'n')) => out.push('\n'),
                Some((_, 't')) => out.push('\t'),
                Some((_, e)) => out.push(e),
                None => return None,
            }
        } else {
            out.push(c);
        }
    }
    None
}

/// The `[shortcuts]` rows of a commands.toml text, in file order.
pub fn parse_entries(text: &str) -> Vec<ShortcutEntry> {
    let mut out = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            inside = t == "[shortcuts]";
            continue;
        }
        if !inside || t.is_empty() || t.starts_with('#') {
            continue;
        }
        let Some((key, rest)) = read_string(t) else { continue };
        let Some(rest) = rest.trim_start().strip_prefix('=') else { continue };
        let Some((what, _)) = read_string(rest) else { continue };
        let Some((view, keys)) = key.split_once(':') else { continue };
        out.push(ShortcutEntry {
            view: view.trim().to_string(),
            keys: keys.trim().to_string(),
            what,
        });
    }
    out
}

fn items(entries: &[ShortcutEntry], view: &str) -> Vec<(String, String)> {
    entries
        .iter()
        .filter(|e| e.view == view)
        .map(|e| (e.keys.clone(), e.what.clone()))
        .collect()
}

/// `sharedShortcutGroups()` — Sidebars, Previews & Pages, Everywhere.
pub fn shared_groups(entries: &[ShortcutEntry]) -> Vec<ShortcutGroup> {
    [("Sidebars", "sidebar"), ("Previews & Pages", "preview"), ("Everywhere", "all")]
        .into_iter()
        .map(|(t, v)| (t.to_string(), items(entries, v)))
        .filter(|g| !g.1.is_empty())
        .collect()
}

/// `showShortcuts(on:view:)` per view: the view's own rows first (compare
/// lists both Text and Folder Compare), then the shared groups.
pub fn groups_for(view: &str, entries: &[ShortcutEntry]) -> Vec<ShortcutGroup> {
    let mut out: Vec<ShortcutGroup> = Vec::new();
    let own: &[(&str, &str)] = match view {
        "notes" => &[("Notes", "notes")],
        "files" => &[("Files", "files")],
        "jira" => &[("Jira", "jira")],
        "confluence" => &[("Confluence", "confluence")],
        "ai" => &[("AI", "ai")],
        "compare" => &[("Text Compare", "compare"), ("Folder Compare", "compare-folders")],
        "palette" => &[("Palette", "palette")],
        "paths" => &[("Paths", "paths")],
        "screenshot" => &[("Screenshot", "screenshot")],
        _ => &[],
    };
    for (title, v) in own {
        let rows = items(entries, v);
        if !rows.is_empty() {
            out.push((title.to_string(), rows));
        }
    }
    out.extend(shared_groups(entries));
    out
}

/// `ShortcutsOverlay.handleKey` — what a key does while the sheet is up.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SheetKey {
    Close,
    Scroll(f64),
    /// Swallowed (the sheet owns the keyboard).
    Ignore,
}

pub fn sheet_key(code: u16, ctrl: bool, cmd: bool) -> SheetKey {
    match (code, ctrl) {
        (53, _) | (36, _) | (76, _) | (49, false) | (12, false) => SheetKey::Close,
        (44, _) if cmd => SheetKey::Close,
        (125, _) | (45, true) | (38, true) => SheetKey::Scroll(60.0),
        (126, _) | (35, true) | (40, true) => SheetKey::Scroll(-60.0),
        _ => SheetKey::Ignore,
    }
}

pub const ROW_H: f64 = 27.0;
pub const HEAD_H: f64 = 30.0;
pub const TITLE_H: f64 = 40.0;

/// `ShortcutsListView.contentHeight`.
pub fn content_height(groups: &[ShortcutGroup]) -> f64 {
    groups.iter().fold(6.0, |h, g| h + HEAD_H + g.1.len() as f64 * ROW_H) + 8.0
}

#[cfg(target_os = "macos")]
pub use appkit::ShortcutsOverlay;

#[cfg(target_os = "macos")]
mod appkit {
    use super::*;
    use crate::ui::chrome::Rect;
    use crate::ui::style::{self, Weight};
    use crate::ui::theme::{PopupColors, Rgba};
    use objc2::rc::Retained;
    use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSAutoresizingMaskOptions, NSBezierPath, NSEvent, NSScrollView, NSTextField, NSView};
    use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};
    use std::cell::RefCell;

    pub struct ListIvars {
        groups: Vec<ShortcutGroup>,
        colors: PopupColors,
    }

    define_class!(
        #[unsafe(super(crate::ui::view_base::WSFlippedView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSShortcutsList"]
        #[ivars = ListIvars]
        struct ShortcutsList;

        impl ShortcutsList {
            #[unsafe(method(drawRect:))]
            fn draw_rect(&self, _d: NSRect) {
                self.draw();
            }
        }

        unsafe impl NSObjectProtocol for ShortcutsList {}
    );

    impl ShortcutsList {
        fn caps_width(keys: &str) -> f64 {
            let cap = style::mono_font(11.0, Weight::Medium);
            let slash = style::text_size("/", &cap).0 + 8.0;
            let parts: Vec<&str> = keys.split(" / ").collect();
            parts.iter().map(|p| style::text_size(p, &cap).0 + 12.0).sum::<f64>()
                + parts.len().saturating_sub(1) as f64 * slash
        }

        fn draw(&self) {
            let g = &self.ivars().groups;
            let c = &self.ivars().colors;
            let w = self.bounds().size.width;
            let pad = 14.0;
            let widest = g.iter().flat_map(|g| g.1.iter()).map(|(k, _)| Self::caps_width(k)).fold(0.0, f64::max);
            let key_col = (w * 0.5).min(widest) + pad + 16.0;
            let head = style::system_font(10.0, Weight::Bold);
            let text = style::system_font(12.5, Weight::Regular);
            let cap_font = style::mono_font(11.0, Weight::Medium);
            let mut y = 6.0;
            for (title, rows) in g {
                style::draw_text(&title.to_uppercase(), pad, y + HEAD_H - 18.0, &head, c.accent_on());
                y += HEAD_H;
                for (keys, what) in rows {
                    let mut x = pad;
                    let cap_h = 19.0;
                    let cap_y = y + (ROW_H - cap_h) / 2.0;
                    for (i, part) in keys.split(" / ").enumerate() {
                        if i > 0 {
                            let (sw, sh) = style::text_size("/", &cap_font);
                            style::draw_text("/", x + 4.0, cap_y + (cap_h - sh) / 2.0, &cap_font, c.dim);
                            x += sw + 8.0;
                        }
                        let (pw, ph) = style::text_size(part, &cap_font);
                        let cap = NSRect::new(NSPoint::new(x, cap_y), NSSize::new(pw + 12.0, cap_h));
                        let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(cap, 5.0, 5.0);
                        c.mantle().to_nscolor().setFill();
                        path.fill();
                        c.hairline().to_nscolor().setStroke();
                        path.setLineWidth(1.0);
                        path.stroke();
                        style::draw_text(part, x + 6.0, cap_y + (cap_h - ph) / 2.0, &cap_font, c.text);
                        x += pw + 12.0;
                    }
                    let tx = key_col.max(x + 12.0);
                    style::draw_text_left(what, Rect::new(tx, y, (w - tx - pad).max(0.0), ROW_H), &text, c.text);
                    y += ROW_H;
                }
            }
        }
    }

    pub struct OverlayIvars {
        scroll: RefCell<Option<Retained<NSScrollView>>>,
        on_close: RefCell<Option<Box<dyn Fn()>>>,
    }

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSShortcutsOverlay"]
        #[ivars = OverlayIvars]
        pub struct ShortcutsOverlay;

        impl ShortcutsOverlay {
            /// A click outside the card closes the sheet.
            #[unsafe(method(mouseDown:))]
            fn mouse_down(&self, _e: &NSEvent) {
                self.close();
            }
        }

        unsafe impl NSObjectProtocol for ShortcutsOverlay {}
    );

    pub struct CardIvars;

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSShortcutsCard"]
        #[ivars = CardIvars]
        struct Card;

        impl Card {
            /// Clicks on the card don't close it.
            #[unsafe(method(mouseDown:))]
            fn mouse_down(&self, _e: &NSEvent) {}
        }

        unsafe impl NSObjectProtocol for Card {}
    );

    impl ShortcutsOverlay {
        /// Build the sheet over `root` (added as its top subview).
        pub fn show(
            mtm: MainThreadMarker,
            root: &NSView,
            groups: Vec<ShortcutGroup>,
            colors: PopupColors,
            title: &str,
            on_close: Box<dyn Fn()>,
        ) -> Retained<ShortcutsOverlay> {
            let rb = root.bounds();
            let this = Self::alloc(mtm).set_ivars(OverlayIvars {
                scroll: RefCell::new(None),
                on_close: RefCell::new(Some(on_close)),
            });
            let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: rb] };
            this.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable);
            this.setWantsLayer(true);
            if let Some(l) = this.layer() {
                l.setBackgroundColor(Some(&Rgba::new(0.0, 0.0, 0.0, 0.25).to_nscolor().CGColor()));
                // Above the panes' focus-ring / border layers, which otherwise
                // drew through the sheet.
                l.setZPosition(1000.0);
            }

            let w = 640f64.min(rb.size.width - 40.0);
            let list_h = content_height(&groups);
            let h = (TITLE_H + list_h).min(rb.size.height - 60.0);
            let card = Card::alloc(mtm).set_ivars(CardIvars);
            let card: Retained<Card> = unsafe {
                msg_send![super(card), initWithFrame: NSRect::new(
                    NSPoint::new(((rb.size.width - w) / 2.0).round(), ((rb.size.height - h) / 2.0).round()),
                    NSSize::new(w, h),
                )]
            };
            card.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewMinXMargin
                    | NSAutoresizingMaskOptions::ViewMaxXMargin
                    | NSAutoresizingMaskOptions::ViewMinYMargin
                    | NSAutoresizingMaskOptions::ViewMaxYMargin,
            );
            card.setWantsLayer(true);
            if let Some(l) = card.layer() {
                let fill = colors.background.opaque().blended(0.25, Rgba::BLACK).with_alpha(1.0);
                l.setBackgroundColor(Some(&fill.to_nscolor().CGColor()));
                l.setCornerRadius(12.0);
                l.setBorderColor(Some(&colors.text.with_alpha(0.15).to_nscolor().CGColor()));
                l.setBorderWidth(1.0);
                l.setShadowOpacity(0.4);
                l.setShadowRadius(18.0);
            }
            this.addSubview(&card);

            // Title row: the card is unflipped, so the title sits at the top.
            let t = NSTextField::labelWithString(&NSString::from_str(title), mtm);
            t.setFont(Some(&style::system_font(14.0, Weight::Semibold)));
            t.setTextColor(Some(&colors.text.to_nscolor()));
            t.sizeToFit();
            let tf = t.frame().size;
            t.setFrameOrigin(NSPoint::new(14.0, h - TITLE_H / 2.0 - tf.height / 2.0));
            card.addSubview(&t);
            let hint = NSTextField::labelWithString(&NSString::from_str("esc to close"), mtm);
            hint.setFont(Some(&style::system_font(11.0, Weight::Regular)));
            hint.setTextColor(Some(&colors.dim.to_nscolor()));
            hint.sizeToFit();
            let hf = hint.frame().size;
            hint.setFrameOrigin(NSPoint::new(w - hf.width - 14.0, h - TITLE_H / 2.0 - hf.height / 2.0));
            card.addSubview(&hint);
            let rule = NSView::initWithFrame(NSView::alloc(mtm), NSRect::new(NSPoint::new(0.0, h - TITLE_H), NSSize::new(w, 1.0)));
            rule.setWantsLayer(true);
            if let Some(l) = rule.layer() {
                l.setBackgroundColor(Some(&colors.hairline().to_nscolor().CGColor()));
            }
            card.addSubview(&rule);

            let list = ShortcutsList::alloc(mtm).set_ivars(ListIvars { groups, colors });
            let list: Retained<ShortcutsList> = unsafe {
                msg_send![super(list), initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w, list_h))]
            };
            let sv = NSScrollView::initWithFrame(
                NSScrollView::alloc(mtm),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w, h - TITLE_H)),
            );
            sv.setDrawsBackground(false);
            sv.setHasVerticalScroller(true);
            sv.setAutohidesScrollers(true);
            sv.setDocumentView(Some(&list));
            card.addSubview(&sv);
            list.scrollPoint(NSPoint::new(0.0, 0.0));
            *this.ivars().scroll.borrow_mut() = Some(sv);
            root.addSubview(&this);
            this
        }

        pub fn close(&self) {
            self.removeFromSuperview();
            let cb = self.ivars().on_close.borrow_mut().take();
            if let Some(cb) = cb {
                cb();
            }
        }

        /// `handleKey` — Esc / Return / Space / Q / Cmd+/ close; arrows,
        /// Ctrl+N/P and Ctrl+J/K scroll; everything else is swallowed.
        pub fn handle_key(&self, code: u16, ctrl: bool, cmd: bool) -> bool {
            match sheet_key(code, ctrl, cmd) {
                SheetKey::Close => self.close(),
                SheetKey::Scroll(dy) => {
                    if let Some(sv) = self.ivars().scroll.borrow().as_ref() {
                        let clip = sv.contentView();
                        let doc_h = sv.documentView().map(|d| d.frame().size.height).unwrap_or(0.0);
                        let b = clip.bounds();
                        let max_y = (doc_h - b.size.height).max(0.0);
                        let y = (b.origin.y + dy).clamp(0.0, max_y);
                        clip.scrollToPoint(NSPoint::new(0.0, y));
                        sv.reflectScrolledClipView(&clip);
                    }
                }
                SheetKey::Ignore => {}
            }
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONF: &str = r#"
[app]
x = 1
[shortcuts]
"all: Cmd+\\" = "collapse the sidebar"
"notes: Cmd+S" = "save the note"
# a comment
"sidebar: Return" = "open the row"
"compare: Cmd+0" = "Home"
"compare-folders: F5" = "scan again"
"all: Ctrl+B W" = "prefix: view switcher — \"where\""
[prettyprint]
"notes: nope" = "not in shortcuts"
"#;

    #[test]
    fn parses_shortcut_rows_in_order() {
        let e = parse_entries(CONF);
        assert_eq!(e.len(), 6);
        assert_eq!(e[0], ShortcutEntry { view: "all".into(), keys: "Cmd+\\".into(), what: "collapse the sidebar".into() });
        assert_eq!(e[5].what, "prefix: view switcher — \"where\"");
    }

    #[test]
    fn groups_view_first_then_shared() {
        let e = parse_entries(CONF);
        let g = groups_for("notes", &e);
        let titles: Vec<&str> = g.iter().map(|g| g.0.as_str()).collect();
        assert_eq!(titles, ["Notes", "Sidebars", "Everywhere"]);
        let c = groups_for("compare", &e);
        assert_eq!(c[0].0, "Text Compare");
        assert_eq!(c[1].0, "Folder Compare");
        assert_eq!(groups_for("jira", &e)[0].0, "Sidebars");
    }

    #[test]
    fn sheet_keys() {
        assert_eq!(sheet_key(53, false, false), SheetKey::Close);
        assert_eq!(sheet_key(44, false, true), SheetKey::Close);
        assert_eq!(sheet_key(125, false, false), SheetKey::Scroll(60.0));
        assert_eq!(sheet_key(35, true, false), SheetKey::Scroll(-60.0));
        assert_eq!(sheet_key(0, false, false), SheetKey::Ignore);
    }

    /// `(keys, what)` pairs documented under the shared `all:` group *and*
    /// under a specific view — a stale copy of a shared row. Two specific
    /// views may legitimately document the same pair.
    fn all_group_leaks(entries: &[ShortcutEntry]) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for e in entries.iter().filter(|e| e.view == "all") {
            if entries.iter().any(|p| p.view != "all" && p.keys == e.keys && p.what == e.what) {
                out.push((e.keys.clone(), e.what.clone()));
            }
        }
        out
    }

    #[test]
    fn all_group_pair_is_not_repeated_under_a_view() {
        // A well-formed sheet keeps the shared `all:` rows out of the views.
        assert!(all_group_leaks(&parse_entries(CONF)).is_empty());

        // The real regression: the Cmd+/ row copied out of `all:` into a view.
        let leaked = r#"
[shortcuts]
"all: Cmd+/" = "this list"
"ai: Cmd+/" = "this list"
"#;
        assert_eq!(
            all_group_leaks(&parse_entries(leaked)),
            vec![("Cmd+/".to_string(), "this list".to_string())]
        );

        // Two specific views may legitimately share a row (e.g. files + jira).
        let shared_by_views = r#"
[shortcuts]
"files: Home / End / PgUp / PgDn" = "jump in the list"
"jira: Home / End / PgUp / PgDn" = "jump in the list"
"#;
        assert!(all_group_leaks(&parse_entries(shared_by_views)).is_empty());
    }

    #[test]
    fn real_config_has_every_view() {
        let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../commands.toml")).unwrap_or_default();
        if text.is_empty() {
            return;
        }
        let e = parse_entries(&text);
        for v in ["all", "notes", "files", "jira", "compare", "confluence", "ai", "paths", "screenshot", "palette", "sidebar"] {
            assert!(e.iter().any(|x| x.view == v), "no [shortcuts] rows for {v}");
        }
        // A shared `all:` row must not be copied into a view (guards against
        // re-adding "ai: Cmd+/" = "this list").
        let leaks = all_group_leaks(&e);
        assert!(leaks.is_empty(), "shared `all:` rows copied into a view: {leaks:?}");
    }
}
