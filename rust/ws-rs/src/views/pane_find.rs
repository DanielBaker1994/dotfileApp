//! Small floating pickers shared by the window: the Jira rail's jump list
//! (`do:jira-jump`, `SidebarJumpWindow`) and the `/` pane finder the vim keys
//! open over list panes. Both are [`ListPanel`] cards like the Ctrl+B W
//! switcher. (Split out of the removed notes find window.)

#![cfg(target_os = "macos")]

use crate::engines::list_filter::FuzzyIndex;
use crate::ui::list_panel::ListPanel;
use crate::ui::popup::{CardFrame, PopupConfig, PopupPanel};
use crate::ui::style::{self, Weight};
use crate::ui::theme::PopupColors;
use objc2::rc::Retained;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSEvent, NSScreen, NSTextField};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

const ROW_H: f64 = 30.0;

/// `[pane-find]` settings.
#[derive(Clone, Debug)]
pub struct PaneFindConfig {
    pub width: f64,
    pub rows: usize,
}

impl PaneFindConfig {
    pub fn from_section(s: &HashMap<String, String>) -> Self {
        let num = |k: &str| s.get(k).and_then(|v| v.trim().parse::<f64>().ok());
        PaneFindConfig {
            width: num("width").unwrap_or(640.0).max(360.0),
            rows: num("rows").map(|v| v as usize).unwrap_or(12).max(5),
        }
    }
}

// ------------------------------------------------------------- sidebar jump

/// One `PopupWindow.SidebarJumpItem`: a sidebar entry (`row` indexes the
/// rail's pinned-then-list order).
#[derive(Clone, Debug, PartialEq)]
pub struct JumpItem {
    pub title: String,
    pub section: String,
    pub icon: String,
    pub row: usize,
}

/// `SidebarJumpWindow.score(_:_:)`: title span first, else section + title.
pub fn jump_score(item: &JumpItem, q: &str) -> Option<i64> {
    if q.is_empty() {
        return Some(0);
    }
    let span = |s: &str| -> Option<i64> {
        let chars: Vec<char> = s.to_lowercase().chars().collect();
        let (mut i, mut first, mut last) = (0usize, -1i64, -1i64);
        for ch in q.chars() {
            let j = chars[i..].iter().position(|c| *c == ch)? + i;
            if first < 0 {
                first = j as i64;
            }
            last = j as i64;
            i = j + 1;
        }
        Some((last - first) * 4 + first)
    };
    span(&item.title).or_else(|| span(&format!("{} {}", item.section, item.title)).map(|s| 1000 + s))
}

/// `reload()`: the ranked, filtered items.
pub fn jump_filter(all: &[JumpItem], query: &str) -> Vec<JumpItem> {
    let q: String = query.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    let mut scored: Vec<(usize, i64)> =
        all.iter().enumerate().filter_map(|(i, it)| jump_score(it, &q).map(|s| (i, s))).collect();
    scored.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
    scored.into_iter().map(|(i, _)| all[i].clone()).collect()
}

/// `SidebarJumpWindow` (`do:jira-jump`).
pub struct SidebarJumpUi {
    pub panel: Retained<PopupPanel>,
    list: ListPanel,
    empty: Retained<NSTextField>,
    all: RefCell<Vec<JumpItem>>,
    shown: RefCell<Vec<JumpItem>>,
    selection: Cell<usize>,
    cfg: PaneFindConfig,
    colors: PopupColors,
    on_jump: RefCell<Option<Box<dyn Fn(usize)>>>,
}

impl SidebarJumpUi {
    pub fn new(mtm: MainThreadMarker, cfg: PaneFindConfig) -> Rc<SidebarJumpUi> {
        let colors = crate::ui::theme::PopupThemeDefaults::colors();
        let list = ListPanel::new(mtm, colors, cfg.width, ROW_H, "");
        let config = PopupConfig {
            name: "sidebar-jump".to_string(),
            frame: CardFrame { x: 0.0, y: 0.0, width: cfg.width, height: list.height(1, cfg.rows) },
            tool_panel: true,
            floating: true,
            ..PopupConfig::default()
        };
        let panel = PopupPanel::create(mtm, &config);
        panel.setOpaque(false);
        panel.setBackgroundColor(Some(&objc2_app_kit::NSColor::clearColor()));
        panel.setHasShadow(true);
        panel.setContentView(Some(&list.root));
        let empty = NSTextField::labelWithString(&NSString::from_str(""), mtm);
        empty.setFont(Some(&style::system_font(12.0, Weight::Regular)));
        empty.setTextColor(Some(&colors.dim.to_nscolor()));
        empty.setHidden(true);
        list.root.addSubview(&empty);
        let ui = Rc::new(SidebarJumpUi {
            panel,
            list,
            empty,
            all: RefCell::new(Vec::new()),
            shown: RefCell::new(Vec::new()),
            selection: Cell::new(0),
            cfg,
            colors,
            on_jump: RefCell::new(None),
        });
        let weak = Rc::downgrade(&ui);
        ui.list.set_drawer(Box::new(move |r, i, sel, hover| {
            if let Some(ui) = weak.upgrade() {
                ui.draw_row(r, i, sel, hover);
            }
        }));
        let weak = Rc::downgrade(&ui);
        ui.list.set_on_click(Box::new(move |row, _, _| {
            if let Some(ui) = weak.upgrade() {
                ui.open(row);
            }
        }));
        let weak = Rc::downgrade(&ui);
        let weak_cmd = Rc::downgrade(&ui);
        ui.list.set_on_field(
            Box::new(move |_q: String| {
                if let Some(ui) = weak.upgrade() {
                    ui.reload();
                }
            }),
            Box::new(move |cmd| {
                let Some(ui) = weak_cmd.upgrade() else { return false };
                match cmd {
                    "cancelOperation:" => ui.hide(),
                    "insertNewline:" => ui.open(ui.selection.get()),
                    "moveDown:" => ui.move_selection(1),
                    "moveUp:" => ui.move_selection(-1),
                    _ => return false,
                }
                true
            }),
        );
        ui
    }

    pub fn set_on_jump(&self, f: Box<dyn Fn(usize)>) {
        *self.on_jump.borrow_mut() = Some(f);
    }

    pub fn is_shown(&self) -> bool {
        self.panel.isVisible()
    }

    pub fn show(&self, items: Vec<JumpItem>) {
        *self.all.borrow_mut() = items;
        self.reload();
        if !self.is_shown() {
            place_top_center(&self.panel);
        }
        self.panel.makeKeyAndOrderFront(None);
        self.panel.makeFirstResponder(Some(&self.list.search));
    }

    pub fn hide(&self) {
        self.panel.orderOut(None);
    }

    fn reload(&self) {
        let q = self.list.search.stringValue().to_string();
        let shown = jump_filter(&self.all.borrow(), &q);
        let n = shown.len();
        *self.shown.borrow_mut() = shown;
        self.selection.set(0);
        self.list.set_rows(n, 0);
        let h = self.list.height(n, self.cfg.rows);
        let f = self.panel.frame();
        let top = f.origin.y + f.size.height;
        self.panel
            .setFrame_display(NSRect::new(NSPoint::new(f.origin.x, top - h), NSSize::new(self.cfg.width, h)), true);
        let note = if self.all.borrow().is_empty() {
            "The sidebar is empty".to_string()
        } else {
            format!("No sidebar item matches “{}”", q.trim())
        };
        self.empty.setStringValue(&NSString::from_str(&note));
        self.empty.setHidden(n > 0);
        self.empty.setFrame(NSRect::new(
            NSPoint::new(18.0, crate::ui::list_panel::SEARCH_H + 7.0),
            NSSize::new(self.cfg.width - 36.0, 16.0),
        ));
    }

    pub fn move_selection(&self, d: i64) {
        let n = self.shown.borrow().len() as i64;
        if n == 0 {
            return;
        }
        let s = (self.selection.get() as i64 + d).rem_euclid(n) as usize;
        self.selection.set(s);
        self.list.set_rows(n as usize, s);
    }

    pub fn open(&self, i: usize) {
        let Some(row) = self.shown.borrow().get(i).map(|it| it.row) else { return };
        self.hide();
        if let Some(f) = self.on_jump.borrow().as_ref() {
            f(row);
        }
    }

    fn draw_row(&self, rect: crate::ui::chrome::Rect, i: usize, selected: bool, hovered: bool) {
        use crate::ui::chrome::Rect;
        use crate::ui::theme::PopupTone;
        let c = &self.colors;
        let shown = self.shown.borrow();
        let Some(it) = shown.get(i) else { return };
        let inner = Rect::new(rect.x + 6.0, rect.y + 2.0, rect.width - 12.0, rect.height - 4.0);
        style::row_pill(inner, 7.0, selected, selected, hovered, c, style::RowAccent::Rounded(4.0));
        // `tone(_:)` per section, drawn as a tinted chip.
        let tone = match it.section.as_str() {
            "My work" | "My Work" | "MY WORK" => PopupTone::Accent,
            "Boards" | "BOARDS" => PopupTone::Info,
            "Pinned" | "PINNED" | "Starred" | "STARRED" => PopupTone::Warning,
            _ => PopupTone::Accent2,
        };
        let col = c.tone(tone);
        let chip = Rect::new(inner.x + 10.0, inner.mid_y() - 9.0, 18.0, 18.0);
        style::fill_rounded(chip, 5.0, col.with_alpha(0.18));
        let icon = if it.icon.is_empty() { "list.bullet.rectangle" } else { it.icon.as_str() };
        style::draw_symbol(icon, chip, col, 11.0);
        let band = |x: f64, w: f64| Rect::new(x, inner.y, w.max(0.0), inner.height);
        let tf = style::system_font(10.0, Weight::Regular);
        let tw = style::draw_text_right(&it.section, band(inner.x, inner.width - 10.0), &tf, c.dim);
        let x = chip.max_x() + 10.0;
        style::draw_text_left(&it.title, band(x, inner.max_x() - x - tw - 20.0), &style::system_font(13.0, Weight::Regular), c.text);
    }
}

// ------------------------------------------------------------- pane finder

/// The pane item finder behind `<leader>sf` on the non-nvim panes: a floating
/// fuzzy list over the focused pane's rows (Jira issues, Confluence results,
/// the files list, the sidebar). Picking a row activates it, exactly like a
/// mouse click on that row.
pub struct PaneFindUi {
    pub panel: Retained<PopupPanel>,
    list: ListPanel,
    empty: Retained<NSTextField>,
    all: RefCell<Vec<String>>,
    /// Indices into `all`, in filtered order.
    shown: RefCell<Vec<usize>>,
    /// An index into `shown`.
    selection: Cell<usize>,
    cfg: PaneFindConfig,
    colors: PopupColors,
    on_pick: RefCell<Option<Box<dyn Fn(usize)>>>,
}

impl PaneFindUi {
    pub fn new(mtm: MainThreadMarker, placeholder: &str, cfg: PaneFindConfig) -> Rc<PaneFindUi> {
        let colors = crate::ui::theme::PopupThemeDefaults::colors();
        let list = ListPanel::new(mtm, colors, cfg.width, ROW_H, placeholder);
        let config = PopupConfig {
            name: "pane-find".to_string(),
            frame: CardFrame { x: 0.0, y: 0.0, width: cfg.width, height: list.height(1, cfg.rows) },
            tool_panel: true,
            floating: true,
            ..PopupConfig::default()
        };
        let panel = PopupPanel::create(mtm, &config);
        panel.setOpaque(false);
        panel.setBackgroundColor(Some(&objc2_app_kit::NSColor::clearColor()));
        panel.setHasShadow(true);
        panel.setContentView(Some(&list.root));
        let empty = NSTextField::labelWithString(&NSString::from_str(""), mtm);
        empty.setFont(Some(&style::system_font(12.0, Weight::Regular)));
        empty.setTextColor(Some(&colors.dim.to_nscolor()));
        empty.setHidden(true);
        list.root.addSubview(&empty);
        let ui = Rc::new(PaneFindUi {
            panel,
            list,
            empty,
            all: RefCell::new(Vec::new()),
            shown: RefCell::new(Vec::new()),
            selection: Cell::new(0),
            cfg,
            colors,
            on_pick: RefCell::new(None),
        });
        let weak = Rc::downgrade(&ui);
        ui.list.set_drawer(Box::new(move |r, i, sel, hover| {
            if let Some(ui) = weak.upgrade() {
                ui.draw_row(r, i, sel, hover);
            }
        }));
        let weak = Rc::downgrade(&ui);
        ui.list.set_on_click(Box::new(move |row, _, _| {
            if let Some(ui) = weak.upgrade() {
                ui.open(row);
            }
        }));
        let weak = Rc::downgrade(&ui);
        let weak_cmd = Rc::downgrade(&ui);
        ui.list.set_on_field(
            Box::new(move |_q: String| {
                if let Some(ui) = weak.upgrade() {
                    ui.reload();
                }
            }),
            Box::new(move |cmd| {
                let Some(ui) = weak_cmd.upgrade() else { return false };
                match cmd {
                    "cancelOperation:" => ui.hide(),
                    "insertNewline:" => ui.open(ui.selection.get()),
                    "moveDown:" => ui.move_selection(1),
                    "moveUp:" => ui.move_selection(-1),
                    _ => return false,
                }
                true
            }),
        );
        ui
    }

    pub fn set_on_pick(&self, f: Box<dyn Fn(usize)>) {
        *self.on_pick.borrow_mut() = Some(f);
    }

    pub fn is_shown(&self) -> bool {
        self.panel.isVisible()
    }

    /// Show (or re-raise) over `items`, resetting the query.
    pub fn show(&self, items: Vec<String>) {
        *self.all.borrow_mut() = items;
        self.list.search.setStringValue(&NSString::from_str(""));
        self.reload();
        if !self.is_shown() {
            place_top_center(&self.panel);
        }
        self.panel.makeKeyAndOrderFront(None);
        self.panel.makeFirstResponder(Some(&self.list.search));
    }

    pub fn hide(&self) {
        self.panel.orderOut(None);
    }

    fn reload(&self) {
        let q = self.list.search.stringValue().to_string();
        let all = self.all.borrow();
        let q = q.trim();
        let shown: Vec<usize> = if q.is_empty() {
            (0..all.len()).collect()
        } else {
            let mut idx = FuzzyIndex::new(&all[..]);
            idx.ranked(q)
        };
        let n = shown.len();
        *self.shown.borrow_mut() = shown;
        self.selection.set(0);
        self.list.set_rows(n, 0);
        let h = self.list.height(n, self.cfg.rows);
        let f = self.panel.frame();
        let top = f.origin.y + f.size.height;
        self.panel
            .setFrame_display(NSRect::new(NSPoint::new(f.origin.x, top - h), NSSize::new(self.cfg.width, h)), true);
        let note = if all.is_empty() {
            "Nothing to pick in this pane".to_string()
        } else {
            format!("No item matches “{}”", q)
        };
        self.empty.setStringValue(&NSString::from_str(&note));
        self.empty.setHidden(n > 0);
        self.empty.setFrame(NSRect::new(
            NSPoint::new(18.0, crate::ui::list_panel::SEARCH_H + 7.0),
            NSSize::new(self.cfg.width - 36.0, 16.0),
        ));
    }

    pub fn move_selection(&self, d: i64) {
        let n = self.shown.borrow().len() as i64;
        if n == 0 {
            return;
        }
        let s = (self.selection.get() as i64 + d).rem_euclid(n) as usize;
        self.selection.set(s);
        self.list.set_rows(n as usize, s);
    }

    /// Pick shown row `i`: hide, then run the activation callback with the
    /// original item index.
    pub fn open(&self, i: usize) {
        let Some(orig) = self.shown.borrow().get(i).copied() else { return };
        self.hide();
        if let Some(f) = self.on_pick.borrow().as_ref() {
            f(orig);
        }
    }

    fn draw_row(&self, rect: crate::ui::chrome::Rect, i: usize, selected: bool, hovered: bool) {
        use crate::ui::chrome::Rect;
        let c = &self.colors;
        let shown = self.shown.borrow();
        let Some(orig) = shown.get(i).copied() else { return };
        let text = self.all.borrow().get(orig).cloned().unwrap_or_default();
        let inner = Rect::new(rect.x + 6.0, rect.y + 2.0, rect.width - 12.0, rect.height - 4.0);
        style::row_pill(inner, 7.0, selected, selected, hovered, c, style::RowAccent::Rounded(4.0));
        let band = Rect::new(inner.x + 12.0, inner.y, inner.width - 22.0, inner.height);
        style::draw_text_left(&text, band, &style::system_font(13.0, Weight::Regular), c.text);
    }
}

/// `place()`: centered on the mouse's screen, 20% down from the top.
fn place_top_center(panel: &PopupPanel) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let mouse = NSEvent::mouseLocation();
    let screens = NSScreen::screens(mtm);
    let scr = screens
        .iter()
        .find(|s| {
            let f = s.frame();
            mouse.x >= f.origin.x && mouse.x <= f.origin.x + f.size.width && mouse.y >= f.origin.y && mouse.y <= f.origin.y + f.size.height
        })
        .or_else(|| NSScreen::mainScreen(mtm));
    let Some(scr) = scr else { return };
    let vis = scr.visibleFrame();
    let f = panel.frame();
    let x = vis.origin.x + vis.size.width / 2.0 - f.size.width / 2.0;
    let y = (vis.origin.y + vis.size.height - vis.size.height * 0.2 - f.size.height).max(vis.origin.y);
    panel.setFrameOrigin(NSPoint::new(x.round(), y.round()));
}
