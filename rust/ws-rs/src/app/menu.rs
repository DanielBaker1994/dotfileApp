//! Menu-bar menu, ported from `kitchen_sink.swift`
//! (`AppDelegate.installStatusMenus` / `installMainMenu`, `MenuTarget`,
//! `menuNeedsUpdate`, `headerStyleMenuItem`, `showJiraDashboard`,
//! `toggleJiraPoll`).
//!
//! The structure is a plain [`MenuSpec`] so it can be checked without AppKit.
//! The AppKit half ([`build_status_menu`], [`install_status_item`],
//! [`install_main_menu`]) turns a spec into `NSMenu`s and routes clicks
//! through a [`MenuTarget`] `define_class!` callback. One [`MenuTarget`]
//! drives both the status item and the main menu's File ▸ "Reset Window Size"
//! row; File ▸ "Close Window" stays on the responder chain. The Swift
//! controller methods (`toggleJiraPoll`, `showJiraDashboard`…) don't exist in
//! the Rust port yet: the callback the host passes is where the ported ones
//! are dispatched.

use crate::ui::theme::HeaderStyle;

/// The stable, testable id of every menu item (mirrors the Swift selector).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    SetHeaderStyle(HeaderStyle),
    ResetWindowSize,
    CloseWindow,
    Quit,
}

impl MenuAction {
    /// `SetHeaderStyle` ids start here (one per `HeaderStyle::ALL`).
    pub const HEADER_STYLE_BASE: i64 = 100;

    pub fn id(self) -> i64 {
        match self {
            MenuAction::ResetWindowSize => 2,
            MenuAction::CloseWindow => 3,
            MenuAction::Quit => 4,
            MenuAction::SetHeaderStyle(style) => {
                let idx = HeaderStyle::ALL.iter().position(|s| *s == style).unwrap_or(0);
                Self::HEADER_STYLE_BASE + idx as i64
            }
        }
    }

    pub fn from_id(id: i64) -> Option<MenuAction> {
        Some(match id {
            // 1 was `ToggleHideOnFocusLoss`; hide-on-focus-loss is not ported
            // (see "Known gaps" in `AGENTS.md`), so the id is retired.
            2 => MenuAction::ResetWindowSize,
            3 => MenuAction::CloseWindow,
            4 => MenuAction::Quit,
            i if i >= Self::HEADER_STYLE_BASE => {
                let idx = (i - Self::HEADER_STYLE_BASE) as usize;
                MenuAction::SetHeaderStyle(*HeaderStyle::ALL.get(idx)?)
            }
            _ => return None,
        })
    }
}

/// The AppKit modifier mask, minus the objective-c type (testable).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub command: bool,
    pub option: bool,
    pub shift: bool,
    pub control: bool,
}

impl Modifiers {
    pub const NONE: Modifiers = Modifiers {
        command: false,
        option: false,
        shift: false,
        control: false,
    };
    pub const CMD: Modifiers = Modifiers {
        command: true,
        option: false,
        shift: false,
        control: false,
    };
    pub const CMD_OPT: Modifiers = Modifiers {
        command: true,
        option: true,
        shift: false,
        control: false,
    };
}

/// One actionable menu row.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuItemSpec {
    pub action: MenuAction,
    pub title: String,
    pub key_equivalent: String,
    pub modifiers: Modifiers,
    pub checked: bool,
    pub enabled: bool,
    pub hidden: bool,
    pub tag: Option<i64>,
    pub tooltip: Option<String>,
}

impl MenuItemSpec {
    pub fn action(action: MenuAction, title: impl Into<String>) -> Self {
        MenuItemSpec {
            action,
            title: title.into(),
            key_equivalent: String::new(),
            modifiers: Modifiers::NONE,
            checked: false,
            enabled: true,
            hidden: false,
            tag: None,
            tooltip: None,
        }
    }

    pub fn key(mut self, key: impl Into<String>, modifiers: Modifiers) -> Self {
        self.key_equivalent = key.into();
        self.modifiers = modifiers;
        self
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    pub fn hidden(mut self, hidden: bool) -> Self {
        self.hidden = hidden;
        self
    }

    pub fn tag(mut self, tag: i64) -> Self {
        self.tag = Some(tag);
        self
    }

    pub fn tooltip(mut self, tooltip: impl Into<String>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }
}

/// A submenu parent (no action; nested rows live in `items`).
#[derive(Clone, Debug, PartialEq)]
pub struct SubmenuSpec {
    pub title: String,
    pub tag: Option<i64>,
    pub tooltip: Option<String>,
    pub items: Vec<MenuNode>,
}

/// One row of the menu tree.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuNode {
    Item(MenuItemSpec),
    Submenu(SubmenuSpec),
    Separator,
}

/// The whole menu, depth-first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MenuSpec {
    pub items: Vec<MenuNode>,
}

impl MenuSpec {
    /// Every actionable/spec item, depth-first (separators skipped).
    pub fn flatten(&self) -> Vec<&MenuItemSpec> {
        fn walk<'a>(nodes: &'a [MenuNode], out: &mut Vec<&'a MenuItemSpec>) {
            for node in nodes {
                match node {
                    MenuNode::Item(item) => out.push(item),
                    MenuNode::Submenu(sub) => walk(&sub.items, out),
                    MenuNode::Separator => {}
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.items, &mut out);
        out
    }

    /// Top-level labels (`—` marks a separator).
    pub fn top_titles(&self) -> Vec<String> {
        self.items
            .iter()
            .map(|node| match node {
                MenuNode::Item(item) => item.title.clone(),
                MenuNode::Submenu(sub) => sub.title.clone(),
                MenuNode::Separator => "—".to_string(),
            })
            .collect()
    }

    /// Find a submenu anywhere in the tree by title.
    pub fn find_submenu(&self, title: &str) -> Option<&SubmenuSpec> {
        fn walk<'a>(nodes: &'a [MenuNode], title: &str) -> Option<&'a SubmenuSpec> {
            for node in nodes {
                if let MenuNode::Submenu(sub) = node {
                    if sub.title == title {
                        return Some(sub);
                    }
                    if let Some(found) = walk(&sub.items, title) {
                        return Some(found);
                    }
                }
            }
            None
        }
        walk(&self.items, title)
    }
}

/// The live values the menu reflects (mirrors the global `menuNeedsUpdate`
/// reads: `HeaderStyle.current`).
#[derive(Clone, Debug, PartialEq)]
pub struct MenuSettings {
    pub header_style: HeaderStyle,
}

impl Default for MenuSettings {
    fn default() -> Self {
        MenuSettings {
            header_style: HeaderStyle::Flat,
        }
    }
}

/// Swift `HeaderStyle.allCases` → the "Header Bar Style ▸" rows.
fn header_style_submenu(current: HeaderStyle) -> SubmenuSpec {
    let items = HeaderStyle::ALL
        .iter()
        .map(|style| {
            MenuNode::Item(
                MenuItemSpec::action(MenuAction::SetHeaderStyle(*style), style.label())
                    .checked(*style == current),
            )
        })
        .collect();
    SubmenuSpec {
        title: "Header Bar Style".to_string(),
        tag: None,
        tooltip: Some(
            "The header bar's shape/treatment — separate from the colour Theme presets"
                .to_string(),
        ),
        items,
    }
}

/// Build the menu-bar menu (`installStatusMenus`).
///
/// Only the rows whose actions are actually wired (`main.rs::status_menu_action`)
/// are kept — the Swift menu's dead placeholder rows (Setup, Compare,
/// Confluence, AI, Health Checks, Reset Defaults, Vim Mode, Config Issues,
/// Jira poll/dashboard) and its per-view window toggles are gone; the palette
/// and the header nav cover those features.
pub fn build_menu_spec(settings: &MenuSettings) -> MenuSpec {
    let mut items = Vec::new();

    items.push(MenuNode::Submenu(header_style_submenu(settings.header_style)));
    items.push(MenuNode::Separator);

    items.push(MenuNode::Item(
        // No key equivalent: ⌘W belongs to File ▸ "Close Window" (the key
        // window handles it), so only one row owns it.
        MenuItemSpec::action(MenuAction::CloseWindow, "Close Window"),
    ));
    items.push(MenuNode::Separator);

    items.push(MenuNode::Item(
        MenuItemSpec::action(MenuAction::Quit, "Quit").key("q", Modifiers::CMD),
    ));

    MenuSpec { items }
}

#[cfg(target_os = "macos")]
#[allow(unused_imports)]
pub use appkit::{
    build_status_menu, install_main_menu, install_status_item, pop_status_menu, MenuTarget,
    StatusMenuHandle,
};

#[cfg(target_os = "macos")]
mod appkit {
    use super::*;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, NSObject, ProtocolObject, Sel};
    use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly, Message};
    use objc2_app_kit::{
        NSApplication, NSControlStateValueOff, NSControlStateValueOn, NSEventModifierFlags, NSMenu,
        NSMenuDelegate, NSMenuItem, NSStatusBar, NSStatusItem, NSSquareStatusItemLength,
    };
    use objc2_foundation::{ns_string, NSObjectProtocol, NSString};
    use std::cell::RefCell;

    fn as_any<T: Message + ?Sized>(obj: &T) -> &AnyObject {
        unsafe { &*(obj as *const T as *const AnyObject) }
    }

    fn ns_modifiers(modifiers: Modifiers) -> NSEventModifierFlags {
        let mut flags = NSEventModifierFlags::empty();
        if modifiers.command {
            flags |= NSEventModifierFlags::Command;
        }
        if modifiers.option {
            flags |= NSEventModifierFlags::Option;
        }
        if modifiers.shift {
            flags |= NSEventModifierFlags::Shift;
        }
        if modifiers.control {
            flags |= NSEventModifierFlags::Control;
        }
        flags
    }

    pub struct MenuTargetIvars {
        handler: RefCell<Option<Box<dyn Fn(MenuAction)>>>,
        on_needs_update: RefCell<Option<Box<dyn Fn()>>>,
    }

    define_class!(
        // `MenuTarget` used to be an `NSObject` singleton with one `@objc`
        // method per action; here every action shares `menuAction:`, decoded
        // from the sender's tag via `MenuAction::from_id`.
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSMenuTarget"]
        #[ivars = MenuTargetIvars]
        pub struct MenuTarget;

        impl MenuTarget {
            #[unsafe(method(menuAction:))]
            fn menu_action(&self, sender: &NSMenuItem) {
                if let Some(action) = MenuAction::from_id(sender.tag() as i64) {
                    if let Some(cb) = self.ivars().handler.borrow().as_ref() {
                        cb(action);
                    }
                }
            }

            // Required `NSMenuDelegate` method: rebuild the dynamic rows.
            #[unsafe(method(menuNeedsUpdate:))]
            fn menu_needs_update(&self, _menu: &NSMenu) {
                if let Some(cb) = self.ivars().on_needs_update.borrow().as_ref() {
                    cb();
                }
            }
        }

        unsafe impl NSObjectProtocol for MenuTarget {}
        unsafe impl NSMenuDelegate for MenuTarget {}
    );

    impl MenuTarget {
        pub fn new(mtm: MainThreadMarker, handler: impl Fn(MenuAction) + 'static) -> Retained<MenuTarget> {
            let this = MenuTarget::alloc(mtm).set_ivars(MenuTargetIvars {
                handler: RefCell::new(Some(Box::new(handler))),
                on_needs_update: RefCell::new(None),
            });
            unsafe { msg_send![super(this), init] }
        }

        /// The Swift `menuNeedsUpdate(_:)` hook (config-issue refresh, jira
        /// title/visibility, window states). Not ported yet: a host that
        /// wants it supplies the rebuild closure.
        pub fn set_needs_update_handler(&self, handler: impl Fn() + 'static) {
            *self.ivars().on_needs_update.borrow_mut() = Some(Box::new(handler));
        }
    }

    fn make_item(mtm: MainThreadMarker, node: &MenuNode, target: &MenuTarget) -> Retained<NSMenuItem> {
        match node {
            MenuNode::Separator => NSMenuItem::separatorItem(mtm),
            MenuNode::Submenu(sub) => {
                let item = unsafe {
                    NSMenuItem::initWithTitle_action_keyEquivalent(
                        NSMenuItem::alloc(mtm),
                        &NSString::from_str(&sub.title),
                        None,
                        ns_string!(""),
                    )
                };
                if let Some(tag) = sub.tag {
                    item.setTag(tag as isize);
                }
                if let Some(tip) = &sub.tooltip {
                    item.setToolTip(Some(&NSString::from_str(tip)));
                }
                let submenu = NSMenu::new(mtm);
                submenu.setAutoenablesItems(false);
                for child in &sub.items {
                    submenu.addItem(&make_item(mtm, child, target));
                }
                item.setSubmenu(Some(&submenu));
                item
            }
            MenuNode::Item(spec) => {
                let item = unsafe {
                    NSMenuItem::initWithTitle_action_keyEquivalent(
                        NSMenuItem::alloc(mtm),
                        &NSString::from_str(&spec.title),
                        Some(sel!(menuAction:)),
                        &NSString::from_str(&spec.key_equivalent),
                    )
                };
                unsafe { item.setTarget(Some(as_any(target))) };
                item.setTag(spec.tag.unwrap_or_else(|| spec.action.id()) as isize);
                item.setKeyEquivalentModifierMask(ns_modifiers(spec.modifiers));
                item.setEnabled(spec.enabled);
                item.setHidden(spec.hidden);
                item.setState(if spec.checked {
                    NSControlStateValueOn
                } else {
                    NSControlStateValueOff
                });
                if let Some(tip) = &spec.tooltip {
                    item.setToolTip(Some(&NSString::from_str(tip)));
                }
                item
            }
        }
    }

    /// Turn a [`MenuSpec`] into an `NSMenu` wired to `target`.
    pub fn build_status_menu(mtm: MainThreadMarker, spec: &MenuSpec, target: &MenuTarget) -> Retained<NSMenu> {
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        for node in &spec.items {
            menu.addItem(&make_item(mtm, node, target));
        }
        menu
    }

    thread_local! {
        /// The status menu, reused by the header app-icon button.
        static STATUS_MENU: std::cell::RefCell<Option<Retained<NSMenu>>> =
            const { std::cell::RefCell::new(None) };
    }

    /// Pop the status menu under `(x, y)` in `view` (Swift `popUpIconMenu`).
    pub fn pop_status_menu(view: &objc2_app_kit::NSView, x: f64, y: f64) {
        let Some(menu) = STATUS_MENU.with(|m| m.borrow().clone()) else {
            return;
        };
        menu.popUpMenuPositioningItem_atLocation_inView(
            None,
            objc2_foundation::NSPoint::new(x, y),
            Some(view),
        );
    }

    /// The live status item + the objects that must outlive it (`target` is a
    /// weak `NSMenuItem.target`, the menu a weak delegate).
    pub struct StatusMenuHandle {
        pub status_item: Retained<NSStatusItem>,
        pub target: Retained<MenuTarget>,
        pub menu: Retained<NSMenu>,
    }

    /// `installStatusMenus`: a square status item whose menu is `spec`.
    ///
    /// `handler` is the click-routing callback — it replaces the Swift
    /// `MenuTarget` `@objc` methods (`toggleJiraPoll`, `showJiraDashboard`…),
    /// which the Rust port doesn't have yet.
    pub fn install_status_item(
        mtm: MainThreadMarker,
        spec: &MenuSpec,
        handler: impl Fn(MenuAction) + 'static,
    ) -> StatusMenuHandle {
        let target = MenuTarget::new(mtm, handler);
        let menu = build_status_menu(mtm, spec, &target);
        menu.setDelegate(Some(ProtocolObject::from_ref(&*target)));

        let bar = NSStatusBar::systemStatusBar();
        let status_item = bar.statusItemWithLength(NSSquareStatusItemLength);
        if let Some(button) = status_item.button(mtm) {
            button.setToolTip(Some(ns_string!("kitchen-sink")));
            button.setTitle(ns_string!("☰"));
        }
        status_item.setMenu(Some(&menu));
        STATUS_MENU.with(|m| *m.borrow_mut() = Some(menu.clone()));

        StatusMenuHandle {
            status_item,
            target,
            menu,
        }
    }

    fn add_leaf(
        mtm: MainThreadMarker,
        menu: &NSMenu,
        title: &str,
        action: Option<Sel>,
        key: &str,
    ) -> Retained<NSMenuItem> {
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(title),
                action,
                &NSString::from_str(key),
            )
        };
        menu.addItem(&item);
        item
    }

    fn add_reset_size(mtm: MainThreadMarker, menu: &NSMenu, target: &MenuTarget) {
        let item = add_leaf(mtm, menu, "Reset Window Size", Some(sel!(menuAction:)), "0");
        item.setTag(MenuAction::ResetWindowSize.id() as isize);
        unsafe { item.setTarget(Some(as_any(target))) };
        item.setToolTip(Some(ns_string!("Reset all windows to their default size")));
    }

    /// `installMainMenu` — the standard AppKit main menu (About / File / Edit
    /// / Window). Items without an explicit action ride the responder chain, so
    /// File ▸ "Close Window" (⌘W) reaches the key window: `performClose:` on the
    /// shared window funnels to the controller's close request (hide), while
    /// transient overlays (tool panels, Quick Look, sheets) close themselves.
    ///
    /// Only File ▸ "Reset Window Size" goes through `target`; the row (and its
    /// separator) is omitted when `target` is `None` (the `prose` child).
    pub fn install_main_menu(mtm: MainThreadMarker, target: Option<&MenuTarget>) {
        let app = NSApplication::sharedApplication(mtm);
        let main_menu = NSMenu::new(mtm);

        let app_item = NSMenuItem::new(mtm);
        let app_menu = NSMenu::new(mtm);
        add_leaf(mtm, &app_menu, "About kitchen-sink", Some(sel!(orderFrontStandardAboutPanel:)), "");
        app_menu.addItem(&NSMenuItem::separatorItem(mtm));
        add_leaf(mtm, &app_menu, "Hide", Some(sel!(hide:)), "h");
        add_leaf(mtm, &app_menu, "Hide Others", Some(sel!(hideOtherApplications:)), "h")
            .setKeyEquivalentModifierMask(objc2_app_kit::NSEventModifierFlags::Command | objc2_app_kit::NSEventModifierFlags::Option);
        add_leaf(mtm, &app_menu, "Show All", Some(sel!(unhideAllApplications:)), "");
        app_menu.addItem(&NSMenuItem::separatorItem(mtm));
        add_leaf(mtm, &app_menu, "Quit", Some(sel!(terminate:)), "q");
        app_item.setSubmenu(Some(&app_menu));
        main_menu.addItem(&app_item);

        let file_item = NSMenuItem::new(mtm);
        let file_menu = NSMenu::new(mtm);
        file_menu.setTitle(ns_string!("File"));
        // File ▸ "Close Window" owns ⌘W and rides the responder chain: the key
        // window decides. `performClose:` on the shared window already funnels
        // to the controller's close request (hide), and it lets transient
        // overlays (tool panels, Quick Look, sheets) close themselves instead
        // of hiding the window behind them. Only "Reset Window Size" needs the
        // target; it is omitted for the target-less `prose` child.
        add_leaf(mtm, &file_menu, "Close Window", Some(sel!(performClose:)), "w");
        if let Some(target) = target {
            file_menu.addItem(&NSMenuItem::separatorItem(mtm));
            add_reset_size(mtm, &file_menu, target);
        }
        file_item.setSubmenu(Some(&file_menu));
        main_menu.addItem(&file_item);

        let edit_item = NSMenuItem::new(mtm);
        let edit_menu = NSMenu::new(mtm);
        edit_menu.setTitle(ns_string!("Edit"));
        add_leaf(mtm, &edit_menu, "Undo", Some(sel!(undo:)), "z");
        add_leaf(mtm, &edit_menu, "Redo", Some(sel!(redo:)), "Z");
        edit_menu.addItem(&NSMenuItem::separatorItem(mtm));
        add_leaf(mtm, &edit_menu, "Cut", Some(sel!(cut:)), "x");
        add_leaf(mtm, &edit_menu, "Copy", Some(sel!(copy:)), "c");
        add_leaf(mtm, &edit_menu, "Paste", Some(sel!(paste:)), "v");
        add_leaf(mtm, &edit_menu, "Select All", Some(sel!(selectAll:)), "a");
        edit_item.setSubmenu(Some(&edit_menu));
        main_menu.addItem(&edit_item);

        let window_item = NSMenuItem::new(mtm);
        let window_menu = NSMenu::new(mtm);
        window_menu.setTitle(ns_string!("Window"));
        add_leaf(mtm, &window_menu, "Minimize", Some(sel!(performMiniaturize:)), "m");
        add_leaf(mtm, &window_menu, "Zoom", Some(sel!(performZoom:)), "");
        window_menu.addItem(&NSMenuItem::separatorItem(mtm));
        add_leaf(mtm, &window_menu, "Bring All to Front", Some(sel!(arrangeInFront:)), "");
        window_item.setSubmenu(Some(&window_menu));
        main_menu.addItem(&window_item);

        app.setMainMenu(Some(&main_menu));
        // The Window menu proper: AppKit lists the open windows under it.
        app.setWindowsMenu(Some(&window_menu));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_ids_round_trip() {
        let actions = [
            MenuAction::ResetWindowSize,
            MenuAction::CloseWindow,
            MenuAction::Quit,
        ];
        let mut seen = std::collections::HashSet::new();
        for action in actions {
            let id = action.id();
            assert!(seen.insert(id), "duplicate id {id} for {action:?}");
            assert_eq!(MenuAction::from_id(id), Some(action));
        }
        for style in HeaderStyle::ALL {
            let action = MenuAction::SetHeaderStyle(style);
            assert_eq!(MenuAction::from_id(action.id()), Some(action));
        }
        // Id 1 (the retired hide-on-focus-loss toggle) no longer decodes.
        assert_eq!(MenuAction::from_id(1), None);
        assert_eq!(MenuAction::from_id(-1), None);
        assert_eq!(MenuAction::from_id(999), None);
        assert_eq!(HeaderStyle::ALL.len(), 7);
        assert_eq!(
            MenuAction::SetHeaderStyle(HeaderStyle::Quiet).id(),
            MenuAction::HEADER_STYLE_BASE
        );
    }

    #[test]
    fn spec_contains_the_ported_items_in_order() {
        let spec = build_menu_spec(&MenuSettings::default());
        assert_eq!(
            spec.top_titles(),
            vec!["Header Bar Style", "—", "Close Window", "—", "Quit"]
        );
        // The header-style submenu is a direct top-level node now that the
        // single-child "Global Window Options" wrapper is gone.
        assert!(matches!(spec.items[0], MenuNode::Submenu(_)));

        // Every row is actionable and wired to a handler in `main.rs`.
        let actions: Vec<MenuAction> = spec.flatten().iter().map(|item| item.action).collect();
        for required in [MenuAction::CloseWindow, MenuAction::Quit] {
            assert!(actions.contains(&required), "missing {required:?}");
        }
        // The one documented submenu exists.
        assert!(spec.find_submenu("Header Bar Style").is_some());

        let pos = |a: MenuAction| actions.iter().position(|x| *x == a).unwrap();
        assert!(pos(MenuAction::CloseWindow) < pos(MenuAction::Quit));

        // ⌘W is owned by File ▸ "Close Window" (`install_main_menu`), so the
        // status row carries no key equivalent; the reset row lives only in
        // the main menu, never in this spec.
        let close = spec
            .flatten()
            .into_iter()
            .find(|item| item.action == MenuAction::CloseWindow)
            .expect("close row");
        assert!(close.key_equivalent.is_empty());
        assert_eq!(close.modifiers, Modifiers::NONE);
        assert!(!actions.contains(&MenuAction::ResetWindowSize));
    }

    #[test]
    fn header_style_items_populated_from_presets() {
        let settings = MenuSettings {
            header_style: HeaderStyle::Aurora,
            ..MenuSettings::default()
        };
        let spec = build_menu_spec(&settings);

        let header = spec
            .find_submenu("Header Bar Style")
            .expect("header bar style submenu");
        assert_eq!(
            header.tooltip.as_deref(),
            Some("The header bar's shape/treatment — separate from the colour Theme presets")
        );
        assert_eq!(header.items.len(), HeaderStyle::ALL.len());
        let rows: Vec<&MenuItemSpec> = header
            .items
            .iter()
            .map(|node| match node {
                MenuNode::Item(item) => item,
                other => panic!("expected a header style item, got {other:?}"),
            })
            .collect();
        for (i, style) in HeaderStyle::ALL.iter().enumerate() {
            assert_eq!(rows[i].action, MenuAction::SetHeaderStyle(*style));
            assert_eq!(rows[i].title, style.label());
            assert_eq!(rows[i].checked, *style == HeaderStyle::Aurora);
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn appkit_menu_builds_when_on_main_thread() {
        let Some(mtm) = objc2::MainThreadMarker::new() else {
            return;
        };
        let spec = build_menu_spec(&MenuSettings::default());
        let target = MenuTarget::new(mtm, |_action| {});
        let menu = build_status_menu(mtm, &spec, &target);
        // Every top-level node (items + separators + submenus) became a row.
        assert_eq!(menu.numberOfItems(), spec.items.len() as isize);
    }
}
