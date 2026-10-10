//! `CardWindow.swift`, ported: `CardNSWindow` (a titled window with a hidden
//! titlebar, a `_cornerRadius` override and header clicks caught in
//! `sendEvent:`) and `CardWindowController` (a themed blur-card root, the
//! shared key routing incl. Ctrl+Tab / Cmd+W and the `editKey` rule).
//!
//! The key decision is a pure [`route_card_key`] over a [`CardRouteContext`]
//! so it is unit-testable without AppKit; the AppKit side is
//! [`CardNSWindow::create`] / [`create_card_window`] and
//! [`CardWindowController::themed_root`].

use crate::ui::chrome::{ChromeConfig, PopupChrome};
use crate::ui::popup::{
    CardFrame, KeyInput, KEY_A, KEY_BACKSLASH, KEY_C, KEY_TAB, KEY_V, KEY_W, KEY_X, KEY_Z,
};

// ---------------------------------------------------------------------------
// Pure key routing (mirrors `CardWindowController.routeKey` / `editKey`).
// ---------------------------------------------------------------------------

/// `JiraEditKeys`-style edit operations routed through the controller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardEditOp {
    SelectAll,
    Copy,
    Paste,
    Cut,
    Undo,
    Redo,
}

/// The outcome of routing one key through a card window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardKeyAction {
    /// The event is returned unchanged (not consumed).
    Pass,
    /// `confirmCard.handleKey` consumed it.
    ConfirmHandled,
    /// `PopupWindow.keyInterceptor` consumed it.
    InterceptorConsumed,
    /// `keyBeforeSheet` consumed it.
    KeyBeforeSheetConsumed,
    /// `editKey` into the attached sheet (Ctrl+C/V, Cmd+Z …).
    SheetEdit(CardEditOp),
    /// The sheet is not key: the event propagates.
    SheetPass,
    /// The shortcuts card consumed it.
    ShortcutsHandled,
    /// Ctrl+Tab: switch view by the given direction.
    CycleView(i32),
    /// Cmd+W: close (or hide) the window.
    Close,
    /// Cmd+\: toggle the sidebar rail.
    ToggleSidebarRail,
    /// The subclass `handleKey` consumed it.
    SubclassHandled,
    /// A routed edit op (Ctrl+C/V, Cmd+Z) that never passes to the menu.
    Edit(CardEditOp),
}

/// The live guards `routeKey` reads, kept explicit so routing is pure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CardRouteContext {
    pub key_window: bool,
    pub has_attached_sheet: bool,
    pub sheet_is_key: bool,
    pub has_confirm_card: bool,
    /// Whether `confirmCard` would consume the key (it owns every key when up).
    pub confirm_handles: bool,
    pub has_shortcuts_card: bool,
    pub interceptor_handles: bool,
    pub key_before_sheet: bool,
    pub has_cycle_view_hook: bool,
    pub rail_toggle_succeeds: bool,
    pub app_active: bool,
    pub first_responder_is_text: bool,
    pub subclass_handles: bool,
}

impl Default for CardRouteContext {
    fn default() -> Self {
        CardRouteContext {
            key_window: true,
            has_attached_sheet: false,
            sheet_is_key: false,
            has_confirm_card: false,
            confirm_handles: false,
            has_shortcuts_card: false,
            interceptor_handles: false,
            key_before_sheet: false,
            has_cycle_view_hook: false,
            rail_toggle_succeeds: false,
            app_active: true,
            first_responder_is_text: false,
            subclass_handles: false,
        }
    }
}

/// `CardWindowController.routeKey`, as a pure function.
pub fn route_card_key(ctx: &CardRouteContext, key: KeyInput) -> CardKeyAction {
    if ctx.has_confirm_card && ctx.key_window {
        if ctx.confirm_handles {
            return CardKeyAction::ConfirmHandled;
        }
        return edit_key_route(ctx, key);
    }
    if ctx.key_window && !ctx.has_attached_sheet && ctx.interceptor_handles {
        return CardKeyAction::InterceptorConsumed;
    }
    if ctx.key_before_sheet {
        return CardKeyAction::KeyBeforeSheetConsumed;
    }
    if ctx.has_attached_sheet {
        return if ctx.sheet_is_key {
            edit_key_route(ctx, key)
        } else {
            CardKeyAction::SheetPass
        };
    }
    if !ctx.key_window {
        return CardKeyAction::Pass;
    }
    if ctx.has_shortcuts_card {
        return CardKeyAction::ShortcutsHandled;
    }
    if key.ctrl && !key.cmd && key.key_code == KEY_TAB && ctx.has_cycle_view_hook {
        return CardKeyAction::CycleView(if key.shift { -1 } else { 1 });
    }
    if key.cmd && key.key_code == KEY_W {
        return CardKeyAction::Close;
    }
    if key.cmd && !key.shift && key.key_code == KEY_BACKSLASH && ctx.rail_toggle_succeeds {
        return CardKeyAction::ToggleSidebarRail;
    }
    if ctx.subclass_handles {
        return CardKeyAction::SubclassHandled;
    }
    edit_key_route(ctx, key)
}

/// `CardWindowController.editKey`: Cmd+A/X/C/V pass through to the Edit menu
/// while the app is active; Ctrl+C/V and Cmd+Z are routed to `JiraEditKeys`.
pub fn edit_key_route(ctx: &CardRouteContext, key: KeyInput) -> CardKeyAction {
    if key.cmd
        && !key.ctrl
        && ctx.app_active
        && matches!(key.key_code, KEY_A | KEY_X | KEY_C | KEY_V)
    {
        return CardKeyAction::Pass;
    }
    if (key.cmd || key.ctrl) && ctx.first_responder_is_text {
        return match key.key_code {
            KEY_V => CardKeyAction::Edit(CardEditOp::Paste),
            KEY_C => CardKeyAction::Edit(CardEditOp::Copy),
            KEY_A if key.cmd => CardKeyAction::Edit(CardEditOp::SelectAll),
            KEY_X => CardKeyAction::Edit(CardEditOp::Cut),
            KEY_Z if key.cmd => CardKeyAction::Edit(if key.shift {
                CardEditOp::Redo
            } else {
                CardEditOp::Undo
            }),
            _ => CardKeyAction::Pass,
        };
    }
    CardKeyAction::Pass
}

// ---------------------------------------------------------------------------
// Window config + AppKit window.
// ---------------------------------------------------------------------------

/// Construction parameters for [`create_card_window`].
#[derive(Clone, Debug)]
pub struct CardConfig {
    pub frame: CardFrame,
    pub title: String,
    pub min_size: (f64, f64),
    pub corner_radius: f64,
    pub header_band: f64,
}

impl Default for CardConfig {
    fn default() -> Self {
        CardConfig {
            frame: CardFrame::default(),
            title: "kitchen-sink".to_string(),
            min_size: (320.0, 220.0),
            corner_radius: 10.0,
            header_band: 0.0,
        }
    }
}

#[cfg(target_os = "macos")]
#[allow(unused_imports)]
pub use appkit::{create_card_window, CardNSWindow, CardWindowController};

#[cfg(target_os = "macos")]
mod appkit {
    use super::*;
    use crate::ui::chrome::Rect;
    use crate::ui::theme::Rgba;
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{
        define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly, Message,
    };
    use objc2_app_kit::{
        NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
        NSAutoresizingMaskOptions, NSBackingStoreType, NSColor, NSImage, NSVisualEffectBlendingMode,
        NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView, NSWindow,
        NSWindowButton, NSWindowCollectionBehavior, NSWindowStyleMask, NSWindowTitleVisibility,
        NSText,
    };
    use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    fn as_any<T: Message + ?Sized>(obj: &T) -> &AnyObject {
        unsafe { &*(obj as *const T as *const AnyObject) }
    }

    pub struct CardNSWindowIvars {
        corner_radius: Cell<f64>,
        header_band: Cell<f64>,
        on_header_click: RefCell<Option<Box<dyn Fn(f64, f64)>>>,
        click_down: Cell<Option<(f64, f64)>>,
    }

    define_class!(
        #[unsafe(super(NSWindow))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSCardNSWindow"]
        #[ivars = CardNSWindowIvars]
        pub struct CardNSWindow;

        impl CardNSWindow {
            #[unsafe(method(_cornerRadius))]
            fn _corner_radius(&self) -> f64 {
                self.ivars().corner_radius.get()
            }

            #[unsafe(method(canBecomeKeyWindow))]
            fn can_become_key(&self) -> bool {
                true
            }

            #[unsafe(method(canBecomeMainWindow))]
            fn can_become_main(&self) -> bool {
                true
            }

            #[unsafe(method(sendEvent:))]
            fn send_event(&self, event: &objc2_app_kit::NSEvent) {
                self.track_header_click(event);
                unsafe { msg_send![super(self), sendEvent: event] }
            }
        }

        unsafe impl NSObjectProtocol for CardNSWindow {}
    );

    impl CardNSWindow {
        /// `CardNSWindow(contentRect:styleMask:backing:defer:)` with the
        /// hidden-titlebar card setup.
        pub fn create(mtm: MainThreadMarker, config: &CardConfig) -> Retained<CardNSWindow> {
            let frame = NSRect::new(
                NSPoint::new(config.frame.x, config.frame.y),
                NSSize::new(config.frame.width, config.frame.height),
            );
            let mask = NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Resizable
                | NSWindowStyleMask::Miniaturizable
                | NSWindowStyleMask::FullSizeContentView;
            let this = CardNSWindow::alloc(mtm).set_ivars(CardNSWindowIvars {
                corner_radius: Cell::new(config.corner_radius),
                header_band: Cell::new(config.header_band),
                on_header_click: RefCell::new(None),
                click_down: Cell::new(None),
            });
            let window: Retained<CardNSWindow> = unsafe {
                msg_send![
                    super(this),
                    initWithContentRect: frame,
                    styleMask: mask,
                    backing: NSBackingStoreType::Buffered,
                    defer: false
                ]
            };
            window.setTitle(&NSString::from_str(&config.title));
            unsafe { window.setReleasedWhenClosed(false) };
            window.setMinSize(NSSize::new(config.min_size.0, config.min_size.1));
            window.setTitlebarAppearsTransparent(true);
            window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
            window.setOpaque(false);
            window.setBackgroundColor(Some(&NSColor::clearColor()));
            window.setHasShadow(true);
            window.setMovableByWindowBackground(true);
            window.setAcceptsMouseMovedEvents(true);
            window.setCollectionBehavior(
                NSWindowCollectionBehavior::CanJoinAllSpaces
                    | NSWindowCollectionBehavior::FullScreenAuxiliary,
            );
            for b in [
                NSWindowButton::CloseButton,
                NSWindowButton::MiniaturizeButton,
                NSWindowButton::ZoomButton,
            ] {
                if let Some(btn) = window.standardWindowButton(b) {
                    btn.setHidden(true);
                }
            }
            window
        }

        pub fn corner_radius(&self) -> f64 {
            self.ivars().corner_radius.get()
        }

        pub fn set_corner_radius(&self, r: f64) {
            self.ivars().corner_radius.set(r);
            self.invalidateShadow();
        }

        pub fn header_band(&self) -> f64 {
            self.ivars().header_band.get()
        }

        pub fn set_header_band(&self, band: f64) {
            self.ivars().header_band.set(band);
        }

        pub fn set_on_header_click(&self, cb: Box<dyn Fn(f64, f64)>) {
            *self.ivars().on_header_click.borrow_mut() = Some(cb);
        }

        /// `HeaderClickTracker.track` — a click (≤ 4 pt travel) inside the
        /// header band reports the flipped point.
        fn track_header_click(&self, event: &objc2_app_kit::NSEvent) {
            let band = self.ivars().header_band.get();
            if band <= 0.0 {
                return;
            }
            let t = event.r#type();
            if t == objc2_app_kit::NSEventType::LeftMouseDown {
                let loc = event.locationInWindow();
                if loc.y >= self.frame().size.height - band {
                    let m = objc2_app_kit::NSEvent::mouseLocation();
                    self.ivars().click_down.set(Some((m.x, m.y)));
                }
            } else if t == objc2_app_kit::NSEventType::LeftMouseUp {
                if let Some(d) = self.ivars().click_down.take() {
                    let up = objc2_app_kit::NSEvent::mouseLocation();
                    if (up.x - d.0).abs() < 4.0 && (up.y - d.1).abs() < 4.0 {
                        let loc = event.locationInWindow();
                        let y = self.frame().size.height - loc.y;
                        if let Some(cb) = self.ivars().on_header_click.borrow().as_ref() {
                            cb(loc.x, y);
                        }
                    }
                }
            }
        }
    }

    /// `create_card_window` — build a [`CardNSWindow`] from a [`CardConfig`].
    pub fn create_card_window(mtm: MainThreadMarker, config: &CardConfig) -> Retained<CardNSWindow> {
        CardNSWindow::create(mtm, config)
    }

    type VoidCallback = Rc<RefCell<Option<Box<dyn Fn()>>>>;
    type IntCallback = Rc<RefCell<Option<Box<dyn Fn(i32)>>>>;

    /// `CardWindowController` — owns the window, its chrome and the shared key
    /// routing. The `SwitcherController` coupling is replaced by callbacks.
    pub struct CardWindowController {
        pub window: Retained<CardNSWindow>,
        pub home_window: Retained<CardNSWindow>,
        pub chrome: Option<Retained<PopupChrome>>,
        pub on_slot_hide: VoidCallback,
        pub on_cycle_view: IntCallback,
        pub slot_nav_click: IntCallback,
        pub key_before_sheet: Option<Box<dyn Fn(&KeyInput) -> bool>>,
        pub subclass_handle_key: Option<Box<dyn Fn(&KeyInput) -> bool>>,
        pub interceptor_handles: Option<Box<dyn Fn(&KeyInput) -> bool>>,
        pub has_confirm_card: Cell<bool>,
        pub confirm_handles: Cell<bool>,
        pub has_shortcuts_card: Cell<bool>,
    }

    impl CardWindowController {
        pub fn new(
            mtm: MainThreadMarker,
            frame: CardFrame,
            title: &str,
            min_size: (f64, f64),
        ) -> Self {
            let config = CardConfig {
                frame,
                title: title.to_string(),
                min_size,
                ..Default::default()
            };
            let home = CardNSWindow::create(mtm, &config);
            CardWindowController {
                window: home.clone(),
                home_window: home,
                chrome: None,
                on_slot_hide: Rc::new(RefCell::new(None)),
                on_cycle_view: Rc::new(RefCell::new(None)),
                slot_nav_click: Rc::new(RefCell::new(None)),
                key_before_sheet: None,
                subclass_handle_key: None,
                interceptor_handles: None,
                has_confirm_card: Cell::new(false),
                confirm_handles: Cell::new(false),
                has_shortcuts_card: Cell::new(false),
            }
        }

        /// Read the live guards `routeKey` checks.
        pub fn context(&self, key: &KeyInput) -> CardRouteContext {
            let sheet = self.window.attachedSheet();
            let has_attached_sheet = sheet.is_some();
            let sheet_is_key = sheet.as_ref().map(|s| s.isKeyWindow()).unwrap_or(false);
            let first_responder_is_text = self
                .window
                .firstResponder()
                .map(|r| as_any(&*r).downcast_ref::<NSText>().is_some())
                .unwrap_or(false);
            let app_active = MainThreadMarker::new()
                .map(|m| objc2_app_kit::NSApplication::sharedApplication(m).isActive())
                .unwrap_or(false);
            CardRouteContext {
                key_window: self.window.isKeyWindow(),
                has_attached_sheet,
                sheet_is_key,
                has_confirm_card: self.has_confirm_card.get(),
                confirm_handles: self.confirm_handles.get(),
                has_shortcuts_card: self.has_shortcuts_card.get(),
                interceptor_handles: self
                    .interceptor_handles
                    .as_ref()
                    .map(|f| f(key))
                    .unwrap_or(false),
                key_before_sheet: self.key_before_sheet.as_ref().map(|f| f(key)).unwrap_or(false),
                has_cycle_view_hook: self.on_cycle_view.borrow().is_some(),
                rail_toggle_succeeds: true,
                app_active,
                first_responder_is_text,
                subclass_handles: self
                    .subclass_handle_key
                    .as_ref()
                    .map(|f| f(key))
                    .unwrap_or(false),
            }
        }

        pub fn route_key(&self, key: &KeyInput) -> CardKeyAction {
            route_card_key(&self.context(key), *key)
        }

        /// Apply a routed action; returns whether the event was consumed.
        pub fn handle_event(&mut self, key: &KeyInput) -> bool {
            match self.route_key(key) {
                CardKeyAction::Close => {
                    self.close_or_hide();
                    true
                }
                CardKeyAction::CycleView(dir) => {
                    if let Some(f) = self.on_cycle_view.borrow().as_ref() {
                        f(dir);
                    }
                    true
                }
                CardKeyAction::Pass | CardKeyAction::SheetPass => false,
                _ => true,
            }
        }

        /// `closeOrHide`.
        pub fn close_or_hide(&self) {
            if let Some(f) = self.on_slot_hide.borrow().as_ref() {
                f();
                return;
            }
            self.leave_window();
        }

        /// `leaveWindow` (the `slotDetach` half is a later cut).
        pub fn leave_window(&self) {
            self.window.orderOut(None);
        }

        /// `themedRoot(_:name:colors:headerColor:icon:title:minTint:)`: build
        /// the blur card + tint + header chrome + content root.
        pub fn themed_root(
            &mut self,
            content: &objc2_app_kit::NSView,
            name: &str,
            colors: crate::ui::theme::PopupColors,
            header_color: Rgba,
            icon: Option<Retained<NSImage>>,
            _title: &str,
            min_tint: f64,
        ) -> Retained<objc2_app_kit::NSView> {
            let mtm = MainThreadMarker::new().expect("themed_root on the main thread");
            let mut cfg = ChromeConfig::default();
            cfg.colors = colors;
            cfg.header_height = 30.0;
            cfg.title_pill = false;
            cfg.header_color = Some(header_color);
            let radius = cfg.corner_radius + 1.0;
            let header_height = cfg.header_height;
            let tint_alpha = cfg.tint_alpha;

            let window = self.home_window.clone();
            window.set_corner_radius(radius);
            window.set_header_band(header_height);
            window.setTitlebarAppearsTransparent(true);
            window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
            window.setOpaque(false);
            window.setBackgroundColor(Some(&NSColor::clearColor()));
            window.setHasShadow(true);
            for b in [
                NSWindowButton::CloseButton,
                NSWindowButton::MiniaturizeButton,
                NSWindowButton::ZoomButton,
            ] {
                if let Some(btn) = window.standardWindowButton(b) {
                    btn.setHidden(true);
                }
            }
            let appearance_name = unsafe {
                if colors.is_light() {
                    NSAppearanceNameAqua
                } else {
                    NSAppearanceNameDarkAqua
                }
            };
            if let Some(app) = NSAppearance::appearanceNamed(appearance_name) {
                window.setAppearance(Some(&app));
            }

            let frame = window.frame();
            let root = objc2_app_kit::NSView::initWithFrame(
                objc2_app_kit::NSView::alloc(mtm),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(frame.size.width, frame.size.height)),
            );
            root.setWantsLayer(true);
            if let Some(layer) = root.layer() {
                layer.setCornerRadius(radius);
                layer.setMasksToBounds(true);
            }

            let fx = objc2_app_kit::NSVisualEffectView::initWithFrame(
                NSVisualEffectView::alloc(mtm),
                root.bounds(),
            );
            fx.setMaterial(NSVisualEffectMaterial::HUDWindow);
            fx.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
            fx.setState(NSVisualEffectState::Active);
            fx.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewHeightSizable,
            );

            let tint = objc2_app_kit::NSView::initWithFrame(
                objc2_app_kit::NSView::alloc(mtm),
                root.bounds(),
            );
            tint.setWantsLayer(true);
            if let Some(layer) = tint.layer() {
                let fill = colors.base().with_alpha(tint_alpha.max(min_tint));
                layer.setBackgroundColor(Some(&fill.to_nscolor().CGColor()));
                layer.setBorderColor(Some(&colors.border.to_nscolor().CGColor()));
                layer.setBorderWidth(1.0);
                layer.setCornerRadius(radius);
            }

            let ch = PopupChrome::create(mtm, cfg);
            ch.set_drag_header_height(header_height);
            ch.set_header_icon(icon);
            ch.set_header_title(None);
            ch.set_copy_labels("", "");
            ch.setFrame(NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(root.bounds().size.width, header_height),
            ));
            ch.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);

            content.setFrame(NSRect::new(
                NSPoint::new(1.0, header_height),
                NSSize::new(
                    root.bounds().size.width - 2.0,
                    root.bounds().size.height - header_height - 1.0,
                ),
            ));
            content.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewHeightSizable,
            );

            root.addSubview(&fx);
            root.addSubview(&tint);
            root.addSubview(&ch);
            root.addSubview(content);
            window.setContentView(Some(&root));
            self.chrome = Some(ch.clone());

            let chrome = ch.clone();
            let on_hide = self.on_slot_hide.clone();
            let on_nav = self.slot_nav_click.clone();
            let handler: Box<dyn Fn(f64, f64)> = Box::new(move |x, y| {
                let close = chrome.close_button_rect().inset_by(-2.0, -2.0);
                if close.contains(x, y) {
                    if let Some(f) = on_hide.borrow().as_ref() {
                        f();
                    }
                    return;
                }
                let extra = chrome.extra_button_rects();
                if let Some((id, _)) = extra.iter().find(|(_, r)| r.contains(x, y)) {
                    if let Some(f) = on_nav.borrow().as_ref() {
                        f(*id);
                    }
                    return;
                }
                let icon_rect = chrome.icon_button_rect().inset_by(-4.0, -4.0);
                if chrome.header_icon().is_some() && icon_rect.contains(x, y) {
                    // `showIconMenu`: a later cut.
                }
            });
            window.set_on_header_click(handler);

            let _ = name;
            root
        }

        /// `setSlotNav(_:icons:icon:on:click:)`.
        pub fn set_slot_nav(
            &mut self,
            buttons: Vec<(String, i32)>,
            icons: Vec<crate::ui::chrome::NavIcon>,
            icon: Option<Retained<NSImage>>,
            on: Option<i32>,
            click: Option<Box<dyn Fn(i32)>>,
        ) {
            if let Some(ch) = &self.chrome {
                ch.set_extra_buttons(buttons);
                ch.set_nav_icons(icons);
                ch.set_nav_on(on);
                ch.set_header_icon(icon);
            }
            *self.slot_nav_click.borrow_mut() = click;
        }

        /// The visible close-button rect (used by tests / hit-testing).
        pub fn close_button_rect(&self) -> Rect {
            self.chrome
                .as_ref()
                .map(|c| c.close_button_rect())
                .unwrap_or(Rect::ZERO)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "macos")]
    use objc2::MainThreadMarker;

    fn key(code: u16) -> KeyInput {
        KeyInput::new(code)
    }

    fn ctx() -> CardRouteContext {
        CardRouteContext::default()
    }

    #[test]
    fn cmd_w_closes() {
        let mut k = key(KEY_W);
        k.cmd = true;
        assert_eq!(route_card_key(&ctx(), k), CardKeyAction::Close);
    }

    #[test]
    fn ctrl_tab_cycles_only_with_a_hook() {
        let mut k = key(KEY_TAB);
        k.ctrl = true;
        assert_eq!(route_card_key(&ctx(), k), CardKeyAction::Pass);

        let mut c = ctx();
        c.has_cycle_view_hook = true;
        assert_eq!(route_card_key(&c, k), CardKeyAction::CycleView(1));
        k.shift = true;
        assert_eq!(route_card_key(&c, k), CardKeyAction::CycleView(-1));
    }

    #[test]
    fn cmd_backslash_toggles_rail_when_it_succeeds() {
        let mut k = key(KEY_BACKSLASH);
        k.cmd = true;
        assert_eq!(route_card_key(&ctx(), k), CardKeyAction::Pass);
        let mut c = ctx();
        c.rail_toggle_succeeds = true;
        assert_eq!(route_card_key(&c, k), CardKeyAction::ToggleSidebarRail);
        // Cmd+Shift+\ is not the rail toggle.
        k.shift = true;
        assert_eq!(route_card_key(&c, k), CardKeyAction::Pass);
    }

    #[test]
    fn cmd_edit_keys_pass_through() {
        let c = ctx();
        for code in [KEY_A, KEY_X, KEY_C, KEY_V] {
            let mut k = key(code);
            k.cmd = true;
            assert_eq!(route_card_key(&c, k), CardKeyAction::Pass, "cmd {code}");
        }
    }

    #[test]
    fn ctrl_copy_and_cmd_z_are_routed() {
        let mut c = ctx();
        c.first_responder_is_text = true;

        let mut ctrl_c = key(KEY_C);
        ctrl_c.ctrl = true;
        assert_eq!(route_card_key(&c, ctrl_c), CardKeyAction::Edit(CardEditOp::Copy));

        let mut ctrl_v = key(KEY_V);
        ctrl_v.ctrl = true;
        assert_eq!(route_card_key(&c, ctrl_v), CardKeyAction::Edit(CardEditOp::Paste));

        let mut cmd_z = key(KEY_Z);
        cmd_z.cmd = true;
        assert_eq!(route_card_key(&c, cmd_z), CardKeyAction::Edit(CardEditOp::Undo));
        cmd_z.shift = true;
        assert_eq!(route_card_key(&c, cmd_z), CardKeyAction::Edit(CardEditOp::Redo));

        // Cmd+A still passes through (editKey's early return beats JiraEditKeys).
        let mut cmd_a = key(KEY_A);
        cmd_a.cmd = true;
        assert_eq!(route_card_key(&c, cmd_a), CardKeyAction::Pass);
    }

    #[test]
    fn ctrl_edit_keys_need_a_text_responder() {
        let mut c = ctx();
        c.first_responder_is_text = false;
        let mut ctrl_c = key(KEY_C);
        ctrl_c.ctrl = true;
        assert_eq!(route_card_key(&c, ctrl_c), CardKeyAction::Pass);
    }

    #[test]
    fn ctrl_a_is_not_select_all() {
        let mut c = ctx();
        c.first_responder_is_text = true;
        let mut ctrl_a = key(KEY_A);
        ctrl_a.ctrl = true;
        assert_eq!(route_card_key(&c, ctrl_a), CardKeyAction::Pass);
    }

    #[test]
    fn confirm_card_owns_keys_else_edits() {
        let mut c = ctx();
        c.has_confirm_card = true;
        c.confirm_handles = true;
        let mut cmd_w = key(KEY_W);
        cmd_w.cmd = true;
        assert_eq!(route_card_key(&c, cmd_w), CardKeyAction::ConfirmHandled);

        c.confirm_handles = false;
        c.first_responder_is_text = true;
        let mut ctrl_c = key(KEY_C);
        ctrl_c.ctrl = true;
        assert_eq!(route_card_key(&c, ctrl_c), CardKeyAction::Edit(CardEditOp::Copy));
    }

    #[test]
    fn sheet_routing() {
        let mut c = ctx();
        c.has_attached_sheet = true;
        c.sheet_is_key = false;
        assert_eq!(route_card_key(&c, key(KEY_W)), CardKeyAction::SheetPass);

        c.sheet_is_key = true;
        c.first_responder_is_text = true;
        let mut cmd_z = key(KEY_Z);
        cmd_z.cmd = true;
        assert_eq!(route_card_key(&c, cmd_z), CardKeyAction::Edit(CardEditOp::Undo));
    }

    #[test]
    fn interceptor_and_key_before_sheet_win() {
        let mut c = ctx();
        c.interceptor_handles = true;
        assert_eq!(
            route_card_key(&c, key(KEY_W)),
            CardKeyAction::InterceptorConsumed
        );
        // The interceptor wins when both fire.
        c.key_before_sheet = true;
        assert_eq!(
            route_card_key(&c, key(KEY_W)),
            CardKeyAction::InterceptorConsumed
        );
        // A sheet suppresses the interceptor; keyBeforeSheet still wins.
        c.interceptor_handles = false;
        assert_eq!(
            route_card_key(&c, key(KEY_W)),
            CardKeyAction::KeyBeforeSheetConsumed
        );
        c.has_attached_sheet = true;
        assert_eq!(
            route_card_key(&c, key(KEY_W)),
            CardKeyAction::KeyBeforeSheetConsumed
        );
    }

    #[test]
    fn not_key_window_passes() {
        let mut c = ctx();
        c.key_window = false;
        let mut cmd_w = key(KEY_W);
        cmd_w.cmd = true;
        assert_eq!(route_card_key(&c, cmd_w), CardKeyAction::Pass);
    }

    #[test]
    fn shortcuts_card_consumes() {
        let mut c = ctx();
        c.has_shortcuts_card = true;
        assert_eq!(route_card_key(&c, key(KEY_W)), CardKeyAction::ShortcutsHandled);
    }

    #[test]
    fn subclass_handles_before_edit() {
        let mut c = ctx();
        c.subclass_handles = true;
        assert_eq!(route_card_key(&c, key(KEY_W)), CardKeyAction::SubclassHandled);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn card_window_geometry_when_on_main_thread() {
        if MainThreadMarker::new().is_none() {
            return;
        }
        let mtm = MainThreadMarker::new().unwrap();
        let mut config = CardConfig::default();
        config.frame = CardFrame { x: 0.0, y: 0.0, width: 480.0, height: 360.0 };
        let window = create_card_window(mtm, &config);
        assert_eq!(window.corner_radius(), 10.0);
        window.set_corner_radius(11.0);
        assert_eq!(window.corner_radius(), 11.0);
        window.set_header_band(30.0);
        assert_eq!(window.header_band(), 30.0);
        assert!(window.canBecomeKeyWindow());
    }
}
