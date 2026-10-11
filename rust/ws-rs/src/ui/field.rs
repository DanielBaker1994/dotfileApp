//! A themed search box: the rounded input-fill + hairline container Swift
//! draws around `BrowserSearchField` / `PopupSearchFieldCell` (with its
//! horizontal inset and vertically centered text), as a borderless
//! `NSTextField` inside a layer-backed box.

#[cfg(target_os = "macos")]
pub use appkit::*;

#[cfg(target_os = "macos")]
mod appkit {
    use crate::ui::style;
    use crate::ui::theme::PopupColors;
    use objc2::rc::Retained;
    use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSAutoresizingMaskOptions, NSFocusRingType, NSFont, NSTextField, NSView};
    use objc2_app_kit::{NSControlTextEditingDelegate, NSTextFieldDelegate};
    use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};

    /// Build the box (`container`) + its field. Size the container; the field
    /// tracks its width and stays vertically centered.
    pub fn search_box(
        mtm: MainThreadMarker,
        colors: &PopupColors,
        font_size: f64,
        placeholder: &str,
        radius: f64,
        height: f64,
    ) -> (Retained<NSView>, Retained<NSTextField>) {
        let container = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(300.0, height)),
        );
        container.setWantsLayer(true);
        if let Some(l) = container.layer() {
            l.setBackgroundColor(Some(&style::input_fill(colors).to_nscolor().CGColor()));
            l.setCornerRadius(radius);
            l.setBorderWidth(1.0);
            l.setBorderColor(Some(&style::input_stroke(colors).to_nscolor().CGColor()));
        }
        let field = NSTextField::new(mtm);
        field.setBezeled(false);
        field.setBordered(false);
        field.setDrawsBackground(false);
        field.setFocusRingType(NSFocusRingType::None);
        let font = NSFont::systemFontOfSize(font_size);
        let line = (font.ascender() - font.descender() + font.leading()).ceil() + 2.0;
        field.setFont(Some(&font));
        field.setTextColor(Some(&colors.text.to_nscolor()));
        field.setPlaceholderString(Some(&NSString::from_str(placeholder)));
        if let Some(cell) = field.cell() {
            cell.setScrollable(true);
            cell.setWraps(false);
        }
        let inset = 8.0;
        field.setFrame(NSRect::new(
            NSPoint::new(inset, ((height - line) / 2.0).floor()),
            NSSize::new(300.0 - inset * 2.0, line),
        ));
        field.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewMinYMargin
                | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );
        container.addSubview(&field);
        (container, field)
    }

    /// A closure-backed `NSTextFieldDelegate`: `on_change(text)` per edit and
    /// `on_command("insertNewline:" | "moveUp:" | "moveDown:" |
    /// "cancelOperation:" | …) -> handled`.
    pub struct FieldDelegateIvars {
        on_change: std::cell::RefCell<Option<Box<dyn Fn(String)>>>,
        on_command: std::cell::RefCell<Option<Box<dyn Fn(&str) -> bool>>>,
    }

    objc2::define_class!(
        #[unsafe(super(objc2::runtime::NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSFieldDelegate"]
        #[ivars = FieldDelegateIvars]
        pub struct FieldDelegate;

        impl FieldDelegate {
            #[unsafe(method(controlTextDidChange:))]
            fn did_change(&self, n: &objc2_foundation::NSNotification) {
                let text = n
                    .object()
                    .and_then(|o| o.downcast::<NSTextField>().ok())
                    .map(|f| f.stringValue().to_string())
                    .unwrap_or_default();
                if let Some(cb) = self.ivars().on_change.borrow().as_ref() {
                    cb(text);
                }
            }

            #[unsafe(method(control:textView:doCommandBySelector:))]
            fn do_command(
                &self,
                _c: &objc2::runtime::AnyObject,
                _tv: &objc2_app_kit::NSTextView,
                cmd: objc2::runtime::Sel,
            ) -> objc2::runtime::Bool {
                let name = cmd.name().to_str().unwrap_or("").to_string();
                let handled = self.ivars().on_command.borrow().as_ref().is_some_and(|cb| cb(&name));
                objc2::runtime::Bool::new(handled)
            }
        }

        unsafe impl NSObjectProtocol for FieldDelegate {}
        unsafe impl NSControlTextEditingDelegate for FieldDelegate {}
        unsafe impl NSTextFieldDelegate for FieldDelegate {}
    );

    impl FieldDelegate {
        /// Attach a delegate to `field` (kept alive by the returned handle —
        /// the field's delegate is weak, so store it).
        pub fn attach(
            mtm: MainThreadMarker,
            field: &NSTextField,
            on_change: Box<dyn Fn(String)>,
            on_command: Box<dyn Fn(&str) -> bool>,
        ) -> Retained<FieldDelegate> {
            use objc2::DefinedClass;
            let this = Self::alloc(mtm).set_ivars(FieldDelegateIvars {
                on_change: std::cell::RefCell::new(Some(on_change)),
                on_command: std::cell::RefCell::new(Some(on_command)),
            });
            let d: Retained<FieldDelegate> = unsafe { objc2::msg_send![super(this), init] };
            unsafe { field.setDelegate(Some(objc2::runtime::ProtocolObject::from_ref(&*d))) };
            d
        }
    }

    /// Show the focus stroke (`updatePartFocus`: 2 pt accent while editing).
    pub fn set_focused(container: &NSView, colors: &PopupColors, on: bool) {
        if let Some(l) = container.layer() {
            l.setBorderWidth(if on { 2.0 } else { 1.0 });
            let c = if on { colors.accent_on() } else { style::input_stroke(colors) };
            l.setBorderColor(Some(&c.to_nscolor().CGColor()));
        }
    }
}
