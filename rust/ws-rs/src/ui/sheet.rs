//! Sheets: Swift's `jiraFormSheet(on:title:info:rows:ok:first:)` (an
//! `NSAlert` with a label/field grid as its accessory view) and a plain
//! button alert. Rule 1: every sheet text field gets the first responder
//! (`window.initialFirstResponder`) so typing and the edit shortcuts land in
//! it at once.

/// One form row.
pub struct FormRow {
    pub label: String,
    pub value: String,
    pub placeholder: String,
    pub secure: bool,
}

impl FormRow {
    pub fn new(label: &str, value: &str, placeholder: &str) -> Self {
        FormRow { label: label.into(), value: value.into(), placeholder: placeholder.into(), secure: false }
    }

    pub fn secure(mut self) -> Self {
        self.secure = true;
        self
    }
}

#[cfg(target_os = "macos")]
pub use appkit::*;

#[cfg(target_os = "macos")]
mod appkit {
    use super::FormRow;
    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSAlert, NSAlertFirstButtonReturn, NSFont, NSModalResponse, NSSecureTextField, NSTextField,
        NSView, NSWindow,
    };
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

    const LABEL_W: f64 = 64.0;
    const FIELD_W: f64 = 420.0;
    const ROW_H: f64 = 24.0;
    const GAP: f64 = 8.0;

    /// Show the form as a sheet on `window`; `then(Some(values))` on OK,
    /// `then(None)` on Cancel.
    pub fn form_sheet(
        window: &NSWindow,
        title: &str,
        info: &str,
        rows: Vec<FormRow>,
        ok: &str,
        then: impl FnOnce(Option<Vec<String>>) + 'static,
    ) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let alert = NSAlert::new(mtm);
        alert.setMessageText(&NSString::from_str(title));
        alert.setInformativeText(&NSString::from_str(info));
        alert.addButtonWithTitle(&NSString::from_str(ok));
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));
        let n = rows.len() as f64;
        let h = n * ROW_H + (n - 1.0).max(0.0) * GAP;
        let acc = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(LABEL_W + 8.0 + FIELD_W, h)),
        );
        let mut fields: Vec<Retained<NSTextField>> = Vec::new();
        for (i, r) in rows.iter().enumerate() {
            // AppKit's accessory view is unflipped: row 0 at the top.
            let y = h - (i as f64 + 1.0) * ROW_H - i as f64 * GAP;
            let l = NSTextField::labelWithString(&NSString::from_str(&r.label), mtm);
            l.setFont(Some(&NSFont::systemFontOfSize(12.0)));
            l.setFrame(NSRect::new(NSPoint::new(0.0, y + 3.0), NSSize::new(LABEL_W, 18.0)));
            l.setAlignment(objc2_app_kit::NSTextAlignment::Right);
            acc.addSubview(&l);
            let f: Retained<NSTextField> = if r.secure {
                Retained::into_super(NSSecureTextField::new(mtm))
            } else {
                NSTextField::new(mtm)
            };
            f.setStringValue(&NSString::from_str(&r.value));
            f.setPlaceholderString(Some(&NSString::from_str(&r.placeholder)));
            f.setFrame(NSRect::new(NSPoint::new(LABEL_W + 8.0, y), NSSize::new(FIELD_W, ROW_H)));
            if let Some(cell) = f.cell() {
                cell.setScrollable(true);
                cell.setWraps(false);
            }
            acc.addSubview(&f);
            fields.push(f);
        }
        for w in fields.windows(2) {
            unsafe { w[0].setNextKeyView(Some(&w[1])) };
        }
        alert.setAccessoryView(Some(&acc));
        alert.layout();
        if let Some(first) = fields.first() {
            alert.window().setInitialFirstResponder(Some(first));
        }
        let fields_c = fields.clone();
        let then = std::cell::RefCell::new(Some(then));
        let handler = RcBlock::new(move |resp: NSModalResponse| {
            let ok = resp == NSAlertFirstButtonReturn;
            let values = ok.then(|| fields_c.iter().map(|f| f.stringValue().to_string()).collect());
            if let Some(cb) = then.borrow_mut().take() {
                cb(values);
            }
        });
        alert.beginSheetModalForWindow_completionHandler(window, Some(&handler));
        if let Some(first) = fields.first() {
            alert.window().makeFirstResponder(Some(first));
        }
    }

    /// A message sheet with `buttons`; `then(index)` (0 = first button).
    pub fn alert_sheet(window: &NSWindow, title: &str, text: &str, buttons: &[&str], then: impl FnOnce(usize) + 'static) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let alert = NSAlert::new(mtm);
        alert.setMessageText(&NSString::from_str(title));
        alert.setInformativeText(&NSString::from_str(text));
        for b in buttons {
            alert.addButtonWithTitle(&NSString::from_str(b));
        }
        let then = std::cell::RefCell::new(Some(then));
        let handler = RcBlock::new(move |resp: NSModalResponse| {
            let i = (resp - NSAlertFirstButtonReturn).max(0) as usize;
            if let Some(cb) = then.borrow_mut().take() {
                cb(i);
            }
        });
        alert.beginSheetModalForWindow_completionHandler(window, Some(&handler));
    }
}
