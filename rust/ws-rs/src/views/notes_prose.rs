//! The notes window's in-place reading view (`ProseView`, NotesProse.swift):
//! a `WKWebView` laid over the editor that renders the note through
//! `prose.screen_html`, with copy buttons, the outline, `/` find, `gg`/`G`,
//! Ctrl+D/U, zoom and Cmd+P PDF export. The pop-out window
//! (`ProseWindow`/`ProseProcess`) lives in `notes.rs`.

/// The page scripts, verbatim from `ProseView.copyButtonJS` / `outlineJS` /
/// `findJS`.
pub const COPY_BUTTON_JS: &str = include_str!("prose_js/copy_buttons.js");
pub const OUTLINE_JS: &str = include_str!("prose_js/outline.js");
pub const FIND_JS: &str = include_str!("prose_js/find.js");

/// `ProseView.init`'s document-start script: one click on an image opens it
/// in the FilePopup preview.
pub const IMAGE_CLICK_JS: &str = "document.addEventListener('click', function (e) {\n  var t = e.target;\n  if (t && t.tagName === 'IMG') window.webkit.messageHandlers.wsImage.postMessage(t.src);\n});";

/// `scrollToSourceLine(_:)`: the last `[data-pos]` block at or above line `n`.
pub fn scroll_to_line_js(n: i64) -> String {
    format!(
        "(function(n){{var best=null,bl=-1;n=+n;\
         document.querySelectorAll('[data-pos]').forEach(function(e){{\
         var l=parseInt(e.getAttribute('data-pos'));\
         if(!isNaN(l)&&l<=n&&l>=bl){{bl=l;best=e;}}}});\
         if(best){{window.scrollTo(0,best.getBoundingClientRect().top+window.scrollY-8);}}}})({n})"
    )
}

/// `topSourceLine(_:)`: the source line of the first block still on screen.
pub const TOP_SOURCE_LINE_JS: &str = "(function(){var r=null;\
var all=document.querySelectorAll('[data-pos]');\
for(var i=0;i<all.length;i++){var b=all[i].getBoundingClientRect();\
if(b.bottom>4){var l=parseInt(all[i].getAttribute('data-pos'));if(!isNaN(l)){r=l;break;}}}\
return r;})()";

/// Where the page lands once loaded: a source line (vim sync) or a pixel
/// offset (re-render of the same note).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LandAt {
    Line(i64),
    Y(f64),
}

/// A `load` hook spliced before `</body>` so the page lands in place without
/// Swift's 0.15 s timer race.
pub fn land_script(at: LandAt) -> String {
    let js = match at {
        LandAt::Line(n) => scroll_to_line_js(n),
        LandAt::Y(y) => format!("window.scrollTo(0,{y})"),
    };
    format!("<script>window.addEventListener('load',function(){{{js}}});</script>")
}

/// Splice `extra` before `</body>` (or append when there is none).
pub fn splice_before_body_end(html: &str, extra: &str) -> String {
    let lower = html.to_ascii_lowercase();
    match lower.rfind("</body>") {
        Some(i) => format!("{}{}{}", &html[..i], extra, &html[i..]),
        None => format!("{html}{extra}"),
    }
}

/// `ProseView.show`'s cache keys: the full key (skip when unchanged) and the
/// head key (patch the body in place when only the text changed).
pub fn show_keys(path: &str, size: f64, font: &str, width: f64, sig: &str, copy: bool, markdown: &str) -> (String, String) {
    let full = format!(
        "{path}\u{1}{}\u{1}{font}\u{1}{}\u{1}{sig}\u{1}{copy}\u{1}{markdown}",
        size as i64, width as i64
    );
    let head = format!("{path}\u{1}{}\u{1}{sig}\u{1}{copy}", width as i64);
    (full, head)
}

/// A message posted by the page scripts.
#[derive(Clone, Debug, PartialEq)]
pub enum ProseMsg {
    /// `wsSearch`: the find bar's state (`idle` / `typing` / `active`).
    Search(String),
    /// `wsJump`: an outline entry was clicked — move the editor there.
    Jump(i64),
    /// `wsImage`: an image was double-clicked (a `file://` path).
    Image(String),
}

#[cfg(target_os = "macos")]
pub use appkit::{add_find, show_context_menu, ProsePane, ProseWebView};

#[cfg(target_os = "macos")]
mod appkit {
    use super::*;
    use crate::ui::theme::PopupColors;
    use crate::views::notes::{
        Mods, NotesConfig, ProseRender, ProseSource, ProseViewState,
    };
    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject, ProtocolObject};
    use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSAutoresizingMaskOptions, NSView};
    use objc2_foundation::{
        ns_string, NSError, NSNumber, NSObjectProtocol, NSRect, NSString, NSURL,
    };
    use objc2_web_kit::{
        WKScriptMessage, WKScriptMessageHandler, WKUserContentController, WKUserScript,
        WKUserScriptInjectionTime, WKWebView, WKWebViewConfiguration,
    };
    use std::cell::RefCell;
    use std::rc::Rc;

    use crate::ui::view_base::as_any;

    type Sink = Rc<RefCell<Option<Box<dyn Fn(ProseMsg)>>>>;

    pub struct HandlerIvars {
        sink: Sink,
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSProseMsgHandler"]
        #[ivars = HandlerIvars]
        pub struct ProseMsgHandler;

        impl ProseMsgHandler {
            #[unsafe(method(userContentController:didReceiveScriptMessage:))]
            fn did_receive(&self, _c: &WKUserContentController, m: &WKScriptMessage) {
                let name = unsafe { m.name() }.to_string();
                let body = unsafe { m.body() };
                let any = as_any(&*body);
                let msg = match name.as_str() {
                    "wsSearch" => Some(ProseMsg::Search(
                        any.downcast_ref::<NSString>().map(|s| s.to_string()).unwrap_or_else(|| "idle".into()),
                    )),
                    "wsJump" => any
                        .downcast_ref::<NSNumber>()
                        .map(|n| ProseMsg::Jump(n.integerValue() as i64)),
                    "wsImage" => any.downcast_ref::<NSString>().and_then(|s| {
                        let u = NSURL::URLWithString(s)?;
                        if !u.isFileURL() {
                            return None;
                        }
                        u.path().map(|p| ProseMsg::Image(p.to_string()))
                    }),
                    _ => None,
                };
                if let Some(msg) = msg {
                    // Clone the callback out so it can re-enter the surface.
                    let sink = self.ivars().sink.clone();
                    let guard = sink.borrow();
                    if let Some(cb) = guard.as_ref() {
                        cb(msg);
                    }
                }
            }
        }

        unsafe impl NSObjectProtocol for ProseMsgHandler {}
        unsafe impl WKScriptMessageHandler for ProseMsgHandler {}
    );

    /// Load bookkeeping shared with the async render completion.
    #[derive(Default)]
    struct Loaded {
        path: Option<String>,
        head_key: String,
        gen: u64,
    }

    pub struct ProsePane {
        pub web: Retained<WKWebView>,
        _handler: Retained<ProseMsgHandler>,
        sink: Sink,
        pub state: ProseViewState,
        last_key: String,
        last_path: String,
        colors: PopupColors,
        loaded: Rc<RefCell<Loaded>>,
        page_file: std::path::PathBuf,
        /// The source line to land on at the next show (vim `line('w0')`).
        pub sync_line: Option<i64>,
        menu_ctx: Rc<RefCell<(String, PopupColors)>>,
    }

    const ZOOM_KEY: &str = "proseZoom";

    impl ProsePane {
        pub fn new(mtm: MainThreadMarker, frame: NSRect) -> ProsePane {
            let config = unsafe { WKWebViewConfiguration::new(mtm) };
            let ucc = unsafe { config.userContentController() };
            let sink: Sink = Rc::new(RefCell::new(None));
            let handler = ProseMsgHandler::alloc(mtm).set_ivars(HandlerIvars { sink: sink.clone() });
            let handler: Retained<ProseMsgHandler> = unsafe { msg_send![super(handler), init] };
            for (src, at) in [
                (IMAGE_CLICK_JS, WKUserScriptInjectionTime::AtDocumentStart),
                (COPY_BUTTON_JS, WKUserScriptInjectionTime::AtDocumentEnd),
                (OUTLINE_JS, WKUserScriptInjectionTime::AtDocumentEnd),
                (FIND_JS, WKUserScriptInjectionTime::AtDocumentEnd),
            ] {
                let s = unsafe {
                    WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
                        WKUserScript::alloc(mtm),
                        &NSString::from_str(src),
                        at,
                        true,
                    )
                };
                unsafe { ucc.addUserScript(&s) };
            }
            for name in ["wsImage", "wsJump", "wsSearch"] {
                unsafe {
                    ucc.addScriptMessageHandler_name(
                        ProtocolObject::from_ref(&*handler),
                        &NSString::from_str(name),
                    )
                };
            }
            let menu_ctx: Rc<RefCell<(String, PopupColors)>> = Rc::new(RefCell::new((String::new(), PopupColors::default())));
            let pweb = ProseWebView::new(mtm, frame, &config);
            {
                let ctx = menu_ctx.clone();
                pweb.set_on_menu(Rc::new(move |v: &NSView| {
                    let (note, colors) = ctx.borrow().clone();
                    show_context_menu(&note, colors, v);
                }));
            }
            let web: Retained<WKWebView> = Retained::into_super(pweb);
            let no = NSNumber::numberWithBool(false);
            let _: () = unsafe { msg_send![&*web, setValue: &*no, forKey: ns_string!("drawsBackground")] };
            web.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            // Trackpad pinch-to-zoom (native magnification, anchored at the pinch).
            unsafe { web.setAllowsMagnification(true) };
            web.setHidden(true);
            let z = crate::platform::get_f64(ZOOM_KEY);
            let mut state = ProseViewState::new();
            if z > 0.0 {
                let z = state.apply_zoom(z);
                unsafe { web.setPageZoom(z) };
            }
            let home = std::env::var("HOME").unwrap_or_default();
            let page_file = std::path::PathBuf::from(format!(
                "{home}/.cache/kitchen-sink/prose/page-{}-{}.html",
                std::process::id(),
                Retained::as_ptr(&web) as usize
            ));
            ProsePane {
                web,
                _handler: handler,
                sink,
                state,
                last_key: String::new(),
                last_path: String::new(),
                colors: PopupColors::default(),
                loaded: Rc::new(RefCell::new(Loaded::default())),
                page_file,
                sync_line: None,
                menu_ctx,
            }
        }

        pub fn view(&self) -> &NSView {
            &self.web
        }

        pub fn set_on_message(&self, cb: Box<dyn Fn(ProseMsg)>) {
            *self.sink.borrow_mut() = Some(cb);
        }

        pub fn last_path(&self) -> &str {
            &self.last_path
        }

        pub fn eval(&self, js: &str) {
            unsafe { self.web.evaluateJavaScript_completionHandler(&NSString::from_str(js), None) };
        }

        /// Evaluate `js` and hand its numeric result to `done`.
        pub fn eval_number(&self, js: &str, done: impl Fn(Option<f64>) + 'static) {
            let block = RcBlock::new(move |v: *mut AnyObject, _e: *mut NSError| {
                let n = unsafe { v.as_ref() }
                    .and_then(|o| o.downcast_ref::<NSNumber>())
                    .map(|n| n.doubleValue());
                done(n);
            });
            unsafe {
                self.web
                    .evaluateJavaScript_completionHandler(&NSString::from_str(js), Some(&block))
            };
        }

        /// `topSourceLine(_:)`.
        pub fn top_source_line(&self, done: impl Fn(Option<i64>) + 'static) {
            self.eval_number(TOP_SOURCE_LINE_JS, move |d| {
                done(d.filter(|d| d.is_finite() && *d >= 1.0 && *d < 1e9).map(|d| d as i64))
            });
        }

        pub fn scroll_by(&self, dy: i64) {
            self.eval(&format!("window.scrollBy(0, {dy})"));
        }

        fn apply_sync_line(&mut self) {
            if let Some(n) = self.sync_line.take() {
                self.eval(&scroll_to_line_js(n));
            }
        }

        /// `ProseView.show(_:colors:font:size:width:)` — render off the main
        /// thread, then patch the body in place (same head) or load the page.
        pub fn show(&mut self, src: ProseSource, colors: PopupColors, font: &str, size: f64, width: f64, cfg: &NotesConfig) {
            self.colors = colors;
            *self.menu_ctx.borrow_mut() = (src.path.clone(), colors);
            let sig = format!(
                "{:?}{:?}{:?}",
                colors.base(),
                colors.text,
                colors.accent_on()
            );
            let (key, head_key) = show_keys(&src.path, size, font, width, &sig, cfg.copy_buttons, &src.markdown);
            if key == self.last_key {
                self.apply_sync_line();
                return;
            }
            let (same, same_head, gen) = {
                let mut l = self.loaded.borrow_mut();
                l.gen += 1;
                let same = l.path.as_deref() == Some(src.path.as_str());
                (same, same && l.head_key == head_key, l.gen)
            };
            self.last_key = key;
            self.last_path = src.path.clone();
            let sync = self.sync_line.take();
            let Some(mtm) = MainThreadMarker::new() else { return };
            let bound = dispatch2::MainThreadBound::new((self.web.clone(), self.loaded.clone()), mtm);
            let page_file = self.page_file.clone();
            let font = font.to_string();
            let cfg = cfg.clone();
            // Wait for the scroll position (same note, new head) before rendering.
            let start = move |prev_y: Option<f64>| {
                std::thread::spawn(move || {
                    let html = ProseRender::page(&src, &colors, &font, size, width, &cfg);
                    crate::platform::run_on_main(move || {
                        let Some(mtm) = MainThreadMarker::new() else { return };
                        let (web, loaded) = bound.get(mtm);
                        if loaded.borrow().gen != gen {
                            return;
                        }
                        if same_head {
                            if let Some(body) = ProseRender::body_inner(&html) {
                                let mut js = ProseRender::patch_js(&body);
                                if let Some(n) = sync {
                                    js.push_str(&scroll_to_line_js(n));
                                }
                                unsafe {
                                    web.evaluateJavaScript_completionHandler(&NSString::from_str(&js), None)
                                };
                                return;
                            }
                        }
                        let land = sync.map(LandAt::Line).or(prev_y.map(LandAt::Y));
                        let html = match land {
                            Some(at) => splice_before_body_end(&html, &land_script(at)),
                            None => html,
                        };
                        if let Some(dir) = page_file.parent() {
                            let _ = std::fs::create_dir_all(dir);
                        }
                        if std::fs::write(&page_file, html).is_err() {
                            return;
                        }
                        {
                            let mut l = loaded.borrow_mut();
                            l.path = Some(src.path.clone());
                            l.head_key = head_key.clone();
                        }
                        let url = NSURL::fileURLWithPath(&NSString::from_str(&page_file.to_string_lossy()));
                        let root = NSURL::fileURLWithPath(ns_string!("/"));
                        unsafe { web.loadFileURL_allowingReadAccessToURL(&url, &root) };
                    });
                });
            };
            if same && !same_head && sync.is_none() {
                let once = RefCell::new(Some(start));
                self.eval_number("window.scrollY", move |y| {
                    if let Some(f) = once.borrow_mut().take() {
                        f(y);
                    }
                });
            } else {
                start(None);
            }
        }

        pub fn set_frame(&self, r: NSRect) {
            self.web.setFrame(r);
        }

        pub fn zoom_by(&mut self, factor: f64) {
            let z = self.state.zoom_by(factor);
            unsafe { self.web.setPageZoom(z) };
            self.save_zoom();
        }

        pub fn reset_zoom(&mut self) {
            let z = self.state.reset_zoom();
            unsafe { self.web.setPageZoom(z) };
            self.save_zoom();
        }

        fn save_zoom(&self) {
            crate::platform::set_f64(ZOOM_KEY, self.state.zoom);
        }

        /// `searchKey(code:mods:)` → the web view action; `None` = not handled.
        pub fn search_key(&mut self, code: u16, m: Mods, now: f64) -> Option<bool> {
            let k = self.state.search_key(code, m, now)?;
            if let Some(js) = k.js() {
                self.eval(&js);
            }
            // `g` (first tap) is swallowed too: Swift returns true for it.
            Some(k.consumed() || code == 5)
        }

        /// `exportPDF()` — `prose.pdf_export` off the main thread, then copy
        /// the PDF path and toast it.
        pub fn export_pdf(&self, cfg: &NotesConfig, copy_toast: &str) {
            export_pdf_and_copy(self.last_path.clone(), cfg.clone(), self.colors, copy_toast, ExportCopy::Pdf);
        }
    }

    /// Which of the export's files ([`export_pdf_and_copy`] writes all three)
    /// lands on the clipboard.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum ExportCopy {
        Pdf,
        Html,
        Png,
    }

    impl ExportCopy {
        /// The file type's name and toast icon.
        fn kind(self) -> (&'static str, &'static str) {
            match self {
                ExportCopy::Pdf => ("PDF", "doc.richtext"),
                ExportCopy::Html => ("HTML", "chevron.left.forwardslash.chevron.right"),
                ExportCopy::Png => ("PNG", "photo"),
            }
        }
    }

    /// Export `note` off the main thread (`<note>.pdf`, `.html` and `.png`),
    /// copy the chosen file's absolute path and toast it. Shared by Cmd+P and
    /// the right-click "Copy PDF / HTML / PNG Path".
    ///
    /// The `EXPORTING` latch is reset on every path that armed it: a worker
    /// panic is caught and reported like a failed export, so one bad render
    /// cannot wedge every later attempt in this process (PLAN-prose-popout H3).
    pub fn export_pdf_and_copy(note: String, cfg: NotesConfig, colors: PopupColors, copy_toast: &str, which: ExportCopy) {
        use std::sync::atomic::{AtomicBool, Ordering};
        static EXPORTING: AtomicBool = AtomicBool::new(false);
        if note.is_empty() || EXPORTING.swap(true, Ordering::SeqCst) {
            crate::app::logging::ws_log(&format!("prose pdf: skip (note empty or already exporting): {note}"));
            return;
        }
        let (kind, icon) = which.kind();
        // The configured copy toast is the PDF's (Cmd+P); the HTML / PNG rows
        // say which file's path landed on the clipboard.
        let fmt = match which {
            ExportCopy::Pdf if !copy_toast.is_empty() => copy_toast.to_string(),
            _ => format!("Copied {kind} path {{}}"),
        };
        crate::app::logging::ws_log(&format!("prose pdf: start {note} ({kind})"));
        crate::ui::toast::show(&format!("Generating {kind}…"), icon);
        std::thread::spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                ProseRender::export_pdf(&note, &cfg, &colors)
            }))
            .unwrap_or_else(|_| crate::views::notes::PdfExport {
                ok: false,
                out: None,
                error: Some("pdf export panicked".to_string()),
                html: None,
                png: None,
            });
            crate::app::logging::ws_log(&format!(
                "prose pdf: result ok={} out={:?} err={:?}",
                r.ok, r.out, r.error
            ));
            crate::platform::run_on_main(move || {
                EXPORTING.store(false, Ordering::SeqCst);
                let path = match which {
                    ExportCopy::Pdf => r.out,
                    ExportCopy::Html => r.html,
                    ExportCopy::Png => r.png,
                };
                match (r.ok, path) {
                    (true, Some(out)) => {
                        let _ = crate::platform::set_text(&out);
                        crate::ui::toast::show(&fmt.replace("{}", &tilde(&out)), icon);
                    }
                    (true, None) => crate::ui::toast::show(
                        &match which {
                            ExportCopy::Png => "PNG not created (needs poppler: brew install poppler)".to_string(),
                            _ => format!("{kind} not created"),
                        },
                        "exclamationmark.triangle.fill",
                    ),
                    _ => crate::ui::toast::show(
                        r.error.as_deref().unwrap_or("pdf export failed"),
                        "exclamationmark.triangle.fill",
                    ),
                }
            });
        });
    }

    fn tilde(path: &str) -> String {
        let home = std::env::var("HOME").unwrap_or_default();
        match path.strip_prefix(&home) {
            Some(rest) if !home.is_empty() => format!("~{rest}"),
            _ => path.to_string(),
        }
    }

    /// The right-click menu of both prose views: copy the note's absolute
    /// path, or build its PDF and copy that path.
    ///
    /// Rows come from the one shared definition ([`notes::prose_menu_rows`] /
    /// [`notes::NOTE_MENU_ORDER`]) so wording and order cannot drift from the
    /// editor and vim menus. No reveal/open-in-notes row: this menu also runs
    /// in the pop-out child process, where those are a silent no-op
    /// (`UI_WEAK` is only set inside a `DaemonUi`).
    pub fn show_context_menu(note: &str, colors: PopupColors, view: &NSView) {
        use crate::ui::menu_kit::{popup_context, MenuEntry};
        use crate::views::notes::{prose_menu_rows, NoteMenuRow};
        if note.is_empty() {
            return;
        }
        // Breadcrumb for the pop-out child (PLAN-prose-popout Step B):
        // `popup_context` needs the right-click as `currentEvent`; a nil one
        // means the menu would open nothing at all.
        if let Some(mtm) = MainThreadMarker::new() {
            let event = objc2_app_kit::NSApplication::sharedApplication(mtm).currentEvent();
            crate::app::logging::ws_log(&format!(
                "prose menu: {note} (currentEvent {})",
                if event.is_some() { "ok" } else { "nil" }
            ));
        }
        let entries: Vec<MenuEntry> = prose_menu_rows()
            .into_iter()
            .map(|row| match row {
                NoteMenuRow::CopyAbsolutePath => {
                    let n = note.to_string();
                    MenuEntry::item(row.label(), move || {
                        crate::app::logging::ws_log("prose menu: picked copy path");
                        let _ = crate::platform::set_text(&n);
                        crate::ui::toast::show(&format!("Copied {}", tilde(&n)), "doc.on.clipboard");
                    })
                }
                NoteMenuRow::CopyPdfPath | NoteMenuRow::CopyHtmlPath | NoteMenuRow::CopyPngPath => {
                    let n = note.to_string();
                    let which = match row {
                        NoteMenuRow::CopyHtmlPath => ExportCopy::Html,
                        NoteMenuRow::CopyPngPath => ExportCopy::Png,
                        _ => ExportCopy::Pdf,
                    };
                    MenuEntry::item(row.label(), move || {
                        crate::app::logging::ws_log(&format!("prose menu: picked copy {which:?} path"));
                        export_pdf_and_copy(n, NotesConfig::load(), colors, "", which);
                    })
                }
                _ => unreachable!("prose_menu_rows yields only copy rows"),
            })
            .collect();
        popup_context(entries, view);
    }

    pub struct WebIvars {
        on_menu: RefCell<Option<Rc<dyn Fn(&NSView)>>>,
    }

    define_class!(
        /// The prose `WKWebView`: right-click shows our menu, not WebKit's.
        #[unsafe(super(WKWebView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSProseWebView"]
        #[ivars = WebIvars]
        pub struct ProseWebView;

        impl ProseWebView {
            #[unsafe(method(rightMouseDown:))]
            fn right_mouse_down(&self, e: &objc2_app_kit::NSEvent) {
                let cb = self.ivars().on_menu.borrow().clone();
                match cb {
                    Some(cb) => cb(self),
                    None => {
                        let _: () = unsafe { msg_send![super(self), rightMouseDown: e] };
                    }
                }
            }
        }
    );

    impl ProseWebView {
        pub fn new(mtm: MainThreadMarker, frame: NSRect, config: &WKWebViewConfiguration) -> Retained<ProseWebView> {
            let this = ProseWebView::alloc(mtm).set_ivars(WebIvars { on_menu: RefCell::new(None) });
            unsafe { msg_send![super(this), initWithFrame: frame, configuration: config] }
        }

        pub fn set_on_menu(&self, cb: Rc<dyn Fn(&NSView)>) {
            *self.ivars().on_menu.borrow_mut() = Some(cb);
        }
    }

    /// Give a pop-out's configuration the `/` find bar: the page script plus
    /// the `wsSearch` channel the key state machine listens to.
    pub fn add_find(mtm: MainThreadMarker, config: &WKWebViewConfiguration, on_msg: Box<dyn Fn(ProseMsg)>) -> Retained<ProseMsgHandler> {
        let ucc = unsafe { config.userContentController() };
        let sink: Sink = Rc::new(RefCell::new(Some(on_msg)));
        let handler = ProseMsgHandler::alloc(mtm).set_ivars(HandlerIvars { sink });
        let handler: Retained<ProseMsgHandler> = unsafe { msg_send![super(handler), init] };
        let s = unsafe {
            WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
                WKUserScript::alloc(mtm),
                &NSString::from_str(FIND_JS),
                WKUserScriptInjectionTime::AtDocumentEnd,
                true,
            )
        };
        unsafe {
            ucc.addUserScript(&s);
            ucc.addScriptMessageHandler_name(ProtocolObject::from_ref(&*handler), &NSString::from_str("wsSearch"));
        }
        handler
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_are_unescaped_swift() {
        assert!(FIND_JS.contains("__wsFindOpen"));
        assert!(FIND_JS.contains(r"/[.*+?^${}()|[\]\\]/g, '\\$&'"));
        assert!(OUTLINE_JS.contains("'\\u2261'"));
        assert!(OUTLINE_JS.contains("wsJump"));
        assert!(COPY_BUTTON_JS.contains("__wsAddCopyButtons"));
    }

    #[test]
    fn land_script_spliced_before_body_end() {
        let html = "<html><body><p>x</p></BODY></html>";
        let out = splice_before_body_end(html, &land_script(LandAt::Line(12)));
        assert!(out.contains("<script>window.addEventListener('load'"));
        assert!(out.find("<script>").unwrap() < out.find("</BODY>").unwrap());
        assert!(out.contains("})(12)"));
        assert!(splice_before_body_end("<p>", "X").ends_with('X'));
        assert!(land_script(LandAt::Y(240.0)).contains("window.scrollTo(0,240)"));
    }

    #[test]
    fn show_keys_head_ignores_text() {
        let (a, ha) = show_keys("/n.md", 19.0, "F", 900.0, "s", true, "one");
        let (b, hb) = show_keys("/n.md", 19.0, "F", 900.0, "s", true, "two");
        assert_ne!(a, b);
        assert_eq!(ha, hb);
        let (_, hc) = show_keys("/n.md", 19.0, "F", 700.0, "s", true, "two");
        assert_ne!(ha, hc);
    }
}
