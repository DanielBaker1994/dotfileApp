//! Closure-backed pop-up menus (Swift's `menuItem(_:) { … }` helper +
//! `NSMenu.popUp(positioning:at:in:)`). The chosen item's closure runs after
//! the menu closes, so it can freely borrow view / host state.

#[cfg(target_os = "macos")]
pub use appkit::*;

/// One menu row.
pub enum MenuEntry {
    Item { title: String, checked: bool, enabled: bool, action: Box<dyn FnOnce()> },
    Separator,
}

impl MenuEntry {
    pub fn item(title: &str, action: impl FnOnce() + 'static) -> Self {
        MenuEntry::Item { title: title.to_string(), checked: false, enabled: true, action: Box::new(action) }
    }

    pub fn checked(title: &str, on: bool, action: impl FnOnce() + 'static) -> Self {
        MenuEntry::Item { title: title.to_string(), checked: on, enabled: true, action: Box::new(action) }
    }

    pub fn disabled(title: &str) -> Self {
        MenuEntry::Item { title: title.to_string(), checked: false, enabled: false, action: Box::new(|| {}) }
    }
}

#[cfg(target_os = "macos")]
mod appkit {
    use super::MenuEntry;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject};
    use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSControlStateValueOff, NSControlStateValueOn, NSMenu, NSMenuItem, NSView};
    use objc2_foundation::{NSObjectProtocol, NSPoint, NSString};
    use std::cell::Cell;

    pub struct PickIvars {
        chosen: Cell<Option<isize>>,
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSMenuPick"]
        #[ivars = PickIvars]
        struct MenuPick;

        impl MenuPick {
            #[unsafe(method(pick:))]
            fn pick(&self, sender: &AnyObject) {
                let tag: isize = unsafe { msg_send![sender, tag] };
                self.ivars().chosen.set(Some(tag));
            }
        }

        unsafe impl NSObjectProtocol for MenuPick {}
    );

    /// Pop `entries` at `(x, y)` in `view` coordinates (or screen coordinates
    /// when `view` is `None`), then run the chosen closure.
    pub fn popup(entries: Vec<MenuEntry>, view: Option<&NSView>, x: f64, y: f64) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let target = MenuPick::alloc(mtm).set_ivars(PickIvars { chosen: Cell::new(None) });
        let target: Retained<MenuPick> = unsafe { msg_send![super(target), init] };
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        let mut actions: Vec<Option<Box<dyn FnOnce()>>> = Vec::new();
        for e in entries {
            match e {
                MenuEntry::Separator => menu.addItem(&NSMenuItem::separatorItem(mtm)),
                MenuEntry::Item { title, checked, enabled, action } => {
                    let item = unsafe {
                        NSMenuItem::initWithTitle_action_keyEquivalent(
                            NSMenuItem::alloc(mtm),
                            &NSString::from_str(&title),
                            Some(sel!(pick:)),
                            &NSString::from_str(""),
                        )
                    };
                    unsafe { item.setTarget(Some(&target)) };
                    item.setTag(actions.len() as isize);
                    item.setEnabled(enabled);
                    item.setState(if checked { NSControlStateValueOn } else { NSControlStateValueOff });
                    menu.addItem(&item);
                    actions.push(Some(action));
                }
            }
        }
        menu.popUpMenuPositioningItem_atLocation_inView(None, NSPoint::new(x, y), view);
        if let Some(i) = target.ivars().chosen.get() {
            if let Some(Some(a)) = actions.get_mut(i as usize).map(|a| a.take()) {
                a();
            }
        }
    }
}
