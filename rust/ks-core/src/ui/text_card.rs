//! The Copy Text result card: after OCR copies text, show *what* was copied so
//! a misread is caught before it is pasted.
//!
//! It never takes the keyboard on its own (Cmd+V into the app underneath must
//! paste straight away — "never steal focus"). It holds for a few seconds and
//! goes; hovering keeps it up. Clicking the text makes the card key so the
//! text can be fixed: Return (or clicking away) copies the edit, Esc closes,
//! Shift+Return is a new line. "Undo copy" puts back what the clipboard held
//! before the capture, unless something newer was copied since.
//!
//! One card at a time; a new one replaces the old. Main thread only.

/// Words and timing for one card (`[screenshot] text-card-*`).
#[derive(Clone, Debug, PartialEq)]
pub struct TextCardSpec {
    /// Header, e.g. "Copied 5 lines".
    pub title: String,
    /// Footer hint, e.g. "Click to fix · Return copies · Esc closes".
    pub hint: String,
    /// The undo button's label.
    pub undo_label: String,
    /// Toast after an undo.
    pub undone_toast: String,
    /// Toast when an edit was copied.
    pub edited_toast: String,
    /// Seconds before it hides by itself (not while hovered or edited).
    pub hold_secs: f64,
    /// Text lines shown before the box scrolls.
    pub max_lines: usize,
    /// Next-step buttons in the header, left of Undo copy: `(id, label)`.
    pub actions: Vec<(String, String)>,
}

impl Default for TextCardSpec {
    fn default() -> Self {
        TextCardSpec {
            title: "Copied".to_string(),
            hint: "Click to fix \u{b7} Return copies \u{b7} Esc closes".to_string(),
            undo_label: "Undo copy".to_string(),
            undone_toast: "Clipboard put back".to_string(),
            edited_toast: "Copied your edit".to_string(),
            hold_secs: 6.0,
            max_lines: 10,
            actions: Vec::new(),
        }
    }
}

/// The card's height for `lines` text lines (pure layout, unit-tested).
pub fn card_height(lines: usize, max_lines: usize) -> f64 {
    let shown = lines.clamp(1, max_lines.max(1)) as f64;
    HEADER_H + shown * LINE_H + TEXT_PAD * 2.0 + FOOTER_H
}

/// Character columns for tab stops so tab-separated OCR tables line up in the
/// card: each column as wide as its widest cell plus a 2-character gap.
/// Empty when no line has a tab (the default 8-column tabs are left alone).
pub fn tab_columns(text: &str) -> Vec<usize> {
    let mut widths: Vec<usize> = Vec::new();
    for line in text.lines().filter(|l| l.contains('\t')) {
        let cells: Vec<&str> = line.split('\t').collect();
        // The last cell needs no stop after it.
        for (i, c) in cells.iter().take(cells.len().saturating_sub(1)).enumerate() {
            let n = c.chars().count();
            if i >= widths.len() {
                widths.push(n);
            } else if n > widths[i] {
                widths[i] = n;
            }
        }
    }
    let mut at = 0;
    widths
        .into_iter()
        .map(|w| {
            at += w + 2;
            at
        })
        .collect()
}

pub const HEADER_H: f64 = 34.0;
pub const FOOTER_H: f64 = 26.0;
pub const LINE_H: f64 = 16.0;
pub const TEXT_PAD: f64 = 8.0;
pub const WIDTH: f64 = 560.0;

#[cfg(target_os = "macos")]
pub use appkit::{close, is_live, show, state};

#[cfg(not(target_os = "macos"))]
pub fn is_live() -> bool {
    false
}

#[cfg(target_os = "macos")]
mod appkit {
    use super::*;
    use crate::platform::clipboard::{self, Snapshot};
    use crate::ui::popup::MonitorHandle;
    use crate::ui::style::{self, Weight};
    use crate::ui::theme::PopupThemeDefaults;
    use crate::ui::view_base;
    use objc2::rc::Retained;
    use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSAutoresizingMaskOptions, NSBackingStoreType, NSColor, NSEvent, NSPanel, NSScrollView,
        NSTextView, NSView, NSWindowCollectionBehavior, NSWindowStyleMask,
    };
    use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};
    use std::cell::{Cell, RefCell};
    use std::time::{Duration, Instant};

    struct Live {
        panel: Retained<CardPanel>,
        text: Retained<NSTextView>,
        copied: String,
        before: Snapshot,
        spec: TextCardSpec,
        deadline: Instant,
        generation: u64,
        on_action: Option<Box<dyn Fn(&str, &str)>>,
        _keys: MonitorHandle,
    }

    thread_local! {
        static CARD: RefCell<Option<Live>> = const { RefCell::new(None) };
        static GENERATION: Cell<u64> = const { Cell::new(0) };
        /// What the last card did, for `state` (tests / agents).
        static LAST: RefCell<String> = const { RefCell::new(String::new()) };
    }

    define_class!(
        #[unsafe(super(NSPanel))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSTextCardPanel"]
        pub struct CardPanel;

        unsafe impl NSObjectProtocol for CardPanel {}

        impl CardPanel {
            // A borderless panel cannot become key by default; the card must,
            // once clicked, so its text can be edited.
            #[unsafe(method(canBecomeKeyWindow))]
            fn can_become_key(&self) -> bool {
                true
            }

            // Clicking away after an edit keeps (and copies) the edit.
            #[unsafe(method(resignKeyWindow))]
            fn resign_key(&self) {
                let _: () = unsafe { msg_send![super(self), resignKeyWindow] };
                let edited = CARD.with(|c| c.borrow().as_ref().is_some_and(|l| l.text.string().to_string() != l.copied));
                if edited {
                    dispatch_close(true);
                }
            }
        }
    );

    pub struct RootIvars {
        undo: Cell<NSRect>,
        close: Cell<NSRect>,
        /// 0 none, 1 undo, 2 close, 10 + i action i.
        hover: Cell<u8>,
        actions: RefCell<Vec<(String, String, NSRect)>>,
        tracking: RefCell<Option<Retained<objc2_app_kit::NSTrackingArea>>>,
        title: RefCell<String>,
        hint: RefCell<String>,
        undo_label: RefCell<String>,
    }

    define_class!(
        #[unsafe(super(view_base::WSFlippedView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSTextCardRoot"]
        #[ivars = RootIvars]
        pub struct CardRoot;

        impl CardRoot {
            #[unsafe(method(acceptsFirstMouse:))]
            fn accepts_first_mouse(&self, _e: Option<&NSEvent>) -> bool {
                true
            }

            #[unsafe(method(updateTrackingAreas))]
            fn uta(&self) {
                let _: () = unsafe { msg_send![super(self), updateTrackingAreas] };
                view_base::reset_tracking(self, &self.ivars().tracking, view_base::hover_opts());
            }

            #[unsafe(method(mouseMoved:))]
            fn mouse_moved(&self, e: &NSEvent) {
                let p = view_base::mouse_point(self, e);
                let act = self.ivars().actions.borrow().iter().position(|(_, _, r)| contains(*r, p));
                let h = if contains(self.ivars().undo.get(), p) {
                    1
                } else if contains(self.ivars().close.get(), p) {
                    2
                } else if let Some(i) = act {
                    10 + i as u8
                } else {
                    0
                };
                view_base::set_hover(self, &self.ivars().hover, h);
            }

            #[unsafe(method(mouseExited:))]
            fn mouse_exited(&self, _e: &NSEvent) {
                view_base::set_hover(self, &self.ivars().hover, 0);
            }

            #[unsafe(method(mouseDown:))]
            fn mouse_down(&self, _e: &NSEvent) {}

            #[unsafe(method(mouseUp:))]
            fn mouse_up(&self, e: &NSEvent) {
                let p = view_base::mouse_point(self, e);
                let act = self.ivars().actions.borrow().iter().find(|(_, _, r)| contains(*r, p)).map(|(id, _, _)| id.clone());
                if contains(self.ivars().undo.get(), p) {
                    dispatch_undo();
                } else if contains(self.ivars().close.get(), p) {
                    dispatch_close(true);
                } else if let Some(id) = act {
                    dispatch_action(id);
                }
            }

            #[unsafe(method(drawRect:))]
            fn draw_rect(&self, _dirty: NSRect) {
                self.draw();
            }
        }
    );

    fn contains(r: NSRect, p: NSPoint) -> bool {
        p.x >= r.origin.x && p.x <= r.origin.x + r.size.width && p.y >= r.origin.y && p.y <= r.origin.y + r.size.height
    }

    impl CardRoot {
        fn create(mtm: MainThreadMarker, frame: NSRect, spec: &TextCardSpec) -> Retained<CardRoot> {
            let this = CardRoot::alloc(mtm).set_ivars(RootIvars {
                undo: Cell::new(NSRect::ZERO),
                close: Cell::new(NSRect::ZERO),
                hover: Cell::new(0),
                tracking: RefCell::new(None),
                title: RefCell::new(spec.title.clone()),
                hint: RefCell::new(spec.hint.clone()),
                undo_label: RefCell::new(spec.undo_label.clone()),
                actions: RefCell::new(spec.actions.iter().map(|(i, l)| (i.clone(), l.clone(), NSRect::ZERO)).collect()),
            });
            unsafe { msg_send![super(this), initWithFrame: frame] }
        }

        /// Width left for the title once the header buttons are placed.
        fn title_room(&self, w: f64) -> f64 {
            let bf = style::system_font(12.0, Weight::Medium);
            let mut used = 34.0 + 10.0 + style::text_size(&self.ivars().undo_label.borrow(), &bf).0 + 22.0;
            for (_, l, _) in self.ivars().actions.borrow().iter() {
                used += style::text_size(l, &bf).0 + 30.0;
            }
            (w - 36.0 - used - 16.0).max(80.0)
        }

        fn draw(&self) {
            let c = PopupThemeDefaults::colors();
            let b = self.bounds();
            let (w, h) = (b.size.width, b.size.height);
            let card = crate::ui::chrome::Rect::new(0.5, 0.5, w - 1.0, h - 1.0);
            style::fill_rounded(card, 12.0, c.crust().with_alpha(0.97));
            style::stroke_rounded(card, 12.0, 1.0, c.text.with_alpha(0.14));

            // Header: icon + title on the left, [Undo copy] [×] on the right.
            let f = style::system_font(13.0, Weight::Semibold);
            if let Some(img) = style::symbol_image("text.viewfinder", 13.0, Weight::Medium, c.accent) {
                let s = img.size();
                img.drawInRect(NSRect::new(
                    NSPoint::new(14.0, (HEADER_H - s.height) / 2.0),
                    s,
                ));
            }
            style::draw_text_left(
                &self.ivars().title.borrow(),
                crate::ui::chrome::Rect::new(36.0, (HEADER_H - 17.0) / 2.0, self.title_room(w), 17.0),
                &f,
                c.text,
            );
            let bf = style::system_font(12.0, Weight::Medium);
            let label = self.ivars().undo_label.borrow().clone();
            let (lw, _) = style::text_size(&label, &bf);
            let close = NSRect::new(NSPoint::new(w - 34.0, 7.0), NSSize::new(22.0, 22.0));
            let undo = NSRect::new(NSPoint::new(close.origin.x - 10.0 - (lw + 22.0), 7.0), NSSize::new(lw + 22.0, 22.0));
            self.ivars().undo.set(undo);
            self.ivars().close.set(close);
            let hover = self.ivars().hover.get();
            let undo_r = style::rect_of(undo);
            style::fill_rounded(undo_r, 11.0, if hover == 1 { c.accent.with_alpha(0.28) } else { c.text.with_alpha(0.08) });
            style::stroke_rounded(undo_r, 11.0, 1.0, c.accent.with_alpha(0.6));
            style::draw_text_centered(&label, undo_r, &bf, c.text);
            let close_r = style::rect_of(close);
            if hover == 2 {
                style::fill_rounded(close_r, 11.0, c.text.with_alpha(0.12));
            }
            style::draw_text_centered("\u{2715}", close_r, &bf, c.dim);
            // Next-step buttons, right to left from Undo copy.
            let mut x = undo.origin.x - 8.0;
            let mut acts = self.ivars().actions.borrow_mut();
            for (i, (_, label, rect)) in acts.iter_mut().enumerate().rev() {
                let (lw, _) = style::text_size(label, &bf);
                let r = NSRect::new(NSPoint::new(x - (lw + 22.0), 7.0), NSSize::new(lw + 22.0, 22.0));
                *rect = r;
                x = r.origin.x - 8.0;
                let rr = style::rect_of(r);
                let on = hover == 10 + i as u8;
                style::fill_rounded(rr, 11.0, if on { c.accent.with_alpha(0.85) } else { c.accent.with_alpha(0.22) });
                style::draw_text_centered(label, rr, &bf, if on { c.crust() } else { c.text });
            }

            // Footer hint.
            let hf = style::system_font(11.0, Weight::Regular);
            style::draw_text_left(
                &self.ivars().hint.borrow(),
                crate::ui::chrome::Rect::new(14.0, h - FOOTER_H + 5.0, w - 28.0, 15.0),
                &hf,
                c.dim,
            );
        }
    }

    /// Show the card for `copied` (already on the clipboard); `before` is the
    /// clipboard as it was, with its `change_count` set to the count after
    /// our copy.
    pub fn show(copied: &str, before: Snapshot, spec: TextCardSpec, on_action: Option<Box<dyn Fn(&str, &str)>>) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        close_now(false);
        let Some(scr) = crate::platform::screen_under_mouse() else { return };
        let c = PopupThemeDefaults::colors();
        let lines = copied.lines().count();
        let w = WIDTH.min(scr.frame.w - 64.0);
        let h = card_height(lines, spec.max_lines);
        let rect = NSRect::new(
            NSPoint::new((scr.frame.x + scr.frame.w / 2.0 - w / 2.0).round(), scr.frame.y + 70.0),
            NSSize::new(w, h),
        );
        let panel: Retained<CardPanel> = unsafe {
            msg_send![
                CardPanel::alloc(mtm),
                initWithContentRect: rect,
                styleMask: NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
                backing: NSBackingStoreType::Buffered,
                defer: false
            ]
        };
        // Above the screenshot overlay level, like the toast.
        panel.setLevel(1001);
        panel.setOpaque(false);
        panel.setBackgroundColor(Some(&NSColor::clearColor()));
        panel.setHasShadow(true);
        panel.setHidesOnDeactivate(false);
        panel.setBecomesKeyOnlyIfNeeded(true);
        unsafe { panel.setReleasedWhenClosed(false) };
        panel.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::IgnoresCycle
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );

        let root = CardRoot::create(mtm, NSRect::new(NSPoint::ZERO, rect.size), &spec);
        let text_rect = NSRect::new(
            NSPoint::new(10.0, HEADER_H),
            NSSize::new(w - 20.0, h - HEADER_H - FOOTER_H),
        );
        let tv = NSTextView::initWithFrame(NSTextView::alloc(mtm), NSRect::new(NSPoint::ZERO, text_rect.size));
        tv.setEditable(true);
        tv.setSelectable(true);
        tv.setRichText(false);
        tv.setAllowsUndo(true);
        tv.setFont(Some(&style::mono_font(12.0, Weight::Regular)));
        tv.setTextColor(Some(&c.text.to_nscolor()));
        tv.setInsertionPointColor(Some(&c.accent.to_nscolor()));
        tv.setDrawsBackground(true);
        tv.setBackgroundColor(&c.text.with_alpha(0.05).to_nscolor());
        tv.setTextContainerInset(NSSize::new(6.0, TEXT_PAD - 2.0));
        tv.setVerticallyResizable(true);
        tv.setHorizontallyResizable(false);
        tv.setMaxSize(NSSize::new(f64::MAX, f64::MAX));
        tv.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        tv.setAutomaticQuoteSubstitutionEnabled(false);
        tv.setAutomaticDashSubstitutionEnabled(false);
        tv.setAutomaticTextReplacementEnabled(false);
        tv.setAutomaticSpellingCorrectionEnabled(false);
        if let Some(tc) = unsafe { tv.textContainer() } {
            tc.setWidthTracksTextView(true);
        }
        // Tab stops from the table's own columns (a monospace font), so an
        // OCR'd table reads as one in the card.
        let cols = tab_columns(copied);
        let para = (!cols.is_empty()).then(|| {
            use objc2::AnyThread;
            use objc2_app_kit::{NSMutableParagraphStyle, NSTextAlignment, NSTextTab};
            let (cw, _) = style::text_size("MMMMMMMMMM", &style::mono_font(12.0, Weight::Regular));
            let cw = cw / 10.0;
            let tabs: Vec<Retained<NSTextTab>> = cols
                .iter()
                .map(|c| unsafe {
                    NSTextTab::initWithTextAlignment_location_options(
                        NSTextTab::alloc(),
                        NSTextAlignment::Left,
                        *c as f64 * cw,
                        &objc2_foundation::NSDictionary::new(),
                    )
                })
                .collect();
            let para = NSMutableParagraphStyle::new();
            para.setTabStops(Some(&objc2_foundation::NSArray::from_retained_slice(&tabs)));
            para.setDefaultTabInterval(cw * 4.0);
            tv.setDefaultParagraphStyle(Some(&para));
            para
        });
        tv.setString(&NSString::from_str(copied));
        if let (Some(para), Some(store)) = (para.as_ref(), unsafe { tv.textStorage() }) {
            let len = store.length();
            let value: &objc2::runtime::AnyObject = para;
            unsafe {
                store.addAttribute_value_range(
                    objc2_app_kit::NSParagraphStyleAttributeName,
                    value,
                    objc2_foundation::NSRange::new(0, len),
                )
            };
        }
        let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), text_rect);
        scroll.setHasVerticalScroller(true);
        scroll.setDrawsBackground(false);
        scroll.setDocumentView(Some(&tv));
        if let Some(clip) = scroll.contentView().layer() {
            clip.setCornerRadius(6.0);
        }
        root.addSubview(&scroll);
        let root_view: &NSView = &root;
        panel.setContentView(Some(root_view));
        panel.orderFrontRegardless();

        let generation = GENERATION.with(|g| {
            g.set(g.get() + 1);
            g.get()
        });
        let keys = crate::ui::popup::install_local_monitor(mtm, move |event| card_key(event));
        CARD.with(|slot| {
            *slot.borrow_mut() = Some(Live {
                panel,
                text: tv,
                copied: copied.to_string(),
                before,
                deadline: Instant::now() + Duration::from_secs_f64(spec.hold_secs.max(1.0)),
                spec,
                generation,
                on_action,
                _keys: keys,
            });
        });
        LAST.with(|l| *l.borrow_mut() = "shown".to_string());
        schedule_tick(generation);
    }

    /// Return / Esc while the card is key. Edit shortcuts reach the text view
    /// through the host's `cmd_edit_in_popup` (rule 1).
    fn card_key(event: &NSEvent) -> bool {
        let Some(mtm) = MainThreadMarker::new() else { return false };
        let ours = CARD.with(|c| {
            c.borrow().as_ref().is_some_and(|l| {
                event.window(mtm).is_some_and(|w| std::ptr::eq(&*w as *const _ as *const u8, &*l.panel as *const _ as *const u8))
            })
        });
        if !ours {
            return false;
        }
        use objc2_app_kit::NSEventModifierFlags as F;
        let flags = event.modifierFlags();
        match event.keyCode() {
            53 => {
                dispatch_close(false);
                true
            }
            36 | 76 if !flags.contains(F::Shift) && !flags.contains(F::Option) => {
                dispatch_close(true);
                true
            }
            _ => false,
        }
    }

    fn schedule_tick(generation: u64) {
        let _ = dispatch2::DispatchQueue::main().after(
            dispatch2::DispatchTime::NOW.time(500_000_000),
            move || tick(generation),
        );
    }

    /// Hide once the hold is over, unless the pointer is on the card, it is
    /// key (being edited) or its text was changed.
    fn tick(generation: u64) {
        let keep = CARD.with(|c| {
            let c = c.borrow();
            let Some(l) = c.as_ref() else { return None };
            if l.generation != generation {
                return None;
            }
            let f = l.panel.frame();
            let m = NSEvent::mouseLocation();
            let hovered = m.x >= f.origin.x
                && m.x <= f.origin.x + f.size.width
                && m.y >= f.origin.y
                && m.y <= f.origin.y + f.size.height;
            let busy = hovered || l.panel.isKeyWindow() || l.text.string().to_string() != l.copied;
            Some(busy || Instant::now() < l.deadline)
        });
        match keep {
            Some(true) => schedule_tick(generation),
            Some(false) => close_now(false),
            None => {}
        }
    }

    // AppKit callbacks run inside the panel's own event handling: close on the
    // next turn of the run loop so the panel is not torn down mid-callback.
    fn dispatch_close(copy_edit: bool) {
        dispatch2::DispatchQueue::main().exec_async(move || close_now(copy_edit));
    }

    fn dispatch_action(id: String) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let id = dispatch2::MainThreadBound::new(id, mtm);
        dispatch2::DispatchQueue::main().exec_async(move || {
            if let Some(mtm) = MainThreadMarker::new() {
                action_now(id.get(mtm));
            }
        });
    }

    /// A next-step button: close the card (copying an edit first), then hand
    /// the text to the action.
    fn action_now(id: &str) {
        let taken = CARD.with(|c| c.borrow_mut().take());
        let Some(l) = taken else { return };
        let now = l.text.string().to_string();
        l.panel.orderOut(None);
        if now != l.copied && !now.is_empty() {
            clipboard::set_text(&now);
        }
        LAST.with(|s| *s.borrow_mut() = format!("action:{id}"));
        if let Some(f) = l.on_action.as_ref() {
            f(id, &now);
        }
    }

    fn dispatch_undo() {
        dispatch2::DispatchQueue::main().exec_async(undo_now);
    }

    fn undo_now() {
        let taken = CARD.with(|c| c.borrow_mut().take());
        let Some(l) = taken else { return };
        let ok = clipboard::restore_if_unchanged(&l.before);
        l.panel.orderOut(None);
        LAST.with(|s| *s.borrow_mut() = if ok { "undone" } else { "undo-skipped" }.to_string());
        if ok {
            crate::ui::toast::show(&l.spec.undone_toast, "arrow.uturn.backward");
        } else {
            crate::ui::toast::show("Not put back \u{2014} something newer was copied", "exclamationmark.triangle.fill");
        }
    }

    /// Close the card; `copy_edit` copies the text when it was changed.
    fn close_now(copy_edit: bool) {
        let taken = CARD.with(|c| c.borrow_mut().take());
        let Some(l) = taken else { return };
        let now = l.text.string().to_string();
        l.panel.orderOut(None);
        if copy_edit && now != l.copied && !now.is_empty() {
            clipboard::set_text(&now);
            crate::ui::toast::show(&l.spec.edited_toast, "checkmark.circle.fill");
            LAST.with(|s| *s.borrow_mut() = "edited".to_string());
        } else {
            LAST.with(|s| *s.borrow_mut() = "closed".to_string());
        }
    }

    /// Close without copying an edit (a new capture starting, the agent quitting).
    pub fn close() {
        close_now(false);
    }

    pub fn is_live() -> bool {
        CARD.with(|c| c.borrow().is_some())
    }

    /// `{"shown", "text", "key", "last"}` for socket state.
    pub fn state() -> serde_json::Value {
        let last = LAST.with(|l| l.borrow().clone());
        CARD.with(|c| match c.borrow().as_ref() {
            Some(l) => serde_json::json!({
                "shown": true,
                "text": l.text.string().to_string(),
                "key": l.panel.isKeyWindow(),
                "actions": l.spec.actions.iter().map(|(i, _)| i.clone()).collect::<Vec<_>>(),
                "last": last,
            }),
            None => serde_json::json!({"shown": false, "last": last}),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_stops_follow_the_widest_cell_per_column() {
        let t = "Service\tp50 ms\tOwner\nscreenshot\t38\tquick agent\nplain line";
        assert_eq!(tab_columns(t), vec![12, 20], "10+2, then 12 + 6+2");
        assert!(tab_columns("no tabs here").is_empty());
    }

    #[test]
    fn height_grows_with_lines_up_to_the_cap() {
        let one = card_height(1, 10);
        assert_eq!(card_height(0, 10), one, "an empty text still gets one line");
        assert_eq!(card_height(5, 10), one + 4.0 * LINE_H);
        assert_eq!(card_height(40, 10), card_height(10, 10), "long texts scroll");
    }
}
