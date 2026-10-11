//! The themed popup card every tool panel sits in — Swift's `PopupWindow`
//! backdrop (HUD vibrancy + the theme tint at `tint-alpha`, a 1 pt outline,
//! 9 pt corners) with a `PopupChrome` header (✕, the icon menu, the dim hint,
//! the right-hand segmented buttons) and a content view under it.

#[cfg(target_os = "macos")]
pub use appkit::*;

#[cfg(target_os = "macos")]
mod appkit {
    use crate::ui::chrome::{ChromeConfig, PopupChrome};
    use crate::ui::theme::PopupColors;
    use objc2::rc::Retained;
    use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSAutoresizingMaskOptions, NSImage, NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial,
        NSVisualEffectState, NSVisualEffectView,
    };
    use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect, NSSize};

    pub struct FlippedIvars;

    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSPanelFrameRoot"]
        #[ivars = FlippedIvars]
        pub struct PanelRoot;

        impl PanelRoot {
            #[unsafe(method(isFlipped))]
            fn is_flipped(&self) -> bool {
                true
            }
        }

        unsafe impl NSObjectProtocol for PanelRoot {}
    );

    /// The pieces of a built card.
    pub struct PanelFrame {
        pub root: Retained<NSView>,
        pub chrome: Retained<PopupChrome>,
        /// Unflipped content area under the header (sized w × (h − header)).
        pub content: Retained<NSView>,
        pub header_height: f64,
    }

    pub struct PanelSpec<'a> {
        pub colors: PopupColors,
        pub header_color: Option<crate::ui::theme::Rgba>,
        pub title: Option<&'a str>,
        pub meta: &'a str,
        pub buttons: Vec<(String, i32)>,
        pub icon: Option<Retained<NSImage>>,
        pub tint_alpha: f64,
    }

    fn fill() -> NSAutoresizingMaskOptions {
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable
    }

    /// Build the card at `w × h`.
    pub fn build(mtm: MainThreadMarker, w: f64, h: f64, spec: PanelSpec<'_>) -> PanelFrame {
        let header_h = 30.0;
        let radius = 9.0;
        let this = PanelRoot::alloc(mtm).set_ivars(FlippedIvars);
        let root: Retained<PanelRoot> = unsafe {
            msg_send![super(this), initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w, h))]
        };
        root.setWantsLayer(true);
        if let Some(l) = root.layer() {
            l.setCornerRadius(radius);
            l.setMasksToBounds(true);
        }
        root.setAutoresizingMask(fill());
        let fx = NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), root.bounds());
        fx.setMaterial(NSVisualEffectMaterial::HUDWindow);
        fx.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        fx.setState(NSVisualEffectState::Active);
        fx.setAutoresizingMask(fill());
        root.addSubview(&fx);
        let tint = NSView::initWithFrame(NSView::alloc(mtm), root.bounds());
        tint.setWantsLayer(true);
        if let Some(l) = tint.layer() {
            l.setBackgroundColor(Some(&spec.colors.base().with_alpha(spec.tint_alpha).to_nscolor().CGColor()));
            l.setBorderColor(Some(&spec.colors.border.to_nscolor().CGColor()));
            l.setBorderWidth(1.0);
            l.setCornerRadius(radius);
        }
        tint.setAutoresizingMask(fill());
        root.addSubview(&tint);
        let chrome = PopupChrome::create(
            mtm,
            ChromeConfig {
                colors: spec.colors,
                header_color: spec.header_color,
                header_height: header_h,
                title_pill: false,
                ..ChromeConfig::default()
            },
        );
        chrome.set_drag_header_height(header_h);
        chrome.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w, header_h)));
        chrome.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
        chrome.set_copy_labels("", "");
        chrome.set_extra_buttons(spec.buttons);
        chrome.set_meta(spec.meta);
        if let Some(t) = spec.title {
            chrome.set_header_title(Some(t.to_string()));
            chrome.set_always_title(true);
        }
        chrome.set_header_icon(spec.icon);
        root.addSubview(&chrome);
        let content = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, header_h), NSSize::new(w, (h - header_h).max(0.0))),
        );
        content.setAutoresizingMask(fill());
        root.addSubview(&content);
        PanelFrame { root: Retained::into_super(root), chrome, content, header_height: header_h }
    }
}
