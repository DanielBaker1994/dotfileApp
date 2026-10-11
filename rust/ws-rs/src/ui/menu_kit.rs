//! Closure-backed pop-up menus (Swift's `menuItem(_:) { … }` helper +
//! `NSMenu.popUp(positioning:at:in:)`). The chosen item's closure runs after
//! the menu closes, so it can freely borrow view / host state.

#[cfg(target_os = "macos")]
pub use appkit::*;

/// One menu row.
pub enum MenuEntry {
    Item {
        title: String,
        checked: bool,
        enabled: bool,
        /// A color swatch drawn as the item image (the theme presets).
        swatch: Option<crate::ui::theme::Rgba>,
        action: Box<dyn FnOnce()>,
    },
    /// A nested submenu (`NSMenuItem.submenu`).
    Submenu { title: String, checked: bool, swatch: Option<crate::ui::theme::Rgba>, entries: Vec<MenuEntry> },
    /// A section title (`NSMenuItem.sectionHeader(title:)`).
    Header(String),
    Separator,
}

impl MenuEntry {
    pub fn item(title: &str, action: impl FnOnce() + 'static) -> Self {
        MenuEntry::Item { title: title.to_string(), checked: false, enabled: true, swatch: None, action: Box::new(action) }
    }

    pub fn checked(title: &str, on: bool, action: impl FnOnce() + 'static) -> Self {
        MenuEntry::Item { title: title.to_string(), checked: on, enabled: true, swatch: None, action: Box::new(action) }
    }

    pub fn disabled(title: &str) -> Self {
        MenuEntry::Item { title: title.to_string(), checked: false, enabled: false, swatch: None, action: Box::new(|| {}) }
    }

    pub fn submenu(title: &str, entries: Vec<MenuEntry>) -> Self {
        MenuEntry::Submenu { title: title.to_string(), checked: false, swatch: None, entries }
    }

    /// Attach a color swatch (items and submenus).
    pub fn with_swatch(mut self, c: crate::ui::theme::Rgba) -> Self {
        match &mut self {
            MenuEntry::Item { swatch, .. } | MenuEntry::Submenu { swatch, .. } => *swatch = Some(c),
            _ => {}
        }
        self
    }

    /// Mark a submenu row checked (the current preset).
    pub fn with_checked(mut self, on: bool) -> Self {
        match &mut self {
            MenuEntry::Item { checked, .. } | MenuEntry::Submenu { checked, .. } => *checked = on,
            _ => {}
        }
        self
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

    /// A 14×14 rounded swatch image (`ThemePreset.swatch()`).
    fn swatch_image(c: crate::ui::theme::Rgba) -> Retained<objc2_app_kit::NSImage> {
        use objc2::AnyThread;
        use objc2_app_kit::{NSBezierPath, NSImage};
        use objc2_foundation::{NSRect, NSSize};
        let img = NSImage::initWithSize(NSImage::alloc(), NSSize::new(14.0, 14.0));
        #[allow(deprecated)]
        img.lockFocus();
        let r = NSRect::new(NSPoint::new(1.0, 1.0), NSSize::new(12.0, 12.0));
        let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(r, 3.0, 3.0);
        c.with_alpha(1.0).to_nscolor().setFill();
        path.fill();
        crate::ui::theme::Rgba::new(0.5, 0.5, 0.5, 0.6).to_nscolor().setStroke();
        path.setLineWidth(0.5);
        path.stroke();
        #[allow(deprecated)]
        img.unlockFocus();
        img
    }

    fn fill(
        mtm: MainThreadMarker,
        menu: &NSMenu,
        entries: Vec<MenuEntry>,
        target: &MenuPick,
        actions: &mut Vec<Option<Box<dyn FnOnce()>>>,
    ) {
        for e in entries {
            match e {
                MenuEntry::Separator => menu.addItem(&NSMenuItem::separatorItem(mtm)),
                MenuEntry::Header(title) => {
                    let item = NSMenuItem::sectionHeaderWithTitle(&NSString::from_str(&title), mtm);
                    menu.addItem(&item);
                }
                MenuEntry::Submenu { title, checked, swatch, entries } => {
                    let item = unsafe {
                        NSMenuItem::initWithTitle_action_keyEquivalent(
                            NSMenuItem::alloc(mtm),
                            &NSString::from_str(&title),
                            None,
                            &NSString::from_str(""),
                        )
                    };
                    item.setState(if checked { NSControlStateValueOn } else { NSControlStateValueOff });
                    if let Some(c) = swatch {
                        item.setImage(Some(&swatch_image(c)));
                    }
                    let sub = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(&title));
                    sub.setAutoenablesItems(false);
                    fill(mtm, &sub, entries, target, actions);
                    item.setSubmenu(Some(&sub));
                    menu.addItem(&item);
                }
                MenuEntry::Item { title, checked, enabled, swatch, action } => {
                    let item = unsafe {
                        NSMenuItem::initWithTitle_action_keyEquivalent(
                            NSMenuItem::alloc(mtm),
                            &NSString::from_str(&title),
                            Some(sel!(pick:)),
                            &NSString::from_str(""),
                        )
                    };
                    unsafe { item.setTarget(Some(target)) };
                    item.setTag(actions.len() as isize);
                    item.setEnabled(enabled);
                    item.setState(if checked { NSControlStateValueOn } else { NSControlStateValueOff });
                    if let Some(c) = swatch {
                        item.setImage(Some(&swatch_image(c)));
                    }
                    menu.addItem(&item);
                    actions.push(Some(action));
                }
            }
        }
    }

    /// Pop `entries` at `(x, y)` in `view` coordinates (or screen coordinates
    /// when `view` is `None`), then run the chosen closure.
    pub fn popup(entries: Vec<MenuEntry>, view: Option<&NSView>, x: f64, y: f64) {
        run(entries, |menu| {
            menu.popUpMenuPositioningItem_atLocation_inView(None, NSPoint::new(x, y), view);
        });
    }

    /// `NSMenu.popUpContextMenu(_:with:for:)` for the event being handled
    /// (a right-click). Unlike [`popup`], the menu opens above `view`'s
    /// window even when that window floats over the normal menu level
    /// (the screenshot overlay's recent grid).
    pub fn popup_context(entries: Vec<MenuEntry>, view: &NSView) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let Some(event) = objc2_app_kit::NSApplication::sharedApplication(mtm).currentEvent() else { return };
        run(entries, |menu| NSMenu::popUpContextMenu_withEvent_forView(menu, &event, view));
    }

    /// Build the menu, show it with `show` (which blocks until it closes),
    /// then run the chosen closure.
    fn run(entries: Vec<MenuEntry>, show: impl FnOnce(&NSMenu)) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let target = MenuPick::alloc(mtm).set_ivars(PickIvars { chosen: Cell::new(None) });
        let target: Retained<MenuPick> = unsafe { msg_send![super(target), init] };
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        let mut actions: Vec<Option<Box<dyn FnOnce()>>> = Vec::new();
        fill(mtm, &menu, entries, &target, &mut actions);
        show(&menu);
        if let Some(i) = target.ivars().chosen.get() {
            if let Some(Some(a)) = actions.get_mut(i as usize).map(|a| a.take()) {
                a();
            }
        }
    }
}
