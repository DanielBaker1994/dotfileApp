//! `NoteFindWindow.swift`: the leader+sf picker over the open notes and the
//! leader+sg ripgrep over them (`do:notes-find` / `do:notes-grep`). The model
//! is [`NoteFindModel`]; this is the floating list panel around it, built on
//! [`ListPanel`] like the Ctrl+B W switcher.

#![cfg(target_os = "macos")]

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
        let placeholder = match mode {
            NoteFindMode::Files => "open notes — type to filter · ↩ switch · esc close",
            NoteFindMode::Grep => "search the open notes (ripgrep) — ↩ jump to the line · esc close",
        };
        let list = ListPanel::new(mtm, colors, cfg.width, ROW_H, placeholder);
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
        ui.list.set_drawer(Box::new(move |r, i, sel, _hover| {
            if let Some(ui) = weak.upgrade() {
                ui.draw_row(r, i, sel);
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
        let Some(mtm) = MainThreadMarker::new() else { return };
        let mouse = NSEvent::mouseLocation();
        let screens = NSScreen::screens(mtm);
        let scr = screens
            .iter()
            .find(|s| {
                let f = s.frame();
                mouse.x >= f.origin.x
                    && mouse.x <= f.origin.x + f.size.width
                    && mouse.y >= f.origin.y
                    && mouse.y <= f.origin.y + f.size.height
            })
            .or_else(|| NSScreen::mainScreen(mtm));
        let Some(scr) = scr else { return };
        let vis = scr.visibleFrame();
        let f = self.panel.frame();
        let x = vis.origin.x + vis.size.width / 2.0 - f.size.width / 2.0;
        let y = (vis.origin.y + vis.size.height - vis.size.height * 0.2 - f.size.height).max(vis.origin.y);
        self.panel.setFrameOrigin(NSPoint::new(x.round(), y.round()));
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
            dispatch2::DispatchQueue::main().exec_async(move || {
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
        let h = self.list.height(n, self.cfg.rows);
        let f = self.panel.frame();
        let top = f.origin.y + f.size.height;
        self.panel.setFrame_display(
            NSRect::new(NSPoint::new(f.origin.x, top - h), NSSize::new(self.cfg.width, h)),
            true,
        );
        self.empty.setStringValue(&NSString::from_str(&note));
        self.empty.setHidden(n > 0);
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

    fn draw_row(&self, rect: crate::ui::chrome::Rect, i: usize, selected: bool) {
        use crate::ui::chrome::Rect;
        let c = &self.colors;
        let m = self.model.borrow();
        let Some(h) = m.shown.get(i) else { return };
        let inner = Rect::new(rect.x + 6.0, rect.y + 2.0, rect.width - 12.0, rect.height - 4.0);
        if selected {
            style::fill_rounded(inner, 7.0, c.highlight);
            style::fill_rounded(Rect::new(inner.x, inner.y + 4.0, 3.0, inner.height - 8.0), 1.5, c.accent);
        }
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
        style::draw_text_left(&name, band(name_x, inner.max_x() - name_x - tw - 20.0), &font, c.text);
    }
}

static GREP_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
