//! The live AI view — the AppKit half of Swift's `AIWindow` (AIWindow.swift):
//! the RULES sidebar, `AICommandLine` ("$ fm respond …"), `AIPipelineStrip`
//! (the chained rule chips), the "Your text" / "Changes" panes with the mode
//! segments + "⧉ Copy for …", and the footer (Run ⌃↩ · status · tokens).
//! The model stays in [`super::ai::AIWindowModel`].

use crate::ui::sidebar::{SidebarEvent, SidebarModel};

pub const CMD_H: f64 = 22.0;
pub const STRIP_H: f64 = 28.0;
pub const FOOT_H: f64 = 44.0;
pub const HEAD_H: f64 = 32.0;
pub const PAD: f64 = 12.0;
pub const GAP: f64 = 10.0;

/// `AIPipelineStrip.State`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepState {
    Waiting,
    Running,
    Done,
    Warn,
    Failed,
}

/// One output run: `(kind, text)` with kind -1 deleted, 0 same, 1 inserted.
pub type OutRun = (i8, String);

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AISnapshot {
    pub sidebar: SidebarModel,
    pub command: String,
    pub warning: String,
    pub steps: Vec<(String, StepState)>,
    pub right_title: String,
    pub modes: Vec<String>,
    pub mode: usize,
    pub copy_title: String,
    pub placeholder: String,
    /// The output pane (None → the web preview shows instead).
    pub output: Option<Vec<OutRun>>,
    pub running: bool,
    pub status: String,
    pub tone: u8,
    pub tokens: String,
    pub tokens_warn: bool,
    /// Set the input text (rule switch / restore) — `None` leaves it alone.
    pub set_input: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AIEvent {
    Sidebar(SidebarEvent),
    InputChanged(String),
    Mode(usize),
    Copy,
    Run,
    CopyCommand,
}

pub const C_MODE: i32 = 10;
pub const C_COPY: i32 = 30;
pub const C_RUN: i32 = 31;
pub const C_CMD: i32 = 32;

#[cfg(target_os = "macos")]
pub use appkit::AIView;

#[cfg(target_os = "macos")]
mod appkit {
    use super::*;
    use crate::ui::chrome::Rect;
    use crate::ui::sidebar::SidebarView;
    use crate::ui::style::{self, ButtonState, Weight};
    use crate::ui::theme::{PopupColors, PopupTone, Rgba};
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, ProtocolObject};
    use objc2::{define_class, msg_send, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, Message};
    use objc2_app_kit::{
        NSBackgroundColorAttributeName, NSColor, NSControlSize, NSEvent, NSFont, NSFontAttributeName,
        NSForegroundColorAttributeName, NSProgressIndicator, NSProgressIndicatorStyle, NSScrollView,
        NSStrikethroughColorAttributeName, NSStrikethroughStyleAttributeName, NSTextDelegate, NSTextView,
        NSTextViewDelegate, NSTrackingArea, NSTrackingAreaOptions, NSView,
    };
    use objc2_foundation::{
        NSAttributedString, NSAttributedStringKey, NSDictionary, NSMutableAttributedString, NSNotification,
        NSNumber, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
    };
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    fn as_any<T: Message + ?Sized>(obj: &T) -> &AnyObject {
        unsafe { &*(obj as *const T as *const AnyObject) }
    }

    fn ns(r: Rect) -> NSRect {
        style::ns(r)
    }

    type Sink = Rc<dyn Fn(AIEvent)>;

    fn tracking(view: &NSView) -> Retained<NSTrackingArea> {
        let opts = NSTrackingAreaOptions::MouseMoved
            | NSTrackingAreaOptions::MouseEnteredAndExited
            | NSTrackingAreaOptions::ActiveAlways
            | NSTrackingAreaOptions::InVisibleRect;
        let t = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(NSTrackingArea::alloc(), view.bounds(), opts, Some(as_any(view)), None)
        };
        view.addTrackingArea(&t);
        t
    }

    fn text_view(mtm: MainThreadMarker, colors: &PopupColors, editable: bool) -> (Retained<NSScrollView>, Retained<NSTextView>) {
        let scroll = NSScrollView::new(mtm);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setDrawsBackground(false);
        let tv = NSTextView::new(mtm);
        tv.setEditable(editable);
        tv.setSelectable(true);
        tv.setRichText(false);
        tv.setDrawsBackground(false);
        tv.setTextContainerInset(NSSize::new(10.0, 8.0));
        tv.setFont(Some(&NSFont::systemFontOfSize(14.0)));
        tv.setTextColor(Some(&colors.text.to_nscolor()));
        tv.setInsertionPointColor(Some(&colors.accent_on().to_nscolor()));
        tv.setVerticallyResizable(true);
        tv.setHorizontallyResizable(false);
        tv.setAutoresizingMask(objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable);
        unsafe {
            tv.setAutomaticQuoteSubstitutionEnabled(false);
            tv.setAutomaticDashSubstitutionEnabled(false);
        }
        if let Some(tc) = unsafe { tv.textContainer() } {
            tc.setWidthTracksTextView(true);
        }
        scroll.setDocumentView(Some(&tv));
        (scroll, tv)
    }

    pub struct AIIvars {
        snap: RefCell<AISnapshot>,
        colors: Cell<PopupColors>,
        sidebar: Retained<SidebarView>,
        in_scroll: Retained<NSScrollView>,
        input: Retained<NSTextView>,
        out_scroll: Retained<NSScrollView>,
        output: Retained<NSTextView>,
        web: Retained<NSView>,
        spinner: Retained<NSProgressIndicator>,
        sink: RefCell<Option<Sink>>,
        hover: Cell<Option<i32>>,
        pressed: Cell<Option<i32>>,
        tracking: RefCell<Option<Retained<NSTrackingArea>>>,
        sidebar_w: Cell<f64>,
        split: Cell<f64>,
        shown_out: RefCell<Option<Vec<OutRun>>>,
    }

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSAIView"]
        #[ivars = AIIvars]
        pub struct AIView;

        impl AIView {

            #[unsafe(method(isFlipped))]
            fn is_flipped(&self) -> bool { true }

            #[unsafe(method(setFrameSize:))]
            fn set_frame_size(&self, size: NSSize) {
                let _: () = unsafe { msg_send![super(self), setFrameSize: size] };
                self.layout_all();
            }

            #[unsafe(method(updateTrackingAreas))]
            fn uta(&self) {
                let _: () = unsafe { msg_send![super(self), updateTrackingAreas] };
                if let Some(t) = self.ivars().tracking.borrow_mut().take() {
                    self.removeTrackingArea(&t);
                }
                *self.ivars().tracking.borrow_mut() = Some(tracking(self));
            }

            #[unsafe(method(mouseMoved:))]
            fn mouse_moved(&self, e: &NSEvent) {
                let p = self.convertPoint_fromView(e.locationInWindow(), None);
                let h = self.hit(p.x, p.y);
                let tip = h.and_then(|id| match id {
                    C_CMD => Some("The command this rule runs — click to copy it (with your text) for a shell".to_string()),
                    C_RUN => Some("Run the rule on your text (Ctrl+Enter)".into()),
                    C_COPY => Some("Copy as rich text for the target".into()),
                    _ => None,
                });
                let cur: Option<String> = self.toolTip().map(|t| t.to_string());
                if cur != tip {
                    self.setToolTip(tip.as_deref().map(NSString::from_str).as_deref());
                }
                if h != self.ivars().hover.get() {
                    self.ivars().hover.set(h);
                    self.setNeedsDisplay(true);
                }
            }

            #[unsafe(method(mouseExited:))]
            fn mouse_exited(&self, _e: &NSEvent) {
                self.ivars().hover.set(None);
                self.setNeedsDisplay(true);
            }

            #[unsafe(method(mouseDown:))]
            fn mouse_down(&self, e: &NSEvent) {
                let p = self.convertPoint_fromView(e.locationInWindow(), None);
                self.ivars().pressed.set(self.hit(p.x, p.y));
                self.setNeedsDisplay(true);
            }

            #[unsafe(method(mouseUp:))]
            fn mouse_up(&self, e: &NSEvent) {
                let p = self.convertPoint_fromView(e.locationInWindow(), None);
                let h = self.hit(p.x, p.y);
                let was = self.ivars().pressed.replace(None);
                self.setNeedsDisplay(true);
                let Some(id) = h.filter(|_| h == was) else { return };
                let ev = match id {
                    C_COPY => AIEvent::Copy,
                    C_RUN => AIEvent::Run,
                    C_CMD => AIEvent::CopyCommand,
                    id if id >= C_MODE && id < C_MODE + 10 => AIEvent::Mode((id - C_MODE) as usize),
                    _ => return,
                };
                self.emit(ev);
            }

            #[unsafe(method(drawRect:))]
            fn draw_rect(&self, _dirty: NSRect) {
                self.draw();
            }

            #[unsafe(method(textView:doCommandBySelector:))]
            fn do_command(&self, _tv: &NSTextView, cmd: objc2::runtime::Sel) -> objc2::runtime::Bool {
                if cmd == objc2::sel!(insertLineBreak:) || cmd == objc2::sel!(insertNewlineIgnoringFieldEditor:) {
                    self.emit(AIEvent::Run);
                    return objc2::runtime::Bool::YES;
                }
                if cmd == objc2::sel!(cancelOperation:) && self.ivars().snap.borrow().running {
                    self.emit(AIEvent::Run);
                    return objc2::runtime::Bool::YES;
                }
                objc2::runtime::Bool::NO
            }

            #[unsafe(method(textDidChange:))]
            fn text_did_change(&self, _n: &NSNotification) {
                let s = self.ivars().input.string().to_string();
                self.emit(AIEvent::InputChanged(s));
                self.setNeedsDisplay(true);
            }
        }

        unsafe impl NSObjectProtocol for AIView {}
        unsafe impl NSTextDelegate for AIView {}
        unsafe impl NSTextViewDelegate for AIView {}
    );

    impl AIView {
        /// `AIWindow.navPanes`: sidebar, input, answer (web or text).
        pub fn nav_panes(&self) -> Vec<crate::app::live_panes::LivePane> {
            use crate::app::live_panes::LivePane;
            let iv = self.ivars();
            let mut out = vec![
                LivePane::new("sidebar", &iv.sidebar),
                LivePane::new("input", &iv.in_scroll).focusing(&iv.input),
            ];
            if !iv.web.isHidden() {
                out.push(LivePane::new("answer", &iv.web));
            } else {
                out.push(LivePane::new("answer", &iv.out_scroll).focusing(&iv.output));
            }
            out
        }
        pub fn new(mtm: MainThreadMarker, colors: PopupColors, web: Retained<NSView>, sidebar_w: f64, split: f64) -> Retained<Self> {
            let sidebar = SidebarView::new(mtm, colors);
            sidebar.set_collapse_key("ai");
            let (in_scroll, input) = text_view(mtm, &colors, true);
            let (out_scroll, output) = text_view(mtm, &colors, false);
            let spinner = NSProgressIndicator::new(mtm);
            spinner.setStyle(NSProgressIndicatorStyle::Spinning);
            spinner.setControlSize(NSControlSize::Small);
            spinner.setDisplayedWhenStopped(false);
            let this = Self::alloc(mtm).set_ivars(AIIvars {
                snap: RefCell::new(AISnapshot::default()),
                colors: Cell::new(colors),
                sidebar,
                in_scroll,
                input,
                out_scroll,
                output,
                web,
                spinner,
                sink: RefCell::new(None),
                hover: Cell::new(None),
                pressed: Cell::new(None),
                tracking: RefCell::new(None),
                sidebar_w: Cell::new(sidebar_w),
                split: Cell::new(split),
                shown_out: RefCell::new(None),
            });
            let v: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
            let iv = v.ivars();
            for sub in [&*iv.sidebar as &NSView, &iv.in_scroll, &iv.out_scroll, &iv.web, &iv.spinner] {
                v.addSubview(sub);
            }
            unsafe { iv.input.setDelegate(Some(ProtocolObject::from_ref(&*v))) };
            v
        }

        pub fn set_on_event(&self, cb: Box<dyn Fn(AIEvent)>) {
            let rc: Sink = Rc::from(cb);
            *self.ivars().sink.borrow_mut() = Some(rc.clone());
            self.ivars().sidebar.set_on_event(Box::new(move |e| rc(AIEvent::Sidebar(e))));
        }

        fn emit(&self, ev: AIEvent) {
            let s = self.ivars().sink.borrow().clone();
            if let Some(s) = s {
                s(ev);
            }
        }

        pub fn input_view(&self) -> Retained<NSTextView> {
            self.ivars().input.clone()
        }

        pub fn input_text(&self) -> String {
            self.ivars().input.string().to_string()
        }

        pub fn apply(&self, mut s: AISnapshot) {
            let iv = self.ivars();
            iv.sidebar.set_model(s.sidebar.clone());
            if let Some(t) = s.set_input.take() {
                iv.input.setString(&NSString::from_str(&t));
                iv.input.setFont(Some(&NSFont::systemFontOfSize(14.0)));
                iv.input.setTextColor(Some(&iv.colors.get().text.to_nscolor()));
            }
            if s.running {
                unsafe { iv.spinner.startAnimation(None) };
            } else {
                unsafe { iv.spinner.stopAnimation(None) };
            }
            let show_web = s.output.is_none();
            iv.out_scroll.setHidden(show_web);
            iv.web.setHidden(!show_web);
            if *iv.shown_out.borrow() != s.output {
                *iv.shown_out.borrow_mut() = s.output.clone();
                if let Some(runs) = &s.output {
                    self.set_output(runs);
                }
            }
            *iv.snap.borrow_mut() = s;
            self.layout_all();
            self.setNeedsDisplay(true);
        }

        /// `renderAnswer()`'s attributed diff.
        fn set_output(&self, runs: &[OutRun]) {
            let c = self.ivars().colors.get();
            let font = NSFont::systemFontOfSize(14.0);
            let out = NSMutableAttributedString::new();
            let danger = c.tone(PopupTone::Danger);
            let success = c.tone(PopupTone::Success);
            for (kind, text) in runs {
                let fg: Retained<NSColor> = if *kind < 0 { danger.with_alpha(0.9).to_nscolor() } else { c.text.to_nscolor() };
                let mut keys: Vec<&NSAttributedStringKey> = unsafe { vec![NSFontAttributeName, NSForegroundColorAttributeName] };
                let mut objs: Vec<Retained<AnyObject>> = vec![
                    unsafe { Retained::cast_unchecked(font.clone()) },
                    unsafe { Retained::cast_unchecked(fg) },
                ];
                if *kind < 0 {
                    keys.push(unsafe { NSStrikethroughStyleAttributeName });
                    objs.push(unsafe { Retained::cast_unchecked(NSNumber::numberWithInt(1)) });
                    keys.push(unsafe { NSStrikethroughColorAttributeName });
                    objs.push(unsafe { Retained::cast_unchecked(danger.to_nscolor()) });
                    keys.push(unsafe { NSBackgroundColorAttributeName });
                    objs.push(unsafe { Retained::cast_unchecked(danger.with_alpha(0.16).to_nscolor()) });
                } else if *kind > 0 {
                    keys.push(unsafe { NSBackgroundColorAttributeName });
                    objs.push(unsafe { Retained::cast_unchecked(success.with_alpha(0.28).to_nscolor()) });
                }
                let refs: Vec<&AnyObject> = objs.iter().map(|o| &**o).collect();
                let attrs = NSDictionary::from_slices(&keys, &refs);
                let piece = unsafe {
                    NSAttributedString::initWithString_attributes(NSAttributedString::alloc(), &NSString::from_str(text), Some(&attrs))
                };
                out.appendAttributedString(&piece);
            }
            if let Some(ts) = unsafe { self.ivars().output.textStorage() } {
                ts.setAttributedString(&out);
            }
        }

        fn left(&self) -> f64 {
            self.ivars().sidebar.width(self.ivars().sidebar_w.get()) + 4.0
        }

        /// (cmd, strip, left box, right box, footer y) — `layoutAll`.
        fn regions(&self) -> (Rect, Rect, Rect, Rect, f64) {
            let b = self.bounds();
            let left = self.left();
            let w = (b.size.width - left).max(200.0);
            let cmd = Rect::new(left, 6.0, w, CMD_H);
            let strip = Rect::new(left + PAD, cmd.max_y() + 4.0, (w - PAD * 2.0).max(0.0), STRIP_H);
            let top = strip.max_y() + 4.0;
            let body_h = (b.size.height - top - FOOT_H).max(80.0);
            let avail = w - PAD * 2.0 - GAP;
            let lw = (avail * self.ivars().split.get()).round();
            let lbox = Rect::new(left + PAD, top, lw, body_h);
            let rbox = Rect::new(left + PAD + lw + GAP, top, avail - lw, body_h);
            let fy = top + body_h + (FOOT_H - 28.0) / 2.0;
            (cmd, strip, lbox, rbox, fy)
        }

        pub fn layout_all(&self) {
            let iv = self.ivars();
            let b = self.bounds();
            iv.sidebar.setFrame(ns(Rect::new(0.0, 0.0, iv.sidebar.width(iv.sidebar_w.get()), b.size.height)));
            let (_, _, lbox, rbox, fy) = self.regions();
            iv.in_scroll.setFrame(ns(Rect::new(lbox.x + 2.0, lbox.y + HEAD_H, lbox.width - 4.0, lbox.height - HEAD_H - 2.0)));
            let out = Rect::new(rbox.x + 2.0, rbox.y + HEAD_H, rbox.width - 4.0, rbox.height - HEAD_H - 2.0);
            iv.out_scroll.setFrame(ns(out));
            iv.web.setFrame(ns(out));
            let run = self.controls().into_iter().find(|(i, _)| *i == C_RUN).map(|(_, r)| r).unwrap_or(Rect::ZERO);
            iv.spinner.setFrame(ns(Rect::new(run.max_x() + 10.0, fy + 6.0, 16.0, 16.0)));
        }

        fn controls(&self) -> Vec<(i32, Rect)> {
            let mut out = Vec::new();
            let (cmd, _, _, rbox, fy) = self.regions();
            out.push((C_CMD, cmd));
            let snap = self.ivars().snap.borrow();
            let bf = style::system_font(12.0, Weight::Medium);
            let cw = style::text_size(&snap.copy_title, &style::system_font(12.0, Weight::Semibold)).0.ceil() + 26.0;
            let mut x = rbox.max_x() - 8.0 - cw;
            out.push((C_COPY, Rect::new(x, rbox.y + (HEAD_H - 24.0) / 2.0, cw, 24.0)));
            let geo = style::CAPSULE_GEO;
            let (widths, total) = style::capsule_metrics(&snap.modes, &bf);
            x -= 6.0 + total;
            let mut sx = x + geo.track_pad;
            for (i, w) in widths.iter().enumerate() {
                out.push((C_MODE + i as i32, Rect::new(sx, rbox.y + (HEAD_H - 26.0) / 2.0 + geo.track_pad, *w, 20.0)));
                sx += w + geo.gutter;
            }
            let title = if snap.running { "Stop  ⌘." } else { "Run  ⌃↩" };
            let rw = style::text_size(title, &style::system_font(13.0, Weight::Semibold)).0.ceil() + 40.0;
            out.push((C_RUN, Rect::new(self.left() + PAD, fy, rw, 28.0)));
            out
        }

        fn hit(&self, x: f64, y: f64) -> Option<i32> {
            self.controls().into_iter().find(|(_, r)| r.contains(x, y)).map(|(id, _)| id)
        }

        fn draw(&self) {
            let c = self.ivars().colors.get();
            let snap = self.ivars().snap.borrow();
            let (cmd, strip, lbox, rbox, fy) = self.regions();
            let controls = self.controls();
            let rect_of = |id: i32| controls.iter().find(|(i, _)| *i == id).map(|(_, r)| *r);
            let hover = self.ivars().hover.get();
            // Command line.
            let mono = style::mono_font(11.0, Weight::Regular);
            let (dw, dh) = style::text_size("$ ", &mono);
            let ty = cmd.mid_y() - dh / 2.0;
            style::draw_text("$ ", cmd.x + 14.0, ty, &mono, c.accent);
            let mut avail = cmd.width - 28.0 - dw;
            let mut warn_w = 0.0;
            if !snap.warning.is_empty() {
                let w = format!("   ⚠ {}", snap.warning);
                warn_w = style::text_size(&w, &mono).0.min(avail * 0.5);
                style::draw_text_right(&w, Rect::new(cmd.x + 14.0, cmd.y, cmd.width - 28.0, cmd.height), &mono, c.tone(PopupTone::Warning));
                avail -= warn_w;
            }
            style::draw_text_left(&snap.command, Rect::new(cmd.x + 14.0 + dw, cmd.y, avail.max(0.0), cmd.height), &mono, c.dim);
            let _ = warn_w;
            // Pipeline strip.
            let sf = style::system_font(11.5, Weight::Medium);
            let mut x = strip.x + 2.0;
            let h = 22.0;
            let y = strip.mid_y() - h / 2.0;
            for (i, (name, st)) in snap.steps.iter().enumerate() {
                let (glyph, tone): (String, Rgba) = match st {
                    StepState::Waiting => (format!("{}", i + 1), c.dim),
                    StepState::Running => ("…".into(), c.accent),
                    StepState::Done => ("✓".into(), c.tone(PopupTone::Success)),
                    StepState::Warn => ("!".into(), c.tone(PopupTone::Warning)),
                    StepState::Failed => ("✕".into(), c.tone(PopupTone::Danger)),
                };
                let label = format!("{glyph}  {name}");
                let (lw, lh) = style::text_size(&label, &sf);
                let w = lw + 20.0;
                if x + w > strip.max_x() {
                    break;
                }
                let r = Rect::new(x, y, w, h);
                style::fill_rounded(r, h / 2.0, tone.with_alpha(if *st == StepState::Waiting { 0.08 } else { 0.16 }));
                style::draw_text(&label, r.x + 10.0, r.mid_y() - lh / 2.0, &sf, if *st == StepState::Waiting { c.text.with_alpha(0.85) } else { tone });
                x = r.max_x();
                if i + 1 < snap.steps.len() {
                    let (aw, ah) = style::text_size("→", &sf);
                    style::draw_text("→", x + 6.0, y + (h - ah) / 2.0, &sf, c.dim);
                    x += 12.0 + aw;
                }
            }
            // Boxes.
            for (bx, title) in [(lbox, "Your text".to_string()), (rbox, snap.right_title.clone())] {
                style::fill_rounded(bx, 8.0, c.mantle().with_alpha(0.45));
                style::stroke_rounded(bx, 8.0, 1.0, c.hairline());
                let tf = style::system_font(11.0, Weight::Semibold);
                let (_, th) = style::text_size(&title, &tf);
                style::draw_text(&title, bx.x + 12.0, bx.y + (HEAD_H - th) / 2.0 + 1.0, &tf, c.text);
            }
            // Input placeholder.
            if self.ivars().input.string().length() == 0 && !snap.placeholder.is_empty() {
                let pf = style::system_font(14.0, Weight::Regular);
                style::draw_text(&snap.placeholder, lbox.x + 2.0 + 15.0, lbox.y + HEAD_H + 8.0, &pf, c.dim.with_alpha(0.8));
            }
            // Mode segments + copy.
            let bf = style::system_font(12.0, Weight::Medium);
            let geo = style::CAPSULE_GEO;
            if let (Some(a), Some(b)) = (rect_of(C_MODE), rect_of(C_MODE + snap.modes.len() as i32 - 1)) {
                style::capsule_track(
                    Rect::new(
                        a.x - geo.track_pad,
                        a.y - geo.track_pad,
                        b.max_x() - a.x + 2.0 * geo.track_pad,
                        a.height + 2.0 * geo.track_pad,
                    ),
                    &c,
                );
                for (i, m) in snap.modes.iter().enumerate() {
                    let r = rect_of(C_MODE + i as i32).unwrap();
                    let on = snap.mode == i;
                    let hov = hover == Some(C_MODE + i as i32);
                    style::capsule_chip(r, &c, on, hov);
                    style::draw_text_centered(m, r, &bf, if on { c.text } else if hov { c.text.with_alpha(0.9) } else { c.dim });
                }
            }
            if let Some(r) = rect_of(C_COPY) {
                let hov = hover == Some(C_COPY);
                style::stroke_rounded(r, r.height / 2.0, 1.0, c.text.with_alpha(0.25));
                if hov {
                    style::fill_rounded(r, r.height / 2.0, c.text.with_alpha(0.08));
                }
                style::draw_text_centered(&snap.copy_title, r, &style::system_font(12.0, Weight::Semibold), c.text.with_alpha(0.9));
            }
            // Footer.
            if let Some(r) = rect_of(C_RUN) {
                style::capsule_outline(r, &c, hover == Some(C_RUN) || self.ivars().pressed.get() == Some(C_RUN));
                let t = if snap.running { "Stop  ⌘." } else { "Run  ⌃↩" };
                style::draw_text_centered(t, r, &style::system_font(13.0, Weight::Semibold), c.accent);
                let tone = match snap.tone {
                    1 => c.tone(PopupTone::Success),
                    2 => c.tone(PopupTone::Warning),
                    3 => c.tone(PopupTone::Danger),
                    _ => c.text.with_alpha(0.85),
                };
                let tf = style::system_font(11.0, Weight::Regular);
                let tw = style::text_size(&snap.tokens, &tf).0.min(rbox.max_x() * 0.4);
                style::draw_text_right(&snap.tokens, Rect::new(rbox.max_x() - tw - 2.0, fy + 6.0, tw + 2.0, 16.0), &tf, if snap.tokens_warn { c.tone(PopupTone::Warning) } else { c.dim });
                let sx = r.max_x() + if snap.running { 32.0 } else { 10.0 };
                style::draw_text_left(&snap.status, Rect::new(sx, fy + 6.0, (rbox.max_x() - tw - 12.0 - sx).max(0.0), 16.0), &style::system_font(11.5, Weight::Regular), tone);
            }
            let _ = ButtonState::Idle;
        }
    }
}
