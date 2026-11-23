//! The silver border every floating preview window wears (the image / text
//! `FilePopup`, pinned screenshots, Quick Look), so a preview is easy to tell
//! apart from the windows behind it. Colour and width come from `[app]
//! preview-border` / `preview-border-width` (0 = no border).

#[cfg(target_os = "macos")]
pub use appkit::attach;

#[cfg(target_os = "macos")]
mod appkit {
    use objc2::rc::Retained;
    use objc2::{define_class, msg_send, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSAutoresizingMaskOptions, NSView};
    use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect};

    pub struct PreviewRingIvars;

    define_class!(
        /// The border view: layer-stroked and click-through.
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSPreviewRing"]
        #[ivars = PreviewRingIvars]
        pub struct PreviewRing;

        impl PreviewRing {
            #[unsafe(method(hitTest:))]
            fn hit_test(&self, _point: NSPoint) -> *mut NSView {
                std::ptr::null_mut()
            }
        }

        unsafe impl NSObjectProtocol for PreviewRing {}
    );

    /// Lay the preview border over `host`'s bounds inset by `inset`, rounded
    /// by `corner`, tracking the host's size. Replaces a ring already there
    /// (so re-attaching to a reused panel never stacks borders); no-op when
    /// the configured width is 0.
    pub fn attach(host: &NSView, inset: f64, corner: f64) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        for v in host.subviews().iter() {
            if v.downcast_ref::<PreviewRing>().is_some() {
                v.removeFromSuperview();
            }
        }
        let mut settings = crate::app::config::AppSettings::default();
        crate::app::config::apply_app_config_from_disk(&mut settings);
        let width = settings.preview_border_width;
        if width <= 0.0 {
            return;
        }
        let b = host.bounds();
        let frame = NSRect::new(
            NSPoint::new(b.origin.x + inset, b.origin.y + inset),
            objc2_foundation::NSSize::new((b.size.width - 2.0 * inset).max(0.0), (b.size.height - 2.0 * inset).max(0.0)),
        );
        let this = PreviewRing::alloc(mtm).set_ivars(PreviewRingIvars);
        let ring: Retained<PreviewRing> = unsafe { msg_send![super(this), initWithFrame: frame] };
        ring.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        ring.setWantsLayer(true);
        if let Some(layer) = ring.layer() {
            layer.setCornerRadius(corner);
            layer.setBorderWidth(width);
            let c = settings.preview_border;
            let color = crate::ui::theme::Rgba::new(c.r, c.g, c.b, c.a);
            layer.setBorderColor(Some(&color.to_nscolor().CGColor()));
        }
        host.addSubview(&ring);
    }
}
