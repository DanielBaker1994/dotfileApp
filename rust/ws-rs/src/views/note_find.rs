//! `NoteFindWindow.swift`: the leader+sf picker over the open notes and the
//! leader+sg ripgrep over them (`do:notes-find` / `do:notes-grep`). The model
//! is [`NoteFindModel`]; this is the floating list panel around it, built on
//! [`ListPanel`] like the Ctrl+B W switcher.

#![cfg(target_os = "macos")]

use crate::engines::list_filter::FuzzyIndex;
use crate::ui::list_panel::ListPanel;
use crate::ui::popup::{CardFrame, PopupConfig, PopupPanel};
use crate::ui::style::{self, Weight};
use crate::ui::theme::PopupColors;
use crate::views::notes::{parse_rgrep, rg_args, rg_path, sort_grep, NoteFindMode, NoteFindModel};
use objc2::rc::Retained;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSEvent, NSScreen, NSTextField};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

const ROW_H: f64 = 30.0;

/// `[notes-find]` settings.
#[derive(Clone, Debug)]
pub struct NoteFindConfig {
    pub width: f64,
    pub rows: usize,
    pub grep_max: usize,
    pub rg_bin: Option<String>,
}

impl NoteFindConfig {
    pub fn from_section(s: &HashMap<String, String>) -> Self {
        let num = |k: &str| s.get(k).and_then(|v| v.trim().parse::<f64>().ok());
        NoteFindConfig {
            width: num("width").unwrap_or(640.0).max(360.0),
            rows: num("rows").map(|v| v as usize).unwrap_or(12).max(5),
            grep_max: num("grep-max").map(|v| v as usize).unwrap_or(200).max(1),
            rg_bin: s.get("rg-bin").cloned(),
        }
    }
}

type OpenFn = Box<dyn Fn(String, Option<i64>)>;

pub struct NoteFindUi {
    pub panel: Retained<PopupPanel>,
    list: ListPanel,
    empty: Retained<NSTextField>,
    model: RefCell<NoteFindModel>,
    paths: RefCell<Vec<String>>,
    cfg: NoteFindConfig,
    colors: PopupColors,
    grep_gen: Cell<u64>,
    on_open: RefCell<Option<OpenFn>>,
    on_log: RefCell<Option<Box<dyn Fn(String)>>>,
}

thread_local! {
    /// The live grep window, for the ripgrep worker's main-thread reply.
    static GREP_UI: RefCell<Option<Weak<NoteFindUi>>> = const { RefCell::new(None) };
}

impl NoteFindUi {
    pub fn new(mtm: MainThreadMarker, mode: NoteFindMode, cfg: NoteFindConfig) -> Rc<NoteFindUi> {
        let colors = crate::ui::theme::PopupThemeDefaults::colors();
        let list = ListPanel::new(mtm, colors, cfg.width, ROW_H, "");
        let h = list.height(1, cfg.rows);
        let config = PopupConfig {
            name: "notes-find".to_string(),
            frame: CardFrame { x: 0.0, y: 0.0, width: cfg.width, height: h },
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
        let model = NoteFindModel::new(mode, cfg.rows, cfg.grep_max);
        let ui = Rc::new(NoteFindUi {
            panel,
            list,
            empty,
            model: RefCell::new(model),
            paths: RefCell::new(Vec::new()),
            cfg,
            colors,
            grep_gen: Cell::new(0),
            on_open: RefCell::new(None),
            on_log: RefCell::new(None),
        });
        let weak = Rc::downgrade(&ui);
        ui.list.set_drawer(Box::new(move |r, i, sel, hover| {
            if let Some(ui) = weak.upgrade() {
                ui.draw_row(r, i, sel, hover);
            }
        }));
        let weak = Rc::downgrade(&ui);
        ui.list.set_on_click(Box::new(move |row, _x, _double| {
            if let Some(ui) = weak.upgrade() {
                ui.open(row);
            }
        }));
        let weak = Rc::downgrade(&ui);
        let weak_cmd = Rc::downgrade(&ui);
        ui.list.set_on_field(
            Box::new(move |q: String| {
                if let Some(ui) = weak.upgrade() {
                    ui.query_changed(&q);
                }
            }),
            Box::new(move |cmd| {
                let Some(ui) = weak_cmd.upgrade() else { return false };
                match cmd {
                    "cancelOperation:" => {
                        ui.hide();
                        true
                    }
                    "insertNewline:" => {
                        ui.open(ui.list.rows.selection());
                        true
                    }
                    "moveDown:" => {
                        ui.move_selection(1);
                        true
                    }
                    "moveUp:" => {
                        ui.move_selection(-1);
                        true
                    }
                    _ => false,
                }
            }),
        );
        if mode == NoteFindMode::Grep {
            GREP_UI.with(|g| *g.borrow_mut() = Some(Rc::downgrade(&ui)));
        }
        ui
    }

    pub fn mode(&self) -> NoteFindMode {
        self.model.borrow().mode
    }

    pub fn set_on_open(&self, f: OpenFn) {
        *self.on_open.borrow_mut() = Some(f);
    }

    pub fn set_on_log(&self, f: Box<dyn Fn(String)>) {
        *self.on_log.borrow_mut() = Some(f);
    }

    fn log(&self, s: String) {
        if let Some(f) = self.on_log.borrow().as_ref() {
            f(s);
        }
    }

    pub fn is_shown(&self) -> bool {
        self.panel.isVisible()
    }

    pub fn has_key(&self) -> bool {
        self.panel.isKeyWindow()
    }

    /// `show()`: place near the top of the mouse's screen, refresh, focus.
    pub fn show(&self, open_paths: Vec<String>) {
        *self.paths.borrow_mut() = open_paths.clone();
        let q = self.list.search.stringValue().to_string().trim().to_string();
        {
            let mut m = self.model.borrow_mut();
            m.query = q.clone();
            m.set_open_paths(&open_paths);
            if m.mode == NoteFindMode::Files {
                self.log(format!("notes-find: {} open notes", m.all.len()));
            }
        }
        if self.mode() == NoteFindMode::Grep {
            self.query_changed(&q);
        }
        self.sync();
        self.place();
        self.panel.makeKeyAndOrderFront(None);
        self.panel.makeFirstResponder(Some(&self.list.search));
    }

    pub fn hide(&self) {
        self.panel.orderOut(None);
    }

    fn place(&self) {
        place_top_center(&self.panel);
    }

    fn query_changed(&self, q: &str) {
        let mode = self.mode();
        self.model.borrow_mut().set_query(q);
        match mode {
            NoteFindMode::Files => self.sync(),
            NoteFindMode::Grep => self.run_grep(),
        }
    }

    /// `runGrep()`: ripgrep the open notes off-main, newest query wins.
    fn run_grep(&self) {
        let gen = self.grep_gen.get() + 1;
        self.grep_gen.set(gen);
        let q = self.model.borrow().query.trim().to_string();
        let paths: Vec<String> =
            self.paths.borrow().iter().filter(|p| std::path::Path::new(p.as_str()).exists()).cloned().collect();
        if q.chars().count() < 2 || paths.is_empty() {
            // `set_query` already set the placeholder note + cleared rows.
            self.sync();
            return;
        }
        let Some(rg) = rg_path(self.cfg.rg_bin.as_deref()) else {
            {
                let mut m = self.model.borrow_mut();
                m.grep_note = "ripgrep (rg) not found — brew install ripgrep".into();
                m.show_grep(&[], 0);
            }
            self.sync();
            return;
        };
        let cap = self.cfg.grep_max;
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(120));
            let live = GREP_GEN.load(std::sync::atomic::Ordering::SeqCst);
            if live > gen {
                return;
            }
            let t0 = std::time::Instant::now();
            let out = std::process::Command::new(&rg)
                .args(rg_args(&q, &paths))
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                .unwrap_or_default();
            let mut hits = parse_rgrep(&out, cap);
            sort_grep(&mut hits, &paths);
            let ms = t0.elapsed().as_millis();
            let n = paths.len();
            crate::platform::run_on_main(move || {
                let ui = GREP_UI.with(|g| g.borrow().as_ref().and_then(|w| w.upgrade()));
                let Some(ui) = ui else { return };
                if ui.grep_gen.get() != gen {
                    return;
                }
                {
                    let mut m = ui.model.borrow_mut();
                    m.grep_note = format!("No match for “{q}”");
                    m.show_grep(&hits, n);
                }
                ui.sync();
                ui.log(format!("notes-grep: '{q}' {} hits, {ms} ms", hits.len()));
            });
        });
        GREP_GEN.store(gen, std::sync::atomic::Ordering::SeqCst);
    }

    /// Mirror the model into the rows, the empty note and the window height.
    fn sync(&self) {
        let (n, sel, note) = {
            let m = self.model.borrow();
            (m.shown.len(), m.selection, m.empty_message())
        };
        self.list.set_rows(n, sel);
        // Nothing to list and nothing to say: just the search field.
        let h = if n == 0 && note.is_empty() {
            crate::ui::list_panel::SEARCH_H + crate::ui::list_panel::PAD_BOTTOM
        } else {
            self.list.height(n, self.cfg.rows)
        };
        let f = self.panel.frame();
        let top = f.origin.y + f.size.height;
        self.panel.setFrame_display(
            NSRect::new(NSPoint::new(f.origin.x, top - h), NSSize::new(self.cfg.width, h)),
            true,
        );
        self.empty.setStringValue(&NSString::from_str(&note));
        self.empty.setHidden(n > 0 || note.is_empty());
        self.empty.setFrame(NSRect::new(
            NSPoint::new(18.0, h - crate::ui::list_panel::SEARCH_H - ROW_H + 7.0),
            NSSize::new(self.cfg.width - 36.0, 16.0),
        ));
    }

    pub fn move_selection(&self, d: i64) {
        let sel = {
            let mut m = self.model.borrow_mut();
            m.move_selection(d);
            m.selection
        };
        let n = self.model.borrow().shown.len();
        self.list.set_rows(n, sel);
    }

    /// `open(_:)`.
    pub fn open(&self, i: usize) {
        let Some((path, line)) = self.model.borrow().open(i) else { return };
        self.hide();
        self.log(format!("notes-find: open {path}"));
        if let Some(f) = self.on_open.borrow().as_ref() {
            f(path, line);
        }
    }

    pub fn selection(&self) -> usize {
        self.model.borrow().selection
    }

    /// `copyPath(_:)` — the selected row's path (Cmd+C with no text selected).
    pub fn selected_path(&self) -> Option<String> {
        let m = self.model.borrow();
        m.shown.get(m.selection).map(|h| h.path.clone())
    }

    /// Whether the search field has a text selection (Cmd+C copies it then).
    pub fn field_has_selection(&self) -> bool {
        self.list
            .search
            .currentEditor()
            .map(|e| e.selectedRange().length > 0)
            .unwrap_or(false)
    }

    fn draw_row(&self, rect: crate::ui::chrome::Rect, i: usize, selected: bool, hovered: bool) {
        use crate::ui::chrome::Rect;
        let c = &self.colors;
        let m = self.model.borrow();
        let Some(h) = m.shown.get(i) else { return };
        let inner = Rect::new(rect.x + 6.0, rect.y + 2.0, rect.width - 12.0, rect.height - 4.0);
        style::row_pill(inner, 7.0, selected, selected, hovered, c, style::RowAccent::Rounded(4.0));
        let band = |x: f64, w: f64| Rect::new(x, inner.y, w.max(0.0), inner.height);
        let x = inner.x + 12.0;
        style::draw_symbol(
            crate::ui::sidebar::icon_for_name(&h.path),
            Rect::new(x, inner.y, 16.0, inner.height),
            c.dim,
            11.0,
        );
        let name_x = x + 24.0;
        let file = std::path::Path::new(&h.path)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (name, trailing) = match m.mode {
            NoteFindMode::Files => {
                let dir = std::path::Path::new(&h.rel)
                    .parent()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs_f64())
                    .unwrap_or(0.0);
                let age = crate::views::paths::age(now - h.mtime);
                (file, if dir.is_empty() { age } else { format!("{dir} · {age}") })
            }
            NoteFindMode::Grep => {
                let line = m.lines.get(i).copied().unwrap_or(0);
                let text = m.grep_texts.get(i).cloned().unwrap_or_default();
                (if text.is_empty() { "(blank)".into() } else { text }, format!("{file}:{line}"))
            }
        };
        let tf = style::system_font(10.0, Weight::Regular);
        let tw = style::draw_text_right(&trailing, band(inner.x, inner.width - 10.0), &tf, c.dim);
        let font = style::system_font(13.0, Weight::Regular);
        let avail = inner.max_x() - name_x - tw - 20.0;
        if m.mode == NoteFindMode::Grep {
            // Cleaned line, windowed on the match, with the match accented.
            let (before, hit, after) = grep_snippet(&name, &m.query);
            let mut x = name_x;
            for (seg, color) in [(before, c.text), (hit, c.accent), (after, c.text)] {
                if seg.is_empty() {
                    continue;
                }
                let left = (name_x + avail - x).max(0.0);
                if left < 6.0 {
                    break;
                }
                x += style::draw_text_left(&seg, band(x, left), &font, color);
            }
            return;
        }
        style::draw_text_left(&name, band(name_x, avail), &font, c.text);
    }
}

/// A grep hit line made readable: markdown noise and runs of whitespace
/// removed, then windowed so the first match of `query` is visible near the
/// start. Returns (before, match, after).
pub fn grep_snippet(line: &str, query: &str) -> (String, String, String) {
    let mut clean = String::new();
    for ch in line.replace("**", "").replace('`', "").trim_start_matches(['#', '>', '-', '*', ' ', '\t']).chars() {
        if ch.is_whitespace() {
            if !clean.ends_with(' ') {
                clean.push(' ');
            }
        } else {
            clean.push(ch);
        }
    }
    let clean = clean.trim().to_string();
    let chars: Vec<char> = clean.chars().collect();
    let q: Vec<char> = query.trim().to_lowercase().chars().collect();
    let lower: Vec<char> = clean.to_lowercase().chars().collect();
    let pos = if q.is_empty() || lower.len() != chars.len() {
        None
    } else {
        (0..=chars.len().saturating_sub(q.len())).find(|&i| lower[i..].starts_with(&q))
    };
    let Some(p) = pos else { return (String::new(), clean, String::new()) };
    let start = p.saturating_sub(18);
    let before: String = if start > 0 {
        format!("…{}", chars[start..p].iter().collect::<String>().trim_start())
    } else {
        chars[..p].iter().collect()
    };
    let hit: String = chars[p..p + q.len()].iter().collect();
    let after: String = chars[p + q.len()..].iter().collect();
    (before, hit, after)
}

static GREP_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

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
    cfg: NoteFindConfig,
    colors: PopupColors,
    on_jump: RefCell<Option<Box<dyn Fn(usize)>>>,
}

impl SidebarJumpUi {
    pub fn new(mtm: MainThreadMarker, cfg: NoteFindConfig) -> Rc<SidebarJumpUi> {
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

    pub fn has_key(&self) -> bool {
        self.panel.isKeyWindow()
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
            NSPoint::new(18.0, h - crate::ui::list_panel::SEARCH_H - ROW_H + 7.0),
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

    pub fn selection(&self) -> usize {
        self.selection.get()
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
    cfg: NoteFindConfig,
    colors: PopupColors,
    on_pick: RefCell<Option<Box<dyn Fn(usize)>>>,
}

impl PaneFindUi {
    pub fn new(mtm: MainThreadMarker, placeholder: &str, cfg: NoteFindConfig) -> Rc<PaneFindUi> {
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

    pub fn has_key(&self) -> bool {
        self.panel.isKeyWindow()
    }

    pub fn selection(&self) -> usize {
        self.selection.get()
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
            NSPoint::new(18.0, h - crate::ui::list_panel::SEARCH_H - ROW_H + 7.0),
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

#[cfg(test)]
mod grep_snippet_tests {
    use super::grep_snippet;

    #[test]
    fn windows_on_the_match() {
        let (b, h, a) = grep_snippet("  **Themed** `dot` fences: no colors, fonts or sizes", "DOT");
        assert_eq!((b.as_str(), h.as_str()), ("Themed ", "dot"));
        assert!(a.starts_with(" fences"));
        let long = "x".repeat(60) + " needle tail";
        let (b, h, _) = grep_snippet(&long, "needle");
        assert!(b.starts_with('…') && h == "needle");
        assert_eq!(grep_snippet("no hit here", "zzz").1, "no hit here");
    }
}
