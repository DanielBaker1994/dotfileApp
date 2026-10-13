//! `InlineRename.swift`: an edit field laid over a row's name. Return / Tab
//! commit, Esc cancels, clicking elsewhere commits (as Finder does); the stem
//! (name without extension) starts selected.

/// The selection a rename starts with: the stem for files, all of a folder
/// name (UTF-16 length, as `NSString` counts).
pub fn initial_selection(name: &str, is_dir: bool) -> (usize, usize) {
    let all = name.encode_utf16().count();
    if is_dir {
        return (0, all);
    }
    let stem = match name.rfind('.') {
        Some(i) if i > 0 => name[..i].encode_utf16().count(),
        _ => all,
    };
    (0, if stem > 0 { stem } else { all })
}

#[cfg(target_os = "macos")]
pub use appkit::{active, begin};

/// Cancel any live rename field (its callback gets `None`).
#[cfg(target_os = "macos")]
pub fn cancel() {
    appkit::finish(Some(None));
}

#[cfg(target_os = "macos")]
mod appkit {
    use super::initial_selection;
    use crate::ui::field::FieldDelegate;
    use crate::ui::theme::PopupColors;
    use objc2::rc::Retained;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSFocusRingType, NSFont, NSTextField, NSView};
    use objc2_foundation::{NSRange, NSRect, NSString};
    use std::cell::RefCell;
    use std::rc::Rc;

    thread_local! {
        /// The live rename field + its delegate (the field holds it weakly).
        static LIVE: RefCell<Option<(Retained<NSTextField>, Retained<FieldDelegate>, Rc<Done>)>> = const { RefCell::new(None) };
    }

    type Done = Box<dyn Fn(Option<String>)>;

    /// Whether a rename field is up (it owns the keyboard: the host's
    /// window keys stand down so typing / Return / Esc reach the field).
    pub fn active() -> bool {
        LIVE.with(|l| l.borrow().as_ref().is_some_and(|(f, _, _)| unsafe { f.superview() }.is_some()))
    }

    /// Drop the field; run its callback with `result` (when one was live).
    pub(super) fn finish(result: Option<Option<String>>) {
        let live = LIVE.with(|l| l.borrow_mut().take());
        if let Some((f, _, done)) = live {
            f.removeFromSuperview();
            if let Some(r) = result {
                done(r);
            }
        }
    }

    /// `InlineRename.begin(parent:nameRect:name:isDir:colors:)`. `done`
    /// receives the new text on commit, `None` on cancel. Any rename already
    /// in progress is cancelled first.
    pub fn begin(
        parent: &NSView,
        name_rect: NSRect,
        name: &str,
        is_dir: bool,
        colors: PopupColors,
        font_size: f64,
        done: Box<dyn Fn(Option<String>)>,
    ) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        finish(Some(None));
        let f = NSTextField::textFieldWithString(&NSString::from_str(name), mtm);
        f.setFont(Some(&NSFont::systemFontOfSize(font_size)));
        f.setBordered(false);
        f.setBezeled(false);
        f.setFocusRingType(NSFocusRingType::None);
        f.setDrawsBackground(true);
        f.setBackgroundColor(Some(&colors.mantle().to_nscolor()));
        f.setTextColor(Some(&colors.text.to_nscolor()));
        f.setUsesSingleLineMode(true);
        f.setWantsLayer(true);
        if let Some(l) = f.layer() {
            l.setCornerRadius(3.0);
            l.setBorderWidth(1.0);
            l.setBorderColor(Some(&colors.accent_on().to_nscolor().CGColor()));
        }
        let nr = name_rect;
        f.setFrame(NSRect::new(
            objc2_foundation::NSPoint::new(nr.origin.x - 3.0, nr.origin.y + 2.0),
            objc2_foundation::NSSize::new(nr.size.width + 3.0, nr.size.height - 4.0),
        ));
        parent.addSubview(&f);
        let done: Rc<Done> = Rc::new(done);
        let field = f.clone();
        let delegate = FieldDelegate::attach(
            mtm,
            &f,
            Box::new(|_| {}),
            Box::new(move |cmd| match cmd {
                "insertNewline:" | "insertTab:" | "insertBacktab:" => {
                    let text = field.stringValue().to_string();
                    // Remove the field after AppKit finishes this command.
                    crate::platform::run_on_main(move || finish(Some(Some(text))));
                    true
                }
                "cancelOperation:" => {
                    crate::platform::run_on_main(|| finish(Some(None)));
                    true
                }
                _ => false,
            }),
        );
        // Click-off: focus left the field without Return/Esc — commit and
        // drop the field (else its edit box lingers). Only for this field:
        // a newer rename may already have replaced it.
        let addr = Retained::as_ptr(&f) as usize;
        delegate.set_on_end(Box::new(move || {
            crate::platform::run_on_main(move || {
                let text = LIVE.with(|l| {
                    l.borrow()
                        .as_ref()
                        .filter(|(lf, _, _)| Retained::as_ptr(lf) as usize == addr)
                        .map(|(lf, _, _)| lf.stringValue().to_string())
                });
                if let Some(text) = text {
                    finish(Some(Some(text)));
                }
            });
        }));
        if let Some(w) = parent.window() {
            w.makeFirstResponder(Some(&f));
        }
        let (loc, len) = initial_selection(name, is_dir);
        if let Some(ed) = f.currentEditor() {
            ed.setSelectedRange(NSRange::new(loc, len));
        }
        LIVE.with(|l| *l.borrow_mut() = Some((f, delegate, done)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stem_selected_for_files_all_for_dirs() {
        assert_eq!(initial_selection("notes.md", false), (0, 5));
        assert_eq!(initial_selection("archive.tar.gz", false), (0, 11));
        assert_eq!(initial_selection(".zshrc", false), (0, 6));
        assert_eq!(initial_selection("src.d", true), (0, 5));
    }
}
