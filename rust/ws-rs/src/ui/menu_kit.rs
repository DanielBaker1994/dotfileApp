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
        /// Runs while the row is highlighted (live preview on hover).
        hover: Option<std::rc::Rc<dyn Fn()>>,
        action: Box<dyn FnOnce()>,
    },
    /// A nested submenu (`NSMenuItem.submenu`). `restore` undoes the hover
    /// previews of its rows when the pointer leaves them or the menu closes.
    Submenu {
        title: String,
        checked: bool,
        swatch: Option<crate::ui::theme::Rgba>,
        hover: Option<std::rc::Rc<dyn Fn()>>,
        restore: Option<std::rc::Rc<dyn Fn()>>,
        entries: Vec<MenuEntry>,
    },
    /// A section title (`NSMenuItem.sectionHeader(title:)`).
    Header(String),
    Separator,
}

impl MenuEntry {
    pub fn item(title: &str, action: impl FnOnce() + 'static) -> Self {
        MenuEntry::Item { title: title.to_string(), checked: false, enabled: true, swatch: None, hover: None, action: Box::new(action) }
    }

    pub fn checked(title: &str, on: bool, action: impl FnOnce() + 'static) -> Self {
        MenuEntry::Item { title: title.to_string(), checked: on, enabled: true, swatch: None, hover: None, action: Box::new(action) }
    }

    pub fn disabled(title: &str) -> Self {
        MenuEntry::Item { title: title.to_string(), checked: false, enabled: false, swatch: None, hover: None, action: Box::new(|| {}) }
    }

    pub fn submenu(title: &str, entries: Vec<MenuEntry>) -> Self {
        MenuEntry::Submenu { title: title.to_string(), checked: false, swatch: None, hover: None, restore: None, entries }
    }

    /// Attach a color swatch (items and submenus).
    pub fn with_swatch(mut self, c: crate::ui::theme::Rgba) -> Self {
        match &mut self {
            MenuEntry::Item { swatch, .. } | MenuEntry::Submenu { swatch, .. } => *swatch = Some(c),
            _ => {}
        }
        self
    }

    /// Run `f` while this row is highlighted (the Theme menu's live preview).
    pub fn with_hover(mut self, f: impl Fn() + 'static) -> Self {
        match &mut self {
            MenuEntry::Item { hover, .. } | MenuEntry::Submenu { hover, .. } => *hover = Some(std::rc::Rc::new(f)),
            _ => {}
        }
        self
    }

    /// On a submenu: undo the hover previews of its rows when the highlight
    /// leaves them or the menu closes (`ThemePreviewDelegate.onClose`).
    pub fn with_preview_restore(mut self, f: impl Fn() + 'static) -> Self {
        if let MenuEntry::Submenu { restore, .. } = &mut self {
            *restore = Some(std::rc::Rc::new(f));
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
    use objc2_app_kit::NSMenuDelegate;
    use objc2_foundation::{NSObjectProtocol, NSPoint, NSString};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

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

    pub struct HoverIvars {
        /// Hover closures, indexed by the `representedObject` number.
        hovers: RefCell<Vec<Rc<dyn Fn()>>>,
        restore: Option<Rc<dyn Fn()>>,
        current: Cell<isize>,
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSMenuHover"]
        #[ivars = HoverIvars]
        struct MenuHover;

        unsafe impl NSObjectProtocol for MenuHover {}

        unsafe impl NSMenuDelegate for MenuHover {
            #[unsafe(method(menu:willHighlightItem:))]
            fn will_highlight(&self, _menu: &NSMenu, item: Option<&NSMenuItem>) {
                let idx: isize = item
                    .and_then(|i| i.representedObject())
                    .and_then(|o| o.downcast::<objc2_foundation::NSNumber>().ok())
                    .map(|n| n.integerValue())
                    .unwrap_or(-1);
                if idx == self.ivars().current.get() {
                    return;
                }
                self.ivars().current.set(idx);
                let f = if idx >= 0 { self.ivars().hovers.borrow().get(idx as usize).cloned() } else { None };
                match (f, &self.ivars().restore) {
                    (Some(f), Some(r)) => {
                        r();
                        f();
                    }
                    (Some(f), None) => f(),
                    (None, Some(r)) => r(),
                    (None, None) => {}
                }
            }

            #[unsafe(method(menuDidClose:))]
            fn menu_did_close(&self, _menu: &NSMenu) {
                self.ivars().current.set(-1);
                if let Some(r) = &self.ivars().restore {
                    r();
                }
            }
        }
    );

    /// Tag a row with its hover closure index.
    fn set_hover(item: &NSMenuItem, hover: Option<Rc<dyn Fn()>>, hv: &mut Vec<Rc<dyn Fn()>>) {
        if let Some(h) = hover {
            let n = objc2_foundation::NSNumber::new_isize(hv.len() as isize);
            unsafe { item.setRepresentedObject(Some(&n)) };
            hv.push(h);
        }
    }

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
        keep: &mut Vec<Retained<MenuHover>>,
        hv: &mut Vec<Rc<dyn Fn()>>,
    ) {
        for e in entries {
            match e {
                MenuEntry::Separator => menu.addItem(&NSMenuItem::separatorItem(mtm)),
                MenuEntry::Header(title) => {
                    let item = NSMenuItem::sectionHeaderWithTitle(&NSString::from_str(&title), mtm);
                    menu.addItem(&item);
                }
                MenuEntry::Submenu { title, checked, swatch, hover, restore, entries } => {
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
                    set_hover(&item, hover, hv);
                    let sub = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(&title));
                    sub.setAutoenablesItems(false);
                    // A submenu with a restore hook previews its rows: its own
                    // delegate owns the rows' hover closures.
                    if restore.is_some() {
                        let mut local: Vec<Rc<dyn Fn()>> = Vec::new();
                        fill(mtm, &sub, entries, target, actions, keep, &mut local);
                        let d = MenuHover::alloc(mtm).set_ivars(HoverIvars {
                            hovers: RefCell::new(local),
                            restore,
                            current: Cell::new(-1),
                        });
                        let d: Retained<MenuHover> = unsafe { msg_send![super(d), init] };
                        sub.setDelegate(Some(objc2::runtime::ProtocolObject::from_ref(&*d)));
                        keep.push(d);
                    } else {
                        fill(mtm, &sub, entries, target, actions, keep, hv);
                    }
                    item.setSubmenu(Some(&sub));
                    menu.addItem(&item);
                }
                MenuEntry::Item { title, checked, enabled, swatch, hover, action } => {
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
                    set_hover(&item, hover, hv);
                    menu.addItem(&item);
                    actions.push(Some(action));
                }
            }
        }
    }

    /// Pop `entries` at `(x, y)` in `view` coordinates (or screen coordinates
    /// when `view` is `None`), then run the chosen closure.
    pub fn popup(entries: Vec<MenuEntry>, view: Option<&NSView>, x: f64, y: f64) {
        // Opened from the right-click itself: show it as a native context
        // menu. Positioned at the point instead, its first item sat under the
        // pointer, so the next click (or a second right-click) chose "Open".
        if view.is_none() {
            if let Some(mtm) = MainThreadMarker::new() {
                let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
                if let Some(event) = app.currentEvent() {
                    let uptime = objc2_foundation::NSProcessInfo::processInfo().systemUptime();
                    let right = event.r#type() == objc2_app_kit::NSEventType::RightMouseDown && uptime - event.timestamp() < 0.5;
                    if let (true, Some(cv)) = (right, event.window(mtm).and_then(|w| w.contentView())) {
                        run(entries, |menu| NSMenu::popUpContextMenu_withEvent_forView(menu, &event, &cv));
                        return;
                    }
                }
            }
        }
        run(entries, |menu| {
            menu.popUpMenuPositioningItem_atLocation_inView(None, NSPoint::new(x, y), view);
        });
    }

    /// [`popup`] at a screen point, never as a context menu — for menus
    /// opened after the click was consumed (the current event is stale).
    pub fn popup_at(entries: Vec<MenuEntry>, x: f64, y: f64) {
        run(entries, |menu| {
            menu.popUpMenuPositioningItem_atLocation_inView(None, NSPoint::new(x, y), None);
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
        let mut keep: Vec<Retained<MenuHover>> = Vec::new();
        let mut hv: Vec<Rc<dyn Fn()>> = Vec::new();
        fill(mtm, &menu, entries, &target, &mut actions, &mut keep, &mut hv);
        show(&menu);
        if let Some(i) = target.ivars().chosen.get() {
            if let Some(Some(a)) = actions.get_mut(i as usize).map(|a| a.take()) {
                a();
            }
        }
    }
}
