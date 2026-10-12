//! `FilePopup.swift` — the floating image / text preview window ("Open Full
//! Size") shared by the notes editor's image attachments, the reading view's
//! image double-click, the vim pane's inline images and the screenshot
//! recents menu.
//!
//! Routing is [`FilePopup::route`]: an existing image file shows in the
//! zoomable window, a short text file shows as selectable monospaced text,
//! anything else opens in the default app.
//!
//! The panel mirrors Swift `FilePopupPanel` / `FilePopupGrip` /
//! `FilePopupRing`: borderless, non-activating but key-capable, floating,
//! joins the active space, resizable from 8 edge/corner grips, `Cmd`+`±`/`0`
//! zoom, `Cmd+C`/`Cmd+A` on text, Esc (or losing key) closes it. One popup at
//! a time — a new one replaces the old.

use std::path::Path;

/// `FilePopup.imageExts` — the extensions the popup treats as images.
pub const FILE_POPUP_IMAGE_EXTS: [&str; 9] = [
    "png", "jpg", "jpeg", "gif", "heic", "webp", "tif", "tiff", "bmp",
];

/// `FilePopup.textLimit` (512 KB).
pub const FILE_POPUP_TEXT_LIMIT: u64 = 512 * 1024;

/// `FilePopup.open` routing decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FilePopupRoute {
    Image,
    Text(String),
    System,
}

pub struct FilePopup;

impl FilePopup {
    pub fn is_image(path: &str) -> bool {
        let ext = Path::new(path)
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        FILE_POPUP_IMAGE_EXTS.contains(&ext.as_str())
    }

    /// `FilePopup.previewText` — up to `limit` bytes, NUL = binary, UTF-8 with
    /// incremental truncation for a cut multibyte tail.
    pub fn preview_text(path: &str, limit: u64) -> Option<String> {
        use std::io::Read;
        let meta = std::fs::metadata(path).ok()?;
        if meta.is_dir() {
            return None;
        }
        let f = std::fs::File::open(path).ok()?;
        let mut data = Vec::new();
        f.take(limit + 4).read_to_end(&mut data).ok()?;
        if data.contains(&0) {
            return None;
        }
        let cut = data.len() as u64 > limit;
        let mut body = if cut {
            data[..limit as usize].to_vec()
        } else {
            data
        };
        let mut text = String::from_utf8(body.clone()).ok();
        let mut trimmed = 0;
        while text.is_none() && trimmed < 3 && !body.is_empty() {
            body.pop();
            trimmed += 1;
            text = String::from_utf8(body.clone()).ok();
        }
        let text = text?;
        if cut {
            Some(format!(
                "{text}\n\n… (preview stops at {} KB)",
                limit / 1024
            ))
        } else {
            Some(text)
        }
    }

    /// `FilePopup.open` routing decision.
    pub fn route(path: &str) -> FilePopupRoute {
        if Self::is_image(path) && std::fs::metadata(path).map(|m| m.is_file()).unwrap_or(false) {
            FilePopupRoute::Image
        } else if !Self::is_image(path) {
            if let Some(text) = Self::preview_text(path, FILE_POPUP_TEXT_LIMIT) {
                return FilePopupRoute::Text(text);
            }
            FilePopupRoute::System
        } else {
            FilePopupRoute::System
        }
    }
}

/// `FilePopup.open(path, over:)` — show the routed popup (main thread).
#[cfg(target_os = "macos")]
pub fn show(path: &str, over: Option<&objc2_app_kit::NSWindow>) {
    appkit::show(path, over);
}

/// Whether the preview popup is up.
#[cfg(target_os = "macos")]
pub fn is_live() -> bool {
    appkit::is_live()
}

#[cfg(not(target_os = "macos"))]
pub fn is_live() -> bool {
    false
}

/// No popup off macOS.
#[cfg(not(target_os = "macos"))]
pub fn show(_path: &str, _over: Option<&()>) {}

// ---------------------------------------------------------------------------
// The live window (macOS)
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod appkit {
    use std::cell::RefCell;
    use std::rc::Rc;

    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, ProtocolObject};
    use objc2::{
        define_class, msg_send, sel, AnyThread, ClassType, DefinedClass, MainThreadMarker,
        MainThreadOnly, Message,
    };
    use objc2_app_kit::{
        NSAutoresizingMaskOptions, NSBackingStoreType, NSBorderType, NSCursor, NSEvent,
        NSEventMask, NSEventType, NSFloatingWindowLevel, NSImage, NSImageScaling, NSImageView,
        NSPanel, NSScreen, NSScrollView, NSScrollerStyle, NSTextView, NSView, NSWindow,
        NSWindowCollectionBehavior, NSWindowDelegate, NSWindowStyleMask,
    };
    use objc2_foundation::{
        NSNotification, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
    };

    use super::{FilePopup, FilePopupRoute};
    use crate::ui::style;

    /// `FilePopupGrip.size` / `.edge`.
    const GRIP_SIZE: f64 = 22.0;
    const GRIP_EDGE: f64 = 8.0;

    /// The one live popup (Swift's `FilePopup.panel` static).
    struct LivePopup {
        panel: Retained<FilePopupPanel>,
    }

    thread_local! {
        static LIVE: RefCell<Option<LivePopup>> = const { RefCell::new(None) };
    }

    pub fn is_live() -> bool {
        LIVE.with(|l| l.borrow().as_ref().is_some_and(|p| p.panel.isVisible()))
    }

    /// `FilePopup.close()` — order out the live popup, if any.
    pub fn close() {
        LIVE.with(|l| {
            if let Some(live) = l.borrow_mut().take() {
                live.panel.orderOut(None);
            }
        });
    }

    /// `FilePopup.open` routing.
    pub fn show(path: &str, over: Option<&NSWindow>) {
        match FilePopup::route(path) {
            FilePopupRoute::Image => {
                let Some(img) = load_image(path) else {
                    open_in_workspace(path);
                    return;
                };
                show_image(img, over);
            }
            FilePopupRoute::Text(text) => show_text(&text, over),
            FilePopupRoute::System => open_in_workspace(path),
        }
    }

    fn open_in_workspace(path: &str) {
        crate::platform::open_path(std::path::Path::new(path));
    }

    fn load_image(path: &str) -> Option<Retained<NSImage>> {
        let img = NSImage::initWithContentsOfFile(NSImage::alloc(), &NSString::from_str(path))?;
        let s = img.size();
        (s.width > 0.0 && s.height > 0.0).then_some(img)
    }

    /// `FilePopup.room(for:on:)` — the visible frame inset 24pt.
    fn room_for(over: Option<&NSWindow>) -> NSRect {
        let Some(mtm) = MainThreadMarker::new() else {
            return NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1440.0, 900.0));
        };
        let screen = over
            .and_then(|w| w.screen())
            .or_else(|| NSScreen::mainScreen(mtm));
        let f = screen.map(|s| s.visibleFrame()).unwrap_or_else(|| {
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1440.0, 900.0))
        });
        NSRect::new(
            NSPoint::new(f.origin.x + 24.0, f.origin.y + 24.0),
            NSSize::new((f.size.width - 48.0).max(1.0), (f.size.height - 48.0).max(1.0)),
        )
    }

    /// `FilePopup.zoomScroll` — the magnifying scroll view around `doc`.
    fn zoom_scroll(mtm: MainThreadMarker, doc: &NSView, frame: NSRect) -> Retained<NSScrollView> {
        let sv = NSScrollView::new(mtm);
        sv.setFrame(frame);
        sv.setDocumentView(Some(doc));
        sv.setAllowsMagnification(true);
        sv.setMinMagnification(0.25);
        sv.setMaxMagnification(8.0);
        sv.setHasVerticalScroller(true);
        sv.setHasHorizontalScroller(true);
        sv.setAutohidesScrollers(true);
        sv.setScrollerStyle(NSScrollerStyle::Overlay);
        sv.setDrawsBackground(false);
        sv.setBorderType(NSBorderType::NoBorder);
        sv.setWantsLayer(true);
        if let Some(layer) = sv.layer() {
            layer.setCornerRadius(8.0);
            layer.setMasksToBounds(true);
        }
        sv
    }

    /// `FilePopup.show(image:label:over:)`.
    fn show_image(img: Retained<NSImage>, over: Option<&NSWindow>) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let is = img.size();
        let room = room_for(over);
        let scale = 1.0f64
            .min(room.size.width / is.width)
            .min(room.size.height / is.height);
        let size = NSSize::new((is.width * scale).floor(), (is.height * scale).floor());
        if size.width <= 0.0 || size.height <= 0.0 {
            return;
        }
        let iv = NSImageView::initWithFrame(
            NSImageView::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), size),
        );
        iv.setImage(Some(&img));
        iv.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
        let frame = NSRect::new(
            NSPoint::new(
                room.origin.x + room.size.width / 2.0 - size.width / 2.0,
                room.origin.y + room.size.height / 2.0 - size.height / 2.0,
            ),
            size,
        );
        let scroll = zoom_scroll(mtm, &iv, NSRect::new(NSPoint::new(0.0, 0.0), size));
        present(mtm, scroll, frame, over, None);
    }

    /// `FilePopup.showText(_:path:over:)`.
    fn show_text(text: &str, over: Option<&NSWindow>) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let colors = crate::ui::theme::section_colors("notes");
        let room = room_for(over);
        let tv = NSTextView::initWithFrame(
            NSTextView::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(600.0, 400.0)),
        );
        tv.setEditable(false);
        tv.setSelectable(true);
        tv.setRichText(false);
        tv.setDrawsBackground(false);
        tv.setFont(Some(&style::mono_font(12.0, style::Weight::Regular)));
        tv.setTextColor(Some(&colors.text.to_nscolor()));
        tv.setInsertionPointColor(Some(&objc2_app_kit::NSColor::clearColor()));
        tv.setTextContainerInset(NSSize::new(14.0, 12.0));
        tv.setHorizontallyResizable(true);
        tv.setVerticallyResizable(true);
        tv.setMaxSize(NSSize::new(f64::MAX, f64::MAX));
        let Some(tc) = (unsafe { tv.textContainer() }) else { return };
        tc.setWidthTracksTextView(false);
        tc.setContainerSize(NSSize::new(f64::MAX, f64::MAX));
        tv.setString(&NSString::from_str(text));
        let Some(lm) = (unsafe { tv.layoutManager() }) else { return };
        lm.ensureLayoutForTextContainer(&tc);
        let used = lm.usedRectForTextContainer(&tc);
        let w = room.size.width.min(980.0f64.min((used.size.width + 40.0).max(420.0)));
        let h = (room.size.height * 0.9).min((used.size.height + 30.0).max(160.0));
        tv.setFrameSize(NSSize::new(
            w.max(used.size.width + 30.0),
            used.size.height + 24.0,
        ));
        let frame = NSRect::new(
            NSPoint::new(
                room.origin.x + room.size.width / 2.0 - w / 2.0,
                room.origin.y + room.size.height / 2.0 - h / 2.0,
            ),
            NSSize::new(w, h),
        );
        let scroll = zoom_scroll(mtm, &tv, NSRect::new(NSPoint::new(0.0, 0.0), frame.size));
        scroll.setDrawsBackground(true);
        scroll.setBackgroundColor(&colors.background.with_alpha(1.0).to_nscolor());
        present(mtm, scroll, frame, over, Some(tv));
    }

    /// `FilePopup.present` — the panel + ring + grips around `scroll`.
    fn present(
        mtm: MainThreadMarker,
        scroll: Retained<NSScrollView>,
        frame: NSRect,
        parent: Option<&NSWindow>,
        text: Option<Retained<NSTextView>>,
    ) {
        close();
        let panel = FilePopupPanel::create(mtm, frame);
        let box_view = NSView::initWithFrame(NSView::alloc(mtm), NSRect::new(NSPoint::new(0.0, 0.0), frame.size));
        scroll.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), frame.size));
        scroll.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        box_view.addSubview(&scroll);

        let mut settings = crate::app::config::AppSettings::default();
        crate::app::config::apply_app_config_from_disk(&mut settings);
        if settings.preview_border_width > 0.0 {
            let border = crate::ui::theme::Rgba::new(
                settings.preview_border.r,
                settings.preview_border.g,
                settings.preview_border.b,
                settings.preview_border.a,
            );
            let ring = FilePopupRing::create(
                mtm,
                NSRect::new(NSPoint::new(0.0, 0.0), frame.size),
                border,
                settings.preview_border_width,
            );
            ring.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            box_view.addSubview(&ring);
        }

        let g = GRIP_SIZE;
        let e = GRIP_EDGE;
        let (w, h) = (frame.size.width, frame.size.height);
        let handles: [(i32, i32, NSRect); 8] = [
            (-1, 0, NSRect::new(NSPoint::new(0.0, g), NSSize::new(e, (h - 2.0 * g).max(0.0)))),
            (1, 0, NSRect::new(NSPoint::new(w - e, g), NSSize::new(e, (h - 2.0 * g).max(0.0)))),
            (0, -1, NSRect::new(NSPoint::new(g, 0.0), NSSize::new((w - 2.0 * g).max(0.0), e))),
            (0, 1, NSRect::new(NSPoint::new(g, h - e), NSSize::new((w - 2.0 * g).max(0.0), e))),
            (-1, -1, NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(g, g))),
            (1, -1, NSRect::new(NSPoint::new(w - g, 0.0), NSSize::new(g, g))),
            (-1, 1, NSRect::new(NSPoint::new(0.0, h - g), NSSize::new(g, g))),
            (1, 1, NSRect::new(NSPoint::new(w - g, h - g), NSSize::new(g, g))),
        ];
        for (dx, dy, r) in handles {
            let grip = FilePopupGrip::create(mtm, dx, dy, &scroll, r);
            box_view.addSubview(&grip);
        }

        panel.setContentView(Some(&box_view));
        *panel.ivars().zoom.borrow_mut() = Some(scroll);
        *panel.ivars().text.borrow_mut() = text.clone();

        // `p.onEscape`: close, then give the parent the keyboard back.
        let parent_ret: Option<Retained<NSWindow>> = parent.map(|p| p.retain());
        let parent_for_escape = parent_ret.clone();
        *panel.ivars().on_escape.borrow_mut() = Some(Rc::new(move || {
            close();
            if let Some(p) = &parent_for_escape {
                p.makeKeyWindow();
            }
        }));

        // `NSWindow.didResignKeyNotification` → close (the delegate form).
        let delegate: &ProtocolObject<dyn NSWindowDelegate> = ProtocolObject::from_ref(&*panel);
        panel.setDelegate(Some(delegate));

        panel.orderFrontRegardless();
        panel.makeKeyWindow();
        if let Some(tv) = text {
            panel.makeFirstResponder(Some(&tv));
        }
        LIVE.with(|l| *l.borrow_mut() = Some(LivePopup { panel }));
    }

    // -- panel --------------------------------------------------------------

    pub struct FilePopupPanelIvars {
        on_escape: RefCell<Option<Rc<dyn Fn()>>>,
        zoom: RefCell<Option<Retained<NSScrollView>>>,
        text: RefCell<Option<Retained<NSTextView>>>,
    }

    define_class!(
        /// `FilePopupPanel` — borderless, non-activating, key-capable.
        #[unsafe(super(NSPanel))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSFilePopupPanel"]
        #[ivars = FilePopupPanelIvars]
        pub struct FilePopupPanel;

        impl FilePopupPanel {
            #[unsafe(method(canBecomeKeyWindow))]
            fn can_become_key(&self) -> bool {
                true
            }

            #[unsafe(method(canBecomeMainWindow))]
            fn can_become_main(&self) -> bool {
                false
            }

            #[unsafe(method(cancelOperation:))]
            fn cancel_operation(&self, _sender: Option<&AnyObject>) {
                self.escape();
            }

            #[unsafe(method(keyDown:))]
            fn key_down(&self, e: &NSEvent) {
                if e.keyCode() == 53 {
                    self.escape();
                } else {
                    let _: () = unsafe { msg_send![super(self), keyDown: e] };
                }
            }

            #[unsafe(method(performKeyEquivalent:))]
            fn perform_key_equivalent(&self, event: &NSEvent) -> objc2::runtime::Bool {
                use objc2_app_kit::NSEventModifierFlags as F;
                let flags = event.modifierFlags() & F::DeviceIndependentFlagsMask;
                if flags == F::Command {
                    if let Some(sv) = self.ivars().zoom.borrow().as_ref() {
                        let b = sv.contentView().bounds().size;
                        let center = NSPoint::new(b.width / 2.0, b.height / 2.0);
                        let ch = event.charactersIgnoringModifiers().map(|s| s.to_string());
                        match ch.as_deref() {
                            Some("=") | Some("+") => {
                                let m = (sv.magnification() * 1.25).min(sv.maxMagnification());
                                sv.setMagnification_centeredAtPoint(m, center);
                            }
                            Some("-") => {
                                let m = (sv.magnification() / 1.25).max(sv.minMagnification());
                                sv.setMagnification_centeredAtPoint(m, center);
                            }
                            Some("0") => sv.setMagnification_centeredAtPoint(1.0, center),
                            Some("w") => self.escape(),
                            Some("c") => {
                                if let Some(tv) = self.ivars().text.borrow().as_ref() {
                                    unsafe { tv.copy(None::<&AnyObject>) };
                                }
                            }
                            Some("a") => {
                                if let Some(tv) = self.ivars().text.borrow().as_ref() {
                                    unsafe { tv.selectAll(None::<&AnyObject>) };
                                }
                            }
                            _ => {
                                return unsafe {
                                    msg_send![super(self), performKeyEquivalent: event]
                                }
                            }
                        }
                        return true.into();
                    }
                }
                unsafe { msg_send![super(self), performKeyEquivalent: event] }
            }

            /// `FilePopupPanel.sendEvent`: grips resize, the top strip (or any
            /// spot without a text view) drags the window.
            #[unsafe(method(sendEvent:))]
            fn send_event(&self, event: &NSEvent) {
                if event.r#type() == NSEventType::LeftMouseDown {
                    if let Some(box_view) = self.contentView() {
                        let p = event.locationInWindow();
                        let w = box_view.bounds().size.width;
                        let h = box_view.bounds().size.height;
                        let corner = (p.x < GRIP_SIZE || p.x > w - GRIP_SIZE)
                            && (p.y < GRIP_SIZE || p.y > h - GRIP_SIZE);
                        let lim = if corner { GRIP_SIZE } else { GRIP_EDGE };
                        let dx = if p.x < lim {
                            -1
                        } else if p.x > w - lim {
                            1
                        } else {
                            0
                        };
                        let dy = if p.y < lim {
                            -1
                        } else if p.y > h - lim {
                            1
                        } else {
                            0
                        };
                        if dx != 0 || dy != 0 {
                            if let Some(grip) = find_grip(&box_view, dx, dy) {
                                grip.mouse_down(sel!(mouseDown:), event);
                                while let Some(n) = self.nextEventMatchingMask(
                                    NSEventMask::LeftMouseDragged | NSEventMask::LeftMouseUp,
                                ) {
                                    if n.r#type() == NSEventType::LeftMouseUp {
                                        grip.mouse_up(sel!(mouseUp:), &n);
                                        break;
                                    }
                                    grip.mouse_dragged(sel!(mouseDragged:), &n);
                                }
                                return;
                            }
                        }
                        let has_text = self.ivars().text.borrow().is_some();
                        if !has_text || p.y > h - 26.0 {
                            self.performWindowDragWithEvent(event);
                            return;
                        }
                    }
                }
                let _: () = unsafe { msg_send![super(self), sendEvent: event] };
            }

            #[unsafe(method(windowDidResignKey:))]
            fn window_did_resign_key(&self, _n: &NSNotification) {
                // Deferred: never release the panel inside its own delegate
                // callback (`close()` drops the last reference).
                crate::platform::run_on_main(close);
            }
        }

        unsafe impl NSObjectProtocol for FilePopupPanel {}
        unsafe impl NSWindowDelegate for FilePopupPanel {}
    );

    impl FilePopupPanel {
        fn create(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(FilePopupPanelIvars {
                on_escape: RefCell::new(None),
                zoom: RefCell::new(None),
                text: RefCell::new(None),
            });
            let panel: Retained<Self> = unsafe {
                msg_send![
                    super(this),
                    initWithContentRect: frame,
                    styleMask: NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
                    backing: NSBackingStoreType::Buffered,
                    defer: false
                ]
            };
            panel.setFloatingPanel(true);
            panel.setLevel(NSFloatingWindowLevel);
            panel.setHidesOnDeactivate(false);
            panel.setHasShadow(true);
            panel.setOpaque(false);
            panel.setBackgroundColor(Some(&objc2_app_kit::NSColor::clearColor()));
            panel.setCollectionBehavior(
                NSWindowCollectionBehavior::MoveToActiveSpace
                    | NSWindowCollectionBehavior::FullScreenAuxiliary,
            );
            unsafe { panel.setReleasedWhenClosed(false) };
            panel
        }

        fn escape(&self) {
            let cb = self.ivars().on_escape.borrow().clone();
            if let Some(cb) = cb {
                let Some(mtm) = MainThreadMarker::new() else { return };
                // Deferred: `cb` closes (and releases) this panel.
                let bound = dispatch2::MainThreadBound::new(cb, mtm);
                crate::platform::run_on_main(move || {
                    let Some(mtm) = MainThreadMarker::new() else { return };
                    let cb = bound.get(mtm);
                    cb();
                });
            }
        }
    }

    /// The grip at the (dx, dy) edge/corner of `box_view`'s subviews.
    fn find_grip(box_view: &NSView, dx: i32, dy: i32) -> Option<Retained<FilePopupGrip>> {
        for v in box_view.subviews().iter() {
            if let Some(grip) = v.downcast_ref::<FilePopupGrip>() {
                if grip.ivars().dx == dx && grip.ivars().dy == dy {
                    return Some(grip.retain());
                }
            }
        }
        None
    }

    // -- grip ---------------------------------------------------------------

    struct GripStart {
        frame: NSRect,
        mouse: NSPoint,
        mag: f64,
        anchor: NSPoint,
    }

    pub struct FilePopupGripIvars {
        dx: i32,
        dy: i32,
        scroll: Retained<NSScrollView>,
        start: RefCell<Option<GripStart>>,
    }

    define_class!(
        /// `FilePopupGrip` — one resize handle; corners scale, edges resize.
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSFilePopupGrip"]
        #[ivars = FilePopupGripIvars]
        pub struct FilePopupGrip;

        impl FilePopupGrip {
            #[unsafe(method(acceptsFirstMouse:))]
            fn accepts_first_mouse(&self, _e: Option<&NSEvent>) -> bool {
                true
            }

            #[unsafe(method(mouseDownCanMoveWindow))]
            fn mouse_down_can_move_window(&self) -> bool {
                false
            }

            #[allow(deprecated)]
            #[unsafe(method(resetCursorRects))]
            fn reset_cursor_rects(&self) {
                let (dx, dy) = (self.ivars().dx, self.ivars().dy);
                let cursor = if dx == 0 {
                    NSCursor::resizeUpDownCursor()
                } else if dy == 0 {
                    NSCursor::resizeLeftRightCursor()
                } else {
                    diagonal_cursor(dx * dy > 0).unwrap_or_else(NSCursor::arrowCursor)
                };
                self.addCursorRect_cursor(self.bounds(), &cursor);
            }

            #[unsafe(method(mouseDown:))]
            fn mouse_down(&self, _e: &NSEvent) {
                let Some(w) = self.window() else { return };
                let sv = &self.ivars().scroll;
                let mut anchor = NSPoint::new(0.0, 0.0);
                if let Some(doc) = sv.documentView() {
                    let b = doc.bounds();
                    if b.size.width > 0.0 && b.size.height > 0.0 {
                        let vis = sv.documentVisibleRect();
                        anchor = NSPoint::new(
                            vis.origin.x / b.size.width,
                            ((b.size.height - (vis.origin.y + vis.size.height)).max(0.0))
                                / b.size.height,
                        );
                    }
                }
                *self.ivars().start.borrow_mut() = Some(GripStart {
                    frame: w.frame(),
                    mouse: NSEvent::mouseLocation(),
                    mag: sv.magnification(),
                    anchor,
                });
            }

            #[unsafe(method(mouseDragged:))]
            fn mouse_dragged(&self, _e: &NSEvent) {
                let Some(w) = self.window() else { return };
                let Some(st) = self.ivars().start.borrow().as_ref().map(|s| {
                    (s.frame, s.mouse, s.mag, s.anchor)
                }) else {
                    return;
                };
                let (frame, mouse0, mag0, anchor) = st;
                if frame.size.width <= 0.0 || frame.size.height <= 0.0 {
                    return;
                }
                let m = NSEvent::mouseLocation();
                let dx = f64::from(self.ivars().dx);
                let dy = f64::from(self.ivars().dy);
                let dw = dx * (m.x - mouse0.x);
                let dh = dy * (m.y - mouse0.y);
                let visible = w
                    .screen()
                    .map(|s| s.visibleFrame())
                    .unwrap_or(frame);
                let mut size = frame.size;
                let mut k = 1.0;
                if dx != 0.0 && dy != 0.0 {
                    let mut kk = ((frame.size.width + dw) / frame.size.width)
                        .max((frame.size.height + dh) / frame.size.height);
                    kk = kk
                        .min(visible.size.width / frame.size.width)
                        .min(visible.size.height / frame.size.height);
                    kk = kk.max((120.0 / frame.size.width).max(80.0 / frame.size.height));
                    let sv = &self.ivars().scroll;
                    kk = kk
                        .min(sv.maxMagnification() / mag0)
                        .max(sv.minMagnification() / mag0);
                    size = NSSize::new(frame.size.width * kk, frame.size.height * kk);
                    k = kk;
                } else {
                    if dx != 0.0 {
                        size.width = (frame.size.width + dw).min(visible.size.width).max(120.0);
                    }
                    if dy != 0.0 {
                        size.height = (frame.size.height + dh).min(visible.size.height).max(80.0);
                    }
                }
                let x = if dx < 0.0 {
                    frame.origin.x + frame.size.width - size.width
                } else {
                    frame.origin.x
                };
                let y = if dy < 0.0 {
                    frame.origin.y + frame.size.height - size.height
                } else {
                    frame.origin.y
                };
                w.setFrame_display(NSRect::new(NSPoint::new(x, y), size), true);
                let sv = &self.ivars().scroll;
                if dx != 0.0 && dy != 0.0 {
                    sv.setMagnification(mag0 * k);
                } else if let Some(iv) = sv.documentView() {
                    // Image mode: refit the document to the window's aspect
                    // and keep the anchored point.
                    if let Ok(iv) = iv.downcast::<NSImageView>() {
                        if let Some(img) = iv.image() {
                            let is = img.size();
                            if is.width > 0.0 && is.height > 0.0 {
                                let s = (size.width / is.width).max(size.height / is.height);
                                let doc = NSSize::new(
                                    size.width.max((is.width * s).round()),
                                    size.height.max((is.height * s).round()),
                                );
                                sv.setMagnification(1.0);
                                iv.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), doc));
                                let clip = sv.contentView();
                                let x = (anchor.x * doc.width)
                                    .clamp(0.0, (doc.width - size.width).max(0.0));
                                let top = anchor.y * doc.height;
                                let y = ((doc.height - top - size.height).max(0.0))
                                    .min((doc.height - size.height).max(0.0));
                                clip.scrollToPoint(NSPoint::new(x, y));
                                sv.reflectScrolledClipView(&clip);
                            }
                        }
                    }
                }
            }

            #[unsafe(method(mouseUp:))]
            fn mouse_up(&self, _e: &NSEvent) {
                *self.ivars().start.borrow_mut() = None;
            }
        }

        unsafe impl NSObjectProtocol for FilePopupGrip {}
    );

    impl FilePopupGrip {
        fn create(
            mtm: MainThreadMarker,
            dx: i32,
            dy: i32,
            scroll: &NSScrollView,
            frame: NSRect,
        ) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(FilePopupGripIvars {
                dx,
                dy,
                scroll: scroll.retain(),
                start: RefCell::new(None),
            });
            let grip: Retained<Self> = unsafe {
                msg_send![super(this), initWithFrame: frame]
            };
            // `FilePopupGrip.init`: stick the grip to its edge/corner as the
            // window resizes.
            let mut mask = NSAutoresizingMaskOptions::ViewNotSizable;
            if dx == 0 {
                mask |= NSAutoresizingMaskOptions::ViewWidthSizable;
            } else if dx < 0 {
                mask |= NSAutoresizingMaskOptions::ViewMaxXMargin;
            } else {
                mask |= NSAutoresizingMaskOptions::ViewMinXMargin;
            }
            if dy == 0 {
                mask |= NSAutoresizingMaskOptions::ViewHeightSizable;
            } else if dy < 0 {
                mask |= NSAutoresizingMaskOptions::ViewMaxYMargin;
            } else {
                mask |= NSAutoresizingMaskOptions::ViewMinYMargin;
            }
            grip.setAutoresizingMask(mask);
            grip
        }
    }

    /// The private diagonal resize cursors, falling back to the arrow.
    fn diagonal_cursor(ne_sw: bool) -> Option<Retained<NSCursor>> {
        let s = if ne_sw {
            sel!(_windowResizeNorthEastSouthWestCursor)
        } else {
            sel!(_windowResizeNorthWestSouthEastCursor)
        };
        let cls = NSCursor::class();
        let responds: bool = unsafe { msg_send![cls, respondsToSelector: s] };
        if !responds {
            return None;
        }
        let ptr: *mut AnyObject = unsafe { msg_send![cls, performSelector: s] };
        if ptr.is_null() {
            return None;
        }
        unsafe { Retained::retain(ptr) }.map(|o| unsafe { Retained::cast_unchecked::<NSCursor>(o) })
    }

    // -- ring ---------------------------------------------------------------

    pub struct FilePopupRingIvars;

    define_class!(
        /// `FilePopupRing` — the themed border; click-through.
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSFilePopupRing"]
        #[ivars = FilePopupRingIvars]
        pub struct FilePopupRing;

        impl FilePopupRing {
            #[unsafe(method(hitTest:))]
            fn hit_test(&self, _point: NSPoint) -> *mut NSView {
                std::ptr::null_mut()
            }
        }

        unsafe impl NSObjectProtocol for FilePopupRing {}
    );

    impl FilePopupRing {
        fn create(
            mtm: MainThreadMarker,
            frame: NSRect,
            color: crate::ui::theme::Rgba,
            width: f64,
        ) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(FilePopupRingIvars);
            let ring: Retained<Self> = unsafe {
                msg_send![super(this), initWithFrame: frame]
            };
            ring.setWantsLayer(true);
            if let Some(layer) = ring.layer() {
                layer.setCornerRadius(8.0);
                layer.setBorderWidth(width);
                layer.setBorderColor(Some(&color.to_nscolor().CGColor()));
            }
            ring
        }
    }
}
