//! Menu-bar menu, ported from `kitchen_sink.swift`
//! (`AppDelegate.installStatusMenus` / `installMainMenu`, `MenuTarget`,
//! `menuNeedsUpdate`, `SwitcherController.addGlobalWindowItems`, the
//! `globalGroupTag` / `configIssuesTag` markers, `headerStyleMenuItem`,
//! `showJiraDashboard`, `toggleJiraPoll`).
//!
//! The structure is a plain [`MenuSpec`] so it can be checked without AppKit.
//! The AppKit half ([`build_status_menu`], [`install_status_item`],
//! [`install_main_menu`]) turns a spec into `NSMenu`s and routes clicks
//! through a [`MenuTarget`] `define_class!` callback. The Swift controller
//! methods (`toggleJiraPoll`, `showJiraDashboard`, `addGlobalWindowItems`…)
//! don't exist in the Rust port yet: the callback the host passes to
//! [`install_status_item`] is the seam where they will be dispatched.

use crate::ui::theme::HeaderStyle;

/// Swift `MenuTarget.configIssuesTag`.
pub const CONFIG_ISSUES_TAG: i64 = 7401;
/// Swift `MenuTarget.globalGroupTag` — the dynamic insertion marker.
pub const GLOBAL_GROUP_TAG: i64 = 7402;

/// The stable, testable id of every menu item (mirrors the Swift selector).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuAction {
    ShowConfigIssues,
    ToggleHideOnFocusLoss,
    SetHeaderStyle(HeaderStyle),
    ResetWindowSize,
    ResetWindowColors,
    CloseWindow,
    Quit,
    OpenSetup,
    ToggleTerminal,
    ToggleNotes,
    ToggleHealthChecks,
    ToggleJiraPoll,
    ToggleJiraWindow,
    OpenJiraDashboard,
    OpenConfluence,
    OpenConfluenceSetup,
    OpenAi,
    OpenCompare,
    OpenWindow,
    ToggleVimMode,
    ResetSettings,
}

impl MenuAction {
    /// `SetHeaderStyle` ids start here (one per `HeaderStyle::ALL`).
    pub const HEADER_STYLE_BASE: i64 = 100;

    pub fn id(self) -> i64 {
        match self {
            MenuAction::ShowConfigIssues => 1,
            MenuAction::ToggleHideOnFocusLoss => 2,
            MenuAction::ResetWindowSize => 3,
            MenuAction::ResetWindowColors => 4,
            MenuAction::CloseWindow => 5,
            MenuAction::Quit => 6,
            MenuAction::OpenSetup => 7,
            MenuAction::ToggleTerminal => 8,
            MenuAction::ToggleNotes => 9,
            MenuAction::ToggleHealthChecks => 10,
            MenuAction::ToggleJiraPoll => 11,
            MenuAction::ToggleJiraWindow => 12,
            MenuAction::OpenJiraDashboard => 13,
            MenuAction::OpenConfluence => 14,
            MenuAction::OpenConfluenceSetup => 15,
            MenuAction::OpenAi => 16,
            MenuAction::OpenCompare => 17,
            MenuAction::OpenWindow => 18,
            MenuAction::ToggleVimMode => 19,
            MenuAction::ResetSettings => 20,
            MenuAction::SetHeaderStyle(style) => {
                let idx = HeaderStyle::ALL.iter().position(|s| *s == style).unwrap_or(0);
                Self::HEADER_STYLE_BASE + idx as i64
            }
        }
    }

    pub fn from_id(id: i64) -> Option<MenuAction> {
        Some(match id {
            1 => MenuAction::ShowConfigIssues,
            2 => MenuAction::ToggleHideOnFocusLoss,
            3 => MenuAction::ResetWindowSize,
            4 => MenuAction::ResetWindowColors,
            5 => MenuAction::CloseWindow,
            6 => MenuAction::Quit,
            7 => MenuAction::OpenSetup,
            8 => MenuAction::ToggleTerminal,
            9 => MenuAction::ToggleNotes,
            10 => MenuAction::ToggleHealthChecks,
            11 => MenuAction::ToggleJiraPoll,
            12 => MenuAction::ToggleJiraWindow,
            13 => MenuAction::OpenJiraDashboard,
            14 => MenuAction::OpenConfluence,
            15 => MenuAction::OpenConfluenceSetup,
            16 => MenuAction::OpenAi,
            17 => MenuAction::OpenCompare,
            18 => MenuAction::OpenWindow,
            19 => MenuAction::ToggleVimMode,
            20 => MenuAction::ResetSettings,
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

/// The live values the menu reflects (mirrors the globals `menuNeedsUpdate`
/// reads: `settings.hideOnFocusLoss`, `HeaderStyle.current`,
/// `jiraEnabledInConfig()`, `confluenceEnabled()`, `aiEnabled()`,
/// `compareEnabled()`, `vimModeEnabled`, `configIssues`).
#[derive(Clone, Debug, PartialEq)]
pub struct MenuSettings {
    pub hide_on_focus_loss: bool,
    pub header_style: HeaderStyle,
    pub config_issues: usize,
    pub config_using_backup: bool,
    pub jira_enabled: bool,
    pub confluence_enabled: bool,
    pub ai_enabled: bool,
    pub compare_enabled: bool,
    pub has_notes: bool,
    pub terminal_shown: bool,
    pub notes_shown: bool,
    pub jira_shown: bool,
    pub health_checks_shown: bool,
    pub vim_mode: bool,
}

impl Default for MenuSettings {
    fn default() -> Self {
        MenuSettings {
            hide_on_focus_loss: true,
            header_style: HeaderStyle::Flat,
            config_issues: 0,
            config_using_backup: false,
            jira_enabled: true,
            confluence_enabled: true,
            ai_enabled: true,
            compare_enabled: true,
            has_notes: true,
            terminal_shown: false,
            notes_shown: false,
            jira_shown: false,
            health_checks_shown: false,
            vim_mode: false,
        }
    }
}

fn config_issues_title(settings: &MenuSettings) -> String {
    if settings.config_using_backup {
        "⚠ Config Invalid — Using Backup…".to_string()
    } else if settings.config_issues > 0 {
        format!("⚠ Config Warnings ({})…", settings.config_issues)
    } else {
        "Config Issues…".to_string()
    }
}

/// Swift `HeaderStyle.allCases` → the "Header Style ▸" rows.
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
        title: "Header Style".to_string(),
        tag: None,
        items,
    }
}

/// Swift `addGlobalWindowItems`: the "Global Window Options ▸" submenu, with
/// the "Header Style ▸" submenu nested inside it.
fn global_window_options(settings: &MenuSettings) -> SubmenuSpec {
    SubmenuSpec {
        title: "Global Window Options".to_string(),
        tag: Some(GLOBAL_GROUP_TAG),
        items: vec![
            MenuNode::Item(
                MenuItemSpec::action(
                    MenuAction::ToggleHideOnFocusLoss,
                    "Hide When Focus Is Lost",
                )
                .checked(settings.hide_on_focus_loss)
                .tooltip("On: the window hides when you switch to another app. Off: it stays until ✕, Cmd+W, the hotkey or Esc (when the view's \"Esc Hides Window\" is on)"),
            ),
            MenuNode::Submenu(header_style_submenu(settings.header_style)),
        ],
    }
}

/// Swift `installStatusMenus`'s "Settings ▸" submenu.
fn settings_submenu(settings: &MenuSettings) -> SubmenuSpec {
    let mut items = Vec::new();
    if settings.has_notes {
        items.push(MenuNode::Item(
            MenuItemSpec::action(MenuAction::ToggleVimMode, "Vim Mode (Notes)")
                .checked(settings.vim_mode),
        ));
    }
    items.push(MenuNode::Separator);
    items.push(MenuNode::Item(MenuItemSpec::action(
        MenuAction::ResetSettings,
        "Reset Settings to Defaults",
    )));
    SubmenuSpec {
        title: "Settings".to_string(),
        tag: None,
        items,
    }
}

/// Build the menu-bar menu (`installStatusMenus` + `addGlobalWindowItems`).
///
/// Order follows the task's App menu: the Jira trio, Setup, Compare, Kitchen
/// Sink, Quit last; the remaining `installStatusMenus` rows keep their
/// relative order (terminal, notes/health, confluence/ai, reset/close).
pub fn build_menu_spec(settings: &MenuSettings) -> MenuSpec {
    let mut items = Vec::new();

    items.push(MenuNode::Item(
        MenuItemSpec::action(MenuAction::ShowConfigIssues, config_issues_title(settings))
            .tag(CONFIG_ISSUES_TAG)
            .hidden(settings.config_issues == 0 && !settings.config_using_backup),
    ));

    items.push(MenuNode::Submenu(global_window_options(settings)));
    items.push(MenuNode::Separator);

    items.push(MenuNode::Item(MenuItemSpec::action(
        MenuAction::ToggleJiraPoll,
        if settings.jira_enabled {
            "Disable Jira"
        } else {
            "Enable Jira"
        },
    )));
    items.push(MenuNode::Item(
        MenuItemSpec::action(MenuAction::ToggleJiraWindow, "Toggle Jira Window")
            .key("j", Modifiers::CMD)
            .checked(settings.jira_shown)
            .hidden(!settings.jira_enabled),
    ));
    items.push(MenuNode::Item(
        MenuItemSpec::action(MenuAction::OpenJiraDashboard, "Open Jira Config Window")
            .hidden(!settings.jira_enabled),
    ));
    items.push(MenuNode::Separator);

    items.push(MenuNode::Item(MenuItemSpec::action(
        MenuAction::OpenSetup,
        "Setup & Health Check…",
    )));
    items.push(MenuNode::Separator);

    items.push(MenuNode::Item(
        MenuItemSpec::action(MenuAction::OpenCompare, "Compare…").hidden(!settings.compare_enabled),
    ));
    items.push(MenuNode::Separator);

    items.push(MenuNode::Item(
        MenuItemSpec::action(MenuAction::ToggleTerminal, "Toggle Terminal")
            .key("t", Modifiers::CMD_OPT)
            .checked(settings.terminal_shown),
    ));
    items.push(MenuNode::Separator);

    items.push(MenuNode::Item(
        MenuItemSpec::action(MenuAction::ToggleNotes, "Toggle Notes")
            .key("n", Modifiers::CMD)
            .checked(settings.notes_shown),
    ));
    items.push(MenuNode::Item(
        MenuItemSpec::action(MenuAction::ToggleHealthChecks, "Toggle Health Checks")
            .key("h", Modifiers::CMD)
            .checked(settings.health_checks_shown),
    ));
    items.push(MenuNode::Separator);

    items.push(MenuNode::Item(
        MenuItemSpec::action(MenuAction::OpenConfluence, "Confluence Search")
            .hidden(!settings.confluence_enabled),
    ));
    items.push(MenuNode::Item(
        MenuItemSpec::action(MenuAction::OpenConfluenceSetup, "Confluence Setup…")
            .hidden(!settings.confluence_enabled),
    ));
    items.push(MenuNode::Item(
        MenuItemSpec::action(MenuAction::OpenAi, "AI (Grammar Check…)").hidden(!settings.ai_enabled),
    ));
    items.push(MenuNode::Separator);

    items.push(MenuNode::Item(
        MenuItemSpec::action(MenuAction::ResetWindowSize, "Reset Default Size").key("0", Modifiers::CMD),
    ));
    items.push(MenuNode::Item(MenuItemSpec::action(
        MenuAction::ResetWindowColors,
        "Reset Default Colors",
    )));
    items.push(MenuNode::Item(
        MenuItemSpec::action(MenuAction::CloseWindow, "Close Window").key("w", Modifiers::CMD),
    ));
    items.push(MenuNode::Separator);

    // The app's own window (the Hyper+S palette's last row, "Kitchen Sink").
    items.push(MenuNode::Item(MenuItemSpec::action(
        MenuAction::OpenWindow,
        "Kitchen Sink",
    )));
    items.push(MenuNode::Separator);

    items.push(MenuNode::Submenu(settings_submenu(settings)));
    items.push(MenuNode::Separator);

    items.push(MenuNode::Item(
        MenuItemSpec::action(MenuAction::Quit, "Quit").key("q", Modifiers::CMD),
    ));

    MenuSpec { items }
}

#[cfg(target_os = "macos")]
#[allow(unused_imports)]
pub use appkit::{
    build_status_menu, install_main_menu, install_status_item, MenuTarget, StatusMenuHandle,
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
                    // A plain on/off row flips its own checkmark (Swift
                    // rebuilt the menu with the new `state:`).
                    if action == MenuAction::ToggleHideOnFocusLoss {
                        sender.setState(if sender.state() == NSControlStateValueOn {
                            NSControlStateValueOff
                        } else {
                            NSControlStateValueOn
                        });
                    }
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
    /// `MenuTarget` `@objc` methods (`toggleJiraPoll`, `showJiraDashboard`,
    /// `addGlobalWindowItems`…), which the Rust port doesn't have yet.
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

    fn add_reset_size(mtm: MainThreadMarker, menu: &NSMenu, target: Option<&MenuTarget>) {
        let item = add_leaf(
            mtm,
            menu,
            "Reset Window Size",
            target.map(|_| sel!(menuAction:)),
            "0",
        );
        if let Some(target) = target {
            item.setTag(MenuAction::ResetWindowSize.id() as isize);
            unsafe { item.setTarget(Some(as_any(target))) };
        }
        item.setToolTip(Some(ns_string!("Reset all windows to their default size")));
    }

    /// `installMainMenu` — the standard AppKit main menu (About / File / Edit
    /// / Window). Items without an explicit action ride the responder chain;
    /// "Reset Window Size" targets `target` when supplied.
    pub fn install_main_menu(mtm: MainThreadMarker, target: Option<&MenuTarget>) {
        let app = NSApplication::sharedApplication(mtm);
        let main_menu = NSMenu::new(mtm);

        let app_item = NSMenuItem::new(mtm);
        let app_menu = NSMenu::new(mtm);
        add_leaf(mtm, &app_menu, "About kitchen-sink", None, "");
        app_menu.addItem(&NSMenuItem::separatorItem(mtm));
        add_leaf(mtm, &app_menu, "Hide", Some(sel!(hide:)), "h");
        add_leaf(mtm, &app_menu, "Hide Others", Some(sel!(hideOtherApplications:)), "h");
        add_leaf(mtm, &app_menu, "Show All", Some(sel!(unhideAllApplications:)), "");
        app_menu.addItem(&NSMenuItem::separatorItem(mtm));
        add_leaf(mtm, &app_menu, "Quit", Some(sel!(terminate:)), "q");
        app_item.setSubmenu(Some(&app_menu));
        main_menu.addItem(&app_item);

        let file_item = NSMenuItem::new(mtm);
        let file_menu = NSMenu::new(mtm);
        file_menu.setTitle(ns_string!("File"));
        add_leaf(mtm, &file_menu, "Close Window", Some(sel!(performClose:)), "w");
        file_menu.addItem(&NSMenuItem::separatorItem(mtm));
        add_reset_size(mtm, &file_menu, target);
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
        add_reset_size(mtm, &window_menu, target);
        window_menu.addItem(&NSMenuItem::separatorItem(mtm));
        add_leaf(mtm, &window_menu, "Minimize", Some(sel!(performMiniaturize:)), "m");
        window_item.setSubmenu(Some(&window_menu));
        main_menu.addItem(&window_item);

        app.setMainMenu(Some(&main_menu));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_ids_round_trip() {
        let actions = [
            MenuAction::ShowConfigIssues,
            MenuAction::ToggleHideOnFocusLoss,
            MenuAction::ResetWindowSize,
            MenuAction::ResetWindowColors,
            MenuAction::CloseWindow,
            MenuAction::Quit,
            MenuAction::OpenSetup,
            MenuAction::ToggleTerminal,
            MenuAction::ToggleNotes,
            MenuAction::ToggleHealthChecks,
            MenuAction::ToggleJiraPoll,
            MenuAction::ToggleJiraWindow,
            MenuAction::OpenJiraDashboard,
            MenuAction::OpenConfluence,
            MenuAction::OpenConfluenceSetup,
            MenuAction::OpenAi,
            MenuAction::OpenCompare,
            MenuAction::OpenWindow,
            MenuAction::ToggleVimMode,
            MenuAction::ResetSettings,
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
        assert_eq!(MenuAction::from_id(-1), None);
        assert_eq!(MenuAction::from_id(999), None);
        assert_eq!(HeaderStyle::ALL.len(), 7);
        assert_eq!(
            MenuAction::SetHeaderStyle(HeaderStyle::Quiet).id(),
            MenuAction::HEADER_STYLE_BASE
        );
    }

    #[test]
    fn spec_contains_documented_items_in_order() {
        let spec = build_menu_spec(&MenuSettings::default());
        assert_eq!(
            spec.top_titles(),
            vec![
                "Config Issues…",
                "Global Window Options",
                "—",
                "Disable Jira",
                "Toggle Jira Window",
                "Open Jira Config Window",
                "—",
                "Setup & Health Check…",
                "—",
                "Compare…",
                "—",
                "Toggle Terminal",
                "—",
                "Toggle Notes",
                "Toggle Health Checks",
                "—",
                "Confluence Search",
                "Confluence Setup…",
                "AI (Grammar Check…)",
                "—",
                "Reset Default Size",
                "Reset Default Colors",
                "Close Window",
                "—",
                "Kitchen Sink",
                "—",
                "Settings",
                "—",
                "Quit",
            ]
        );

        // The task's documented actions are all present...
        let actions: Vec<MenuAction> = spec.flatten().iter().map(|item| item.action).collect();
        for required in [
            MenuAction::ToggleJiraPoll,
            MenuAction::ToggleJiraWindow,
            MenuAction::OpenJiraDashboard,
            MenuAction::OpenSetup,
            MenuAction::OpenCompare,
            MenuAction::OpenWindow,
            MenuAction::Quit,
        ] {
            assert!(actions.contains(&required), "missing {required:?}");
        }
        // ...and the two documented submenus exist (Header Style nested).
        assert!(spec.find_submenu("Global Window Options").is_some());
        assert!(spec.find_submenu("Header Style").is_some());

        // Relative order among the required rows (the task's App menu:
        // Jira trio, Setup, Compare, Kitchen Sink, Quit last).
        let pos = |a: MenuAction| actions.iter().position(|x| *x == a).unwrap();
        assert!(pos(MenuAction::ToggleJiraPoll) < pos(MenuAction::ToggleJiraWindow));
        assert!(pos(MenuAction::ToggleJiraWindow) < pos(MenuAction::OpenJiraDashboard));
        assert!(pos(MenuAction::OpenJiraDashboard) < pos(MenuAction::OpenSetup));
        assert!(pos(MenuAction::OpenSetup) < pos(MenuAction::OpenCompare));
        assert!(pos(MenuAction::OpenCompare) < pos(MenuAction::OpenWindow));
        assert!(pos(MenuAction::OpenWindow) < pos(MenuAction::Quit));
    }

    #[test]
    fn header_style_items_populated_from_presets() {
        let settings = MenuSettings {
            header_style: HeaderStyle::Aurora,
            ..MenuSettings::default()
        };
        let spec = build_menu_spec(&settings);

        let global = spec
            .find_submenu("Global Window Options")
            .expect("global window options submenu");
        assert_eq!(global.tag, Some(GLOBAL_GROUP_TAG));
        let hide = match &global.items[0] {
            MenuNode::Item(item) => item,
            other => panic!("expected the hide item, got {other:?}"),
        };
        assert_eq!(hide.action, MenuAction::ToggleHideOnFocusLoss);
        assert!(hide.checked);
        assert!(matches!(global.items[1], MenuNode::Submenu(_)));

        let header = spec
            .find_submenu("Header Style")
            .expect("header style submenu");
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

    #[test]
    fn jira_titles_and_visibility_follow_settings() {
        let off = build_menu_spec(&MenuSettings {
            jira_enabled: false,
            ..MenuSettings::default()
        });
        let by_action = |spec: &MenuSpec, action: MenuAction| -> MenuItemSpec {
            spec.flatten()
                .into_iter()
                .find(|item| item.action == action)
                .unwrap_or_else(|| panic!("missing {action:?}"))
                .clone()
        };
        let toggle = by_action(&off, MenuAction::ToggleJiraPoll);
        assert_eq!(toggle.title, "Enable Jira");
        assert!(by_action(&off, MenuAction::ToggleJiraWindow).hidden);
        assert!(by_action(&off, MenuAction::OpenJiraDashboard).hidden);

        let on = build_menu_spec(&MenuSettings {
            jira_enabled: true,
            ..MenuSettings::default()
        });
        let toggle = by_action(&on, MenuAction::ToggleJiraPoll);
        assert_eq!(toggle.title, "Disable Jira");
        assert!(!by_action(&on, MenuAction::ToggleJiraWindow).hidden);
        assert!(!by_action(&on, MenuAction::OpenJiraDashboard).hidden);
    }

    #[test]
    fn feature_items_hide_when_disabled() {
        let spec = build_menu_spec(&MenuSettings {
            confluence_enabled: false,
            ai_enabled: false,
            compare_enabled: false,
            ..MenuSettings::default()
        });
        for action in [
            MenuAction::OpenConfluence,
            MenuAction::OpenConfluenceSetup,
            MenuAction::OpenAi,
            MenuAction::OpenCompare,
        ] {
            let item = spec
                .flatten()
                .into_iter()
                .find(|item| item.action == action)
                .unwrap();
            assert!(item.hidden, "{action:?} should be hidden");
        }
    }

    #[test]
    fn config_issues_row_reflects_state() {
        let spec = build_menu_spec(&MenuSettings {
            config_issues: 3,
            ..MenuSettings::default()
        });
        let item = spec
            .flatten()
            .into_iter()
            .find(|item| item.action == MenuAction::ShowConfigIssues)
            .unwrap();
        assert_eq!(item.title, "⚠ Config Warnings (3)…");
        assert_eq!(item.tag, Some(CONFIG_ISSUES_TAG));
        assert!(!item.hidden);

        let backup = build_menu_spec(&MenuSettings {
            config_using_backup: true,
            ..MenuSettings::default()
        });
        let item = backup
            .flatten()
            .into_iter()
            .find(|item| item.action == MenuAction::ShowConfigIssues)
            .unwrap();
        assert_eq!(item.title, "⚠ Config Invalid — Using Backup…");
        assert!(!item.hidden);
    }

    #[test]
    fn settings_submenu_follows_has_notes() {
        let with_notes = build_menu_spec(&MenuSettings::default());
        let settings = with_notes.find_submenu("Settings").unwrap();
        assert!(matches!(settings.items[0], MenuNode::Item(ref i) if i.action == MenuAction::ToggleVimMode));

        let without = build_menu_spec(&MenuSettings {
            has_notes: false,
            ..MenuSettings::default()
        });
        let settings = without.find_submenu("Settings").unwrap();
        assert!(!settings
            .items
            .iter()
            .any(|n| matches!(n, MenuNode::Item(i) if i.action == MenuAction::ToggleVimMode)));
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
