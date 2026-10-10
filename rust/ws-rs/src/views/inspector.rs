//! Port of `PopupInspector.swift` — the read-only issue inspector panel.
//!
//! The content model, the attributed-text composition (`render()`) and the
//! layout are shared with the AppKit view tree built by [`PopupInspector::build_view`].

use crate::ui::chrome::Rect;
use crate::ui::theme::PopupTone;

#[cfg(target_os = "macos")]
use objc2::rc::Retained;
#[cfg(target_os = "macos")]
use objc2::MainThreadOnly;
#[cfg(target_os = "macos")]
use objc2_app_kit::NSView;

#[derive(Clone, Debug, PartialEq)]
pub struct InspectorChip {
    pub text: String,
    pub tone: PopupTone,
}

impl InspectorChip {
    pub fn new(text: impl Into<String>, tone: PopupTone) -> Self {
        InspectorChip {
            text: text.into(),
            tone,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct InspectorField {
    pub label: String,
    pub value: String,
}

impl InspectorField {
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        InspectorField {
            label: label.into(),
            value: value.into(),
        }
    }
}

/// `PopupInspectorContent`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PopupInspectorContent {
    pub key: String,
    pub title: String,
    pub chips: Vec<InspectorChip>,
    pub fields: Vec<InspectorField>,
    pub body: String,
}

impl PopupInspectorContent {
    pub fn new(
        key: impl Into<String>,
        title: impl Into<String>,
        chips: Vec<InspectorChip>,
        fields: Vec<InspectorField>,
        body: impl Into<String>,
    ) -> Self {
        PopupInspectorContent {
            key: key.into(),
            title: title.into(),
            chips,
            fields,
            body: body.into(),
        }
    }
}

/// The body is capped at 700 characters in `render()`.
pub const BODY_CAP: usize = 700;
pub const FOOTER_HEIGHT: f64 = 70.0;
pub const EMPTY_TEXT: &str = "Select an issue";

// `layout()` geometry (flipped, top-left origin).
pub const SCROLL_LEFT: f64 = 1.0;
pub const BUTTON_MIN_WIDTH: f64 = 160.0;
pub const BUTTON_HEIGHT: f64 = 32.0;
pub const BUTTON_TOP: f64 = 8.0;
pub const BUTTON_SIDE_INSET: f64 = 18.0;
pub const HINT_TOP: f64 = 42.0;
pub const HINT_HEIGHT: f64 = 16.0;
pub const HINT_INSET: f64 = 8.0;

/// `layout()` — the rects the AppKit surface positions its subviews with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InspectorLayout {
    pub scroll: Rect,
    pub button: Rect,
    pub hint: Rect,
}

pub fn inspector_layout(width: f64, height: f64) -> InspectorLayout {
    let footer = FOOTER_HEIGHT;
    let scroll = Rect::new(
        SCROLL_LEFT,
        0.0,
        (width - SCROLL_LEFT).max(0.0),
        (height - footer).max(0.0),
    );
    let bw = (width - BUTTON_SIDE_INSET * 2.0).max(BUTTON_MIN_WIDTH);
    let button = Rect::new((width - bw) / 2.0, height - footer + BUTTON_TOP, bw, BUTTON_HEIGHT);
    let hint = Rect::new(
        HINT_INSET,
        height - footer + HINT_TOP,
        (width - HINT_INSET * 2.0).max(0.0),
        HINT_HEIGHT,
    );
    InspectorLayout { scroll, button, hint }
}

/// One run of `render()`'s attributed text, in draw order.
#[derive(Clone, Debug, PartialEq)]
pub enum InspectorSegment {
    Key(String),
    Title(String),
    Chip { text: String, tone: PopupTone },
    Body(String),
    Field { label: String, value: String },
}

/// `render()`'s run list: key, title, chips (in order), the non-empty body,
/// then the fields with non-empty values.
pub fn inspector_segments(content: &PopupInspectorContent) -> Vec<InspectorSegment> {
    let mut out = vec![
        InspectorSegment::Key(content.key.clone()),
        InspectorSegment::Title(content.title.clone()),
    ];
    for chip in &content.chips {
        out.push(InspectorSegment::Chip { text: chip.text.clone(), tone: chip.tone });
    }
    if !content.body.is_empty() {
        out.push(InspectorSegment::Body(content.body.clone()));
    }
    for f in content.fields.iter().filter(|f| !f.value.is_empty()) {
        out.push(InspectorSegment::Field { label: f.label.clone(), value: f.value.clone() });
    }
    out
}

#[derive(Default)]
pub struct PopupInspector {
    pub content: Option<PopupInspectorContent>,
    pub empty_text: String,
    pub zoom: f64,
    pub on_open: Option<Box<dyn FnMut()>>,
    #[cfg(target_os = "macos")]
    pub view: Option<Retained<NSView>>,
}

impl PopupInspector {
    pub fn new() -> Self {
        PopupInspector {
            content: None,
            empty_text: EMPTY_TEXT.to_string(),
            zoom: 1.0,
            on_open: None,
            #[cfg(target_os = "macos")]
            view: None,
        }
    }

    pub fn set_content(&mut self, content: Option<PopupInspectorContent>) {
        self.content = content;
        #[cfg(target_os = "macos")]
        self.refresh_macos();
    }

    pub fn is_empty(&self) -> bool {
        self.content.is_none()
    }

    /// `var z = max(0.5, zoom())`.
    pub fn effective_zoom(&self) -> f64 {
        self.zoom.max(0.5)
    }

    /// The truncated body `render()` draws (`count > 700` → `prefix(700) + "…"`).
    pub fn body_text(&self) -> String {
        match &self.content {
            None => self.empty_text.clone(),
            Some(c) => {
                if c.body.chars().count() > BODY_CAP {
                    let prefix: String = c.body.chars().take(BODY_CAP).collect();
                    format!("{prefix}…")
                } else {
                    c.body.clone()
                }
            }
        }
    }

    /// The plain-text mirror of `render()`: key, title, chips, body, then the
    /// non-empty fields as `label\tvalue` rows.
    pub fn render_text(&self) -> Option<String> {
        let c = self.content.as_ref()?;
        let mut out = String::new();
        out.push_str(&c.key);
        out.push('\n');
        out.push_str(&c.title);
        out.push('\n');
        if !c.chips.is_empty() {
            for (i, chip) in c.chips.iter().enumerate() {
                out.push(' ');
                out.push_str(&chip.text);
                out.push(' ');
                if i + 1 < c.chips.len() {
                    out.push_str("  ");
                }
            }
            out.push('\n');
        }
        if !c.body.is_empty() {
            out.push_str(&self.body_text());
            out.push('\n');
        }
        for f in c.fields.iter().filter(|f| !f.value.is_empty()) {
            out.push_str(&f.label);
            out.push('\t');
            out.push_str(&f.value);
            out.push('\n');
        }
        Some(out)
    }

    pub fn open(&mut self) {
        if let Some(cb) = &mut self.on_open {
            cb();
        }
    }

    #[cfg(target_os = "macos")]
    pub fn view(&self) -> Option<&NSView> {
        self.view.as_deref()
    }

    #[cfg(not(target_os = "macos"))]
    pub fn view(&self) -> Option<&()> {
        None
    }

    /// Build the AppKit view. The panel is a flipped `NSView` holding a
    /// scrollable `NSTextView`, the "Open full detail" button and the hint.
    pub fn build_view(&mut self, mtm: objc2::MainThreadMarker) {
        #[cfg(target_os = "macos")]
        self.build_view_macos(mtm);
        #[cfg(not(target_os = "macos"))]
        let _ = mtm;
    }

    #[cfg(target_os = "macos")]
    fn build_view_macos(&mut self, mtm: objc2::MainThreadMarker) {
        if self.view.is_some() {
            return;
        }
        use crate::ui::theme::PopupThemeDefaults;

        let colors = PopupThemeDefaults::colors();
        let frame = objc2_foundation::NSRect::new(
            objc2_foundation::NSPoint::new(0.0, 0.0),
            objc2_foundation::NSSize::new(320.0, 420.0),
        );
        let this = macos::InspectorView::alloc(mtm)
            .set_ivars(macos::InspectorViewIvars { colors });
        let view: Retained<macos::InspectorView> =
            unsafe { objc2::msg_send![super(this), initWithFrame: frame] };
        view.setWantsLayer(true);

        let scroll = macos::make_scroll(mtm);
        let text = macos::make_text_view(mtm);
        scroll.setDocumentView(Some(&text));
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setDrawsBackground(false);
        view.addSubview(&scroll);

        let hint = macos::make_label(mtm, "⌘I hides this panel", &colors);
        view.addSubview(&hint);

        let handler = macos::InspectorHandler::new(mtm, self.on_open.take());
        let button = macos::make_open_button(mtm, &handler);
        view.addSubview(&button);

        // The view retains its subviews; the handler would otherwise be
        // released with the local, so keep it alive for the view's lifetime.
        macos::retain_handler(&handler);

        macos::apply_layout(&view, &scroll, &text, &button, &hint);
        macos::render(&text, self.content.as_ref(), &self.empty_text, self.effective_zoom(), &colors);
        button.setHidden(self.content.is_none());
        hint.setHidden(self.content.is_none());

        let view: Retained<NSView> = view.into_super();
        self.view = Some(view);
    }

    /// Re-render an already-built view (`content` didSet in Swift).
    #[cfg(target_os = "macos")]
    fn refresh_macos(&mut self) {
        if self.view.is_none() {
            return;
        }
        if let Some(mtm) = objc2::MainThreadMarker::new() {
            let colors = crate::ui::theme::PopupThemeDefaults::colors();
            macos::refresh(
                self.view.as_ref().unwrap(),
                self.content.as_ref(),
                &self.empty_text,
                self.effective_zoom(),
                &colors,
                mtm,
            );
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use crate::ui::theme::PopupColors;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject};
    use objc2::{
        define_class, msg_send, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, Message,
    };
    use objc2_app_kit::{
        NSBezierPath, NSButton, NSFont, NSFontWeightSemibold, NSMutableParagraphStyle,
        NSScrollView, NSTextAlignment, NSTextField, NSTextView, NSView,
    };
    use objc2_foundation::{
        NSAttributedStringKey, NSMutableAttributedString, NSObjectProtocol, NSPoint, NSRange,
        NSRect, NSSize, NSString,
    };

    // Attribute-key names (`NSAttributedStringKey` = `NSString`); the named
    // constants sit behind an objc2-foundation feature this crate omits.
    const FONT_KEY: &str = "NSFont";
    const COLOR_KEY: &str = "NSColor";
    const BG_KEY: &str = "NSBackgroundColor";
    const PARA_KEY: &str = "NSParagraphStyle";

    fn as_any<T: Message + ?Sized>(obj: &T) -> &AnyObject {
        unsafe { &*(obj as *const T as *const AnyObject) }
    }

    fn nsrect(r: Rect) -> NSRect {
        NSRect::new(NSPoint::new(r.x, r.y), NSSize::new(r.width, r.height))
    }

    /// One attributed run: font + color, optional chip background, spacing.
    fn run(
        text: &str,
        font: &NSFont,
        color: crate::ui::theme::Rgba,
        bg: Option<crate::ui::theme::Rgba>,
        spacing: f64,
        line: f64,
    ) -> Retained<NSMutableAttributedString> {
        let m = NSMutableAttributedString::initWithString(
            NSMutableAttributedString::alloc(),
            &NSString::from_str(text),
        );
        let range = NSRange::new(0, m.length());
        let style = NSMutableParagraphStyle::new();
        style.setParagraphSpacing(spacing);
        style.setLineSpacing(line);
        let font_key = NSString::from_str(FONT_KEY);
        let color_key = NSString::from_str(COLOR_KEY);
        let para_key = NSString::from_str(PARA_KEY);
        let bg_key = NSString::from_str(BG_KEY);
        let color_ns = color.to_nscolor();
        add_attr(&m, &font_key, as_any(&*font), range);
        add_attr(&m, &color_key, as_any(&*color_ns), range);
        add_attr(&m, &para_key, as_any(&*style), range);
        if let Some(bg) = bg {
            let bg_ns = bg.to_nscolor();
            add_attr(&m, &bg_key, as_any(&*bg_ns), range);
        }
        m
    }

    fn add_attr(
        m: &NSMutableAttributedString,
        name: &NSAttributedStringKey,
        value: &AnyObject,
        range: NSRange,
    ) {
        unsafe { m.addAttribute_value_range(name, value, range) };
    }

    fn append(acc: &NSMutableAttributedString, piece: &NSMutableAttributedString) {
        acc.appendAttributedString(piece);
    }

    /// `PopupInspectorView.render()`.
    pub fn render(
        text: &NSTextView,
        content: Option<&PopupInspectorContent>,
        empty_text: &str,
        zoom: f64,
        colors: &PopupColors,
    ) {
        let Some(store) = (unsafe { text.textStorage() }) else {
            return;
        };
        let z = zoom.max(0.5);
        let acc = NSMutableAttributedString::new();
        let Some(c) = content else {
            append(
                &acc,
                &run(
                    empty_text,
                    &NSFont::systemFontOfSize(13.0),
                    colors.dim,
                    None,
                    0.0,
                    0.0,
                ),
            );
            store.setAttributedString(&acc);
            return;
        };
        for seg in inspector_segments(c) {
            let piece = match seg {
                InspectorSegment::Key(s) => run(
                    &(s + "\n"),
                    &NSFont::systemFontOfSize_weight(12.5 * z, unsafe { NSFontWeightSemibold }),
                    colors.accent_on(),
                    None,
                    4.0,
                    0.0,
                ),
                InspectorSegment::Title(s) => run(
                    &(s + "\n"),
                    &NSFont::systemFontOfSize_weight(16.0 * z, unsafe { NSFontWeightSemibold }),
                    colors.text,
                    None,
                    10.0,
                    2.0,
                ),
                InspectorSegment::Chip { text, tone } => {
                    let c = colors.tone(tone);
                    run(
                        &format!(" {text} "),
                        &NSFont::systemFontOfSize_weight(12.0 * z, unsafe { NSFontWeightSemibold }),
                        c,
                        Some(c.with_alpha(0.18)),
                        12.0,
                        0.0,
                    )
                }
                InspectorSegment::Body(s) => run(
                    &(s + "\n"),
                    &NSFont::systemFontOfSize(13.0 * z),
                    colors.text,
                    None,
                    14.0,
                    3.0,
                ),
                InspectorSegment::Field { label, value } => run(
                    &format!("{label}\t{value}\n"),
                    &NSFont::systemFontOfSize(12.5 * z),
                    colors.text,
                    None,
                    5.0,
                    0.0,
                ),
            };
            append(&acc, &piece);
        }
        store.setAttributedString(&acc);
    }

    pub struct InspectorViewIvars {
        pub colors: PopupColors,
    }

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSInspectorView"]
        #[ivars = InspectorViewIvars]
        pub struct InspectorView;

        impl InspectorView {
            #[unsafe(method(isFlipped))]
            fn is_flipped(&self) -> bool {
                true
            }

            #[unsafe(method(drawRect:))]
            fn draw_rect(&self, _dirty: NSRect) {
                let c = &self.ivars().colors;
                let b = self.bounds();
                let mantle = c.mantle().with_alpha(0.7).to_nscolor();
                mantle.setFill();
                NSBezierPath::bezierPathWithRect(b).fill();
                let hairline = c.hairline().to_nscolor();
                hairline.setFill();
                NSBezierPath::bezierPathWithRect(NSRect::new(
                    NSPoint::new(0.0, 0.0),
                    NSSize::new(1.0, b.size.height),
                ))
                .fill();
            }
        }

        unsafe impl NSObjectProtocol for InspectorView {}
    );

    pub struct InspectorHandlerIvars {
        pub callback: std::cell::RefCell<Option<Box<dyn FnMut()>>>,
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSInspectorHandler"]
        #[ivars = InspectorHandlerIvars]
        pub struct InspectorHandler;

        impl InspectorHandler {
            #[unsafe(method(openClicked:))]
            fn open_clicked(&self, _sender: Option<&AnyObject>) {
                if let Some(cb) = self.ivars().callback.borrow_mut().as_mut() {
                    cb();
                }
            }
        }

        unsafe impl NSObjectProtocol for InspectorHandler {}
    );

    impl InspectorHandler {
        pub fn new(mtm: MainThreadMarker, callback: Option<Box<dyn FnMut()>>) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(InspectorHandlerIvars {
                callback: std::cell::RefCell::new(callback),
            });
            unsafe { msg_send![super(this), init] }
        }
    }

    thread_local! {
        static LIVE_HANDLERS: std::cell::RefCell<Vec<Retained<InspectorHandler>>>
            = const { std::cell::RefCell::new(Vec::new()) };
    }

    pub fn retain_handler(h: &Retained<InspectorHandler>) {
        LIVE_HANDLERS.with(|v| v.borrow_mut().push(h.clone()));
    }

    pub fn make_scroll(mtm: MainThreadMarker) -> Retained<NSScrollView> {
        let scroll = NSScrollView::new(mtm);
        scroll.setFrame(NSRect::new(
            NSPoint::new(SCROLL_LEFT, 0.0),
            NSSize::new(0.0, 0.0),
        ));
        scroll.setAutoresizingMask(
            objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable
                | objc2_app_kit::NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        scroll
    }

    pub fn make_text_view(mtm: MainThreadMarker) -> Retained<NSTextView> {
        let text = NSTextView::new(mtm);
        text.setEditable(false);
        text.setSelectable(true);
        text.setDrawsBackground(false);
        text.setTextContainerInset(NSSize::new(14.0, 14.0));
        text.setVerticallyResizable(true);
        text.setHorizontallyResizable(false);
        if let Some(container) = unsafe { text.textContainer() } {
            container.setWidthTracksTextView(true);
        }
        text
    }

    pub fn make_label(mtm: MainThreadMarker, s: &str, colors: &PopupColors) -> Retained<NSTextField> {
        let hint = NSTextField::labelWithString(&NSString::from_str(s), mtm);
        hint.setFont(Some(&NSFont::systemFontOfSize(11.0)));
        hint.setAlignment(NSTextAlignment::Center);
        hint.setTextColor(Some(&colors.dim.to_nscolor()));
        hint
    }

    /// The button's target stays unretained; the caller keeps the handler alive
    /// through [`retain_handler`].
    pub fn make_open_button(
        mtm: MainThreadMarker,
        handler: &InspectorHandler,
    ) -> Retained<NSButton> {
        unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str("Open full detail"),
                Some(as_any(handler)),
                Some(objc2::sel!(openClicked:)),
                mtm,
            )
        }
    }

    pub fn apply_layout(
        view: &InspectorView,
        scroll: &NSScrollView,
        text: &NSTextView,
        button: &NSButton,
        hint: &NSTextField,
    ) {
        let l = super::inspector_layout(view.bounds().size.width, view.bounds().size.height);
        scroll.setFrame(nsrect(l.scroll));
        let content_w = scroll.contentSize().width;
        text.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(content_w, l.scroll.height)));
        button.setFrame(nsrect(l.button));
        hint.setFrame(nsrect(l.hint));
    }

    /// Re-render the built view (`refresh_macos`).
    pub fn refresh(
        view: &NSView,
        content: Option<&PopupInspectorContent>,
        empty_text: &str,
        zoom: f64,
        colors: &PopupColors,
        mtm: MainThreadMarker,
    ) {
        let _ = mtm;
        let text = find_text(view);
        let button = find_button(view);
        let hint = find_hint(view);
        if let Some(text) = text {
            render(&text, content, empty_text, zoom, colors);
        }
        if let Some(button) = button {
            button.setHidden(content.is_none());
        }
        if let Some(hint) = hint {
            hint.setHidden(content.is_none());
        }
    }

    fn subviews(view: &NSView) -> Vec<Retained<NSView>> {
        view.subviews().iter().map(|v| v.clone()).collect()
    }

    fn find_text(view: &NSView) -> Option<Retained<NSTextView>> {
        for sub in subviews(view) {
            if let Ok(s) = sub.clone().downcast::<NSScrollView>() {
                if let Some(doc) = s.documentView() {
                    if let Ok(t) = doc.downcast::<NSTextView>() {
                        return Some(t);
                    }
                }
            }
        }
        None
    }

    fn find_button(view: &NSView) -> Option<Retained<NSButton>> {
        subviews(view)
            .into_iter()
            .find_map(|v| v.downcast::<NSButton>().ok())
    }

    fn find_hint(view: &NSView) -> Option<Retained<NSTextField>> {
        subviews(view)
            .into_iter()
            .find_map(|v| v.downcast::<NSTextField>().ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_uses_empty_text() {
        let i = PopupInspector::new();
        assert!(i.is_empty());
        assert_eq!(i.body_text(), EMPTY_TEXT);
        assert!(i.render_text().is_none());
    }

    #[test]
    fn body_is_capped_at_700() {
        let mut c = PopupInspectorContent::new("K-1", "Title", vec![], vec![], "x".repeat(900));
        let i = PopupInspector {
            content: Some(c.clone()),
            ..PopupInspector::new()
        };
        assert_eq!(i.body_text().chars().count(), BODY_CAP + 1);
        c.body = "short".into();
        let i = PopupInspector {
            content: Some(c),
            ..PopupInspector::new()
        };
        assert_eq!(i.body_text(), "short");
    }

    #[test]
    fn fields_with_empty_values_are_dropped() {
        let c = PopupInspectorContent::new(
            "K-1",
            "Title",
            vec![InspectorChip::new("Done", PopupTone::Success)],
            vec![
                InspectorField::new("Status", "Done"),
                InspectorField::new("Notes", ""),
                InspectorField::new("Type", "Bug"),
            ],
            "body",
        );
        let i = PopupInspector {
            content: Some(c),
            ..PopupInspector::new()
        };
        let text = i.render_text().unwrap();
        assert!(text.contains("Status\tDone"));
        assert!(text.contains("Type\tBug"));
        assert!(!text.contains("Notes"));
        assert_eq!(
            text,
            "K-1\nTitle\n Done \nbody\nStatus\tDone\nType\tBug\n"
        );
    }

    #[test]
    fn zoom_floor_is_half() {
        let mut i = PopupInspector::new();
        i.zoom = 0.1;
        assert_eq!(i.effective_zoom(), 0.5);
        i.zoom = 1.5;
        assert_eq!(i.effective_zoom(), 1.5);
    }

    #[test]
    fn open_fires_callback() {
        use std::cell::Cell;
        use std::rc::Rc;
        let hit = Rc::new(Cell::new(false));
        let h = hit.clone();
        let mut i = PopupInspector::new();
        i.on_open = Some(Box::new(move || h.set(true)));
        i.open();
        assert!(hit.get());
    }

    #[test]
    fn layout_footer_and_scroll() {
        let l = inspector_layout(320.0, 420.0);
        assert_eq!(l.scroll, Rect::new(SCROLL_LEFT, 0.0, 319.0, 350.0));
        assert_eq!(l.button.width, 320.0 - 36.0);
        assert_eq!(l.button.x, 18.0);
        assert_eq!(l.button.y, 420.0 - FOOTER_HEIGHT + BUTTON_TOP);
        assert_eq!(l.hint.x, HINT_INSET);
        assert_eq!(l.hint.width, 320.0 - 16.0);
    }

    #[test]
    fn layout_clamps_tiny_bounds() {
        let l = inspector_layout(100.0, 60.0);
        assert_eq!(l.button.width, BUTTON_MIN_WIDTH);
        assert!(l.scroll.height >= 0.0);
        assert_eq!(l.hint.width, 84.0);
    }

    #[test]
    fn segments_order_and_empty_rules() {
        let c = PopupInspectorContent::new(
            "K-1",
            "T",
            vec![
                InspectorChip::new("A", PopupTone::Success),
                InspectorChip::new("B", PopupTone::Danger),
            ],
            vec![
                InspectorField::new("S", "done"),
                InspectorField::new("N", ""),
            ],
            "body",
        );
        let segs = inspector_segments(&c);
        assert_eq!(segs[0], InspectorSegment::Key("K-1".into()));
        assert_eq!(segs[1], InspectorSegment::Title("T".into()));
        assert_eq!(segs[2], InspectorSegment::Chip { text: "A".into(), tone: PopupTone::Success });
        assert_eq!(segs[3], InspectorSegment::Chip { text: "B".into(), tone: PopupTone::Danger });
        assert_eq!(segs[4], InspectorSegment::Body("body".into()));
        assert_eq!(segs[5], InspectorSegment::Field { label: "S".into(), value: "done".into() });
        assert_eq!(segs.len(), 6, "empty field value dropped");

        let c = PopupInspectorContent::new("K", "T", vec![], vec![], "");
        assert_eq!(
            inspector_segments(&c),
            vec![
                InspectorSegment::Key("K".into()),
                InspectorSegment::Title("T".into()),
            ]
        );
    }
}
