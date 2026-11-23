//! `PopupWindow.showActionPicker(title:items:onPick:)`: the Cmd+K card over a
//! list window — a dim title line, then numbered rows (title + detail),
//! ↑↓ / ⌃N ⌃P / Tab to move, Return / ⌃J or 1–9 to pick, Esc to close.

/// `actionPickerKey`'s outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickerKey {
    /// Redraw with this index.
    Move(usize),
    /// Close and run item `i`.
    Pick(usize),
    Close,
    /// Swallowed (the picker owns the keyboard).
    Ignore,
}

/// macOS key codes for the digits 1–9 (`[18: 1, 19: 2, …]`).
const DIGITS: [u16; 9] = [18, 19, 20, 21, 23, 22, 26, 28, 25];

/// `actionPickerKey(_:_:)` over `n` items at `index`.
pub fn picker_key(code: u16, ctrl: bool, shift: bool, index: usize, n: usize) -> PickerKey {
    if n == 0 {
        return PickerKey::Close;
    }
    let step = |d: i64| PickerKey::Move(((index as i64 + d).rem_euclid(n as i64)) as usize);
    match code {
        125 => step(1),
        45 if ctrl => step(1),
        126 => step(-1),
        35 if ctrl => step(-1),
        48 => step(if shift { -1 } else { 1 }),
        36 | 76 => PickerKey::Pick(index),
        38 if ctrl => PickerKey::Pick(index),
        53 => PickerKey::Close,
        c => match DIGITS.iter().position(|d| *d == c) {
            Some(i) if i < n => PickerKey::Pick(i),
            _ => PickerKey::Ignore,
        },
    }
}

#[cfg(target_os = "macos")]
pub use appkit::ActionPicker;

#[cfg(target_os = "macos")]
mod appkit {
    use super::*;
    use crate::ui::chrome::Rect;
    use crate::ui::style::{self, Weight};
    use crate::ui::theme::PopupColors;
use crate::ui::view_base;
    use objc2::rc::Retained;
    use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSEvent, NSView};
    use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect, NSSize};
    use std::cell::{Cell, RefCell};

    const ROW_H: f64 = 34.0;
    const PAD: f64 = 8.0;
    const TITLE_H: f64 = 26.0;

    pub struct PickerIvars {
        title: String,
        items: Vec<(String, String)>,
        index: Cell<usize>,
        colors: PopupColors,
        on_pick: RefCell<Option<Box<dyn Fn(usize)>>>,
    }

    define_class!(
        #[unsafe(super(crate::ui::view_base::WSFlippedView))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSActionPicker"]
        #[ivars = PickerIvars]
        pub struct ActionPicker;

        impl ActionPicker {
            #[unsafe(method(acceptsFirstMouse:))]
            fn afm(&self, _e: Option<&NSEvent>) -> bool {
                true
            }

            #[unsafe(method(mouseDown:))]
            fn mouse_down(&self, e: &NSEvent) {
                let p = view_base::mouse_point(self, e);
                let i = ((p.y - PAD - TITLE_H) / ROW_H).floor();
                if i >= 0.0 && (i as usize) < self.ivars().items.len() {
                    self.pick(i as usize);
                }
            }

            #[unsafe(method(drawRect:))]
            fn draw_rect(&self, _dirty: NSRect) {
                self.draw();
            }
        }

        unsafe impl NSObjectProtocol for ActionPicker {}
    );

    impl ActionPicker {
        /// `showActionPicker`: build and add over `root` (28% down, centered).
        pub fn show(
            mtm: MainThreadMarker,
            root: &NSView,
            title: &str,
            items: Vec<(String, String)>,
            colors: PopupColors,
            on_pick: Box<dyn Fn(usize)>,
        ) -> Option<Retained<ActionPicker>> {
            if items.is_empty() {
                return None;
            }
            let b = root.bounds();
            let w = 420.0f64.min(b.size.width - 40.0);
            let h = TITLE_H + items.len() as f64 * ROW_H + PAD * 2.0;
            let y = if root.isFlipped() { (b.size.height * 0.28).max(40.0) } else { b.size.height * 0.72 - h };
            let this = Self::alloc(mtm).set_ivars(PickerIvars {
                title: title.to_string(),
                items,
                index: Cell::new(0),
                colors,
                on_pick: RefCell::new(Some(on_pick)),
            });
            let v: Retained<Self> = unsafe {
                msg_send![super(this), initWithFrame: NSRect::new(NSPoint::new(((b.size.width - w) / 2.0).round(), y.round()), NSSize::new(w, h))]
            };
            v.setAutoresizingMask(
                objc2_app_kit::NSAutoresizingMaskOptions::ViewMinXMargin
                    | objc2_app_kit::NSAutoresizingMaskOptions::ViewMaxXMargin,
            );
            v.setWantsLayer(true);
            if let Some(l) = v.layer() {
                l.setShadowOpacity(0.35);
                l.setShadowRadius(14.0);
            }
            root.addSubview_positioned_relativeTo(&v, objc2_app_kit::NSWindowOrderingMode::Above, None);
            Some(v)
        }

        pub fn index(&self) -> usize {
            self.ivars().index.get()
        }

        pub fn count(&self) -> usize {
            self.ivars().items.len()
        }

        pub fn titles(&self) -> Vec<String> {
            self.ivars().items.iter().map(|i| i.0.clone()).collect()
        }

        /// Route a key; `true` while the picker stays up.
        pub fn key(&self, code: u16, ctrl: bool, shift: bool) -> bool {
            match picker_key(code, ctrl, shift, self.index(), self.count()) {
                PickerKey::Move(i) => {
                    self.ivars().index.set(i);
                    self.setNeedsDisplay(true);
                    true
                }
                PickerKey::Pick(i) => {
                    self.pick(i);
                    false
                }
                PickerKey::Close => {
                    self.removeFromSuperview();
                    false
                }
                PickerKey::Ignore => true,
            }
        }

        fn pick(&self, i: usize) {
            let cb = self.ivars().on_pick.borrow_mut().take();
            self.removeFromSuperview();
            if let Some(cb) = cb {
                cb(i);
            }
        }

        fn draw(&self) {
            let c = self.ivars().colors;
            let b = self.bounds();
            let card = Rect::new(0.0, 0.0, b.size.width, b.size.height);
            let fill = c.background.blended(0.25, crate::ui::theme::Rgba::BLACK).with_alpha(0.97);
            style::fill_rounded(card, 10.0, fill);
            style::stroke_rounded(card.inset_by(0.5, 0.5), 10.0, 1.0, c.text.with_alpha(0.15));
            let tf = style::system_font(11.0, Weight::Medium);
            style::draw_text_left(
                &format!("{}   ↑↓ / ⌃N ⌃P · ↩ · esc", self.ivars().title),
                Rect::new(PAD + 6.0, PAD, b.size.width - PAD * 2.0, TITLE_H - 6.0),
                &tf,
                c.dim,
            );
            let idx = self.index();
            for (i, (title, detail)) in self.ivars().items.iter().enumerate() {
                let r = Rect::new(PAD, PAD + TITLE_H + i as f64 * ROW_H, b.size.width - PAD * 2.0, ROW_H);
                if i == idx {
                    style::fill_rounded(r, 6.0, c.highlight);
                }
                style::draw_text_left(
                    &format!("{}  {title}", i + 1),
                    Rect::new(r.x + 10.0, r.y + 2.0, r.width - 20.0, 17.0),
                    &style::system_font(13.0, Weight::Semibold),
                    c.text,
                );
                style::draw_text_left(
                    detail,
                    Rect::new(r.x + 26.0, r.y + 18.0, r.width - 36.0, 14.0),
                    &style::system_font(11.0, Weight::Regular),
                    c.dim,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_move_wrap_pick_and_close() {
        assert_eq!(picker_key(125, false, false, 2, 3), PickerKey::Move(0));
        assert_eq!(picker_key(35, true, false, 0, 3), PickerKey::Move(2));
        assert_eq!(picker_key(48, false, true, 0, 3), PickerKey::Move(2));
        assert_eq!(picker_key(36, false, false, 1, 3), PickerKey::Pick(1));
        assert_eq!(picker_key(19, false, false, 0, 3), PickerKey::Pick(1));
        assert_eq!(picker_key(21, false, false, 0, 3), PickerKey::Ignore);
        assert_eq!(picker_key(53, false, false, 0, 3), PickerKey::Close);
    }
}
