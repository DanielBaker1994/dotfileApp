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
    use objc2::{msg_send, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSAutoresizingMaskOptions, NSFocusRingType, NSFont, NSTextField, NSView};
    use objc2_app_kit::{NSControlTextEditingDelegate, NSTextFieldDelegate};
    use objc2_app_kit::{NSFontAttributeName, NSForegroundColorAttributeName};
    use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};
    use objc2_foundation::{NSAttributedString, NSAttributedStringKey, NSDictionary};
    use objc2::runtime::AnyObject;

    use crate::ui::view_base::as_any;

    /// Build an attributed placeholder forced to the theme's `dim` colour so
    /// the text does not resolve against the window's effectiveAppearance.
    pub fn build_attributed_placeholder(
        colors: &PopupColors,
        font_size: f64,
        text: &str,
    ) -> Retained<NSAttributedString> {
        let font = NSFont::systemFontOfSize(font_size);
        let color = colors.dim.to_nscolor();
        let keys: [&NSAttributedStringKey; 2] =
            unsafe { [NSFontAttributeName, NSForegroundColorAttributeName] };
        let objs: [&AnyObject; 2] = [as_any(&*font), as_any(&*color)];
        let attrs = NSDictionary::from_slices(&keys, &objs);
        unsafe {
            NSAttributedString::initWithString_attributes(
                NSAttributedString::alloc(),
                &NSString::from_str(text),
                Some(&attrs),
            )
        }
    }

    /// Apply an attributed placeholder to a text field's cell (AppKit only
    /// honours attributed placeholders set on the cell, not the field).
    pub fn set_attributed_placeholder(field: &NSTextField, attr: &NSAttributedString) {
        let cell: Option<Retained<AnyObject>> = unsafe { msg_send![field, cell] };
        if let Some(cell) = cell {
            let _: () = unsafe { msg_send![&*cell, setPlaceholderAttributedString: &*attr] };
        }
    }

    /// One-shot: build an attributed placeholder from the theme's `dim` colour
    /// and install it on the field's cell.
    pub fn set_themed_placeholder(
        field: &NSTextField,
        colors: &PopupColors,
        font_size: f64,
        text: &str,
    ) {
        let attr = build_attributed_placeholder(colors, font_size, text);
        set_attributed_placeholder(field, &attr);
    }

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
        let attr = build_attributed_placeholder(colors, font_size, placeholder);
        set_attributed_placeholder(&field, &attr);
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
        on_end: std::cell::RefCell<Option<Box<dyn Fn()>>>,
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

            /// Editing ended without a command (focus moved elsewhere, e.g.
            /// a click outside the field).
            #[unsafe(method(controlTextDidEndEditing:))]
            fn did_end(&self, _n: &objc2_foundation::NSNotification) {
                if let Some(cb) = self.ivars().on_end.borrow().as_ref() {
                    cb();
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
            
            let this = Self::alloc(mtm).set_ivars(FieldDelegateIvars {
                on_change: std::cell::RefCell::new(Some(on_change)),
                on_command: std::cell::RefCell::new(Some(on_command)),
                on_end: std::cell::RefCell::new(None),
            });
            let d: Retained<FieldDelegate> = unsafe { objc2::msg_send![super(this), init] };
            unsafe { field.setDelegate(Some(objc2::runtime::ProtocolObject::from_ref(&*d))) };
            d
        }

        /// Run `cb` when editing ends (the field lost focus).
        pub fn set_on_end(&self, cb: Box<dyn Fn()>) {
            use objc2::DefinedClass;
            *self.ivars().on_end.borrow_mut() = Some(cb);
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
