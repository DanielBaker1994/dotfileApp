//! Window framework foundation, ported from `PopupWindow.swift`
//! (`PopupConfig`, `PopupBaseWindow`, `PopupPanel`, `PopupPlainWindow`,
//! `PopupWindow.makePanel` / `makeBackdrop` / `handleKey` / `installMonitors`).
//!
//! This first cut contains:
//! - a pure [`PopupConfig`] + [`route_key`] decision function mirroring the
//!   documented `handleKey` order (testable without AppKit);
//! - `PopupBaseWindow` / `PopupPanel` (`define_class!` over `NSWindow` /
//!   `NSPanel`, overriding the private `_cornerRadius`) and
//!   [`PopupPanel::create`];
//! - [`install_local_monitor`] — the `NSEvent.addLocalMonitorForEvents(matching:
//!   .keyDown)` guard whose handler routes into [`route_key`].

use objc2::MainThreadMarker;

// macOS virtual key codes (as used by `NSEvent.keyCode` in the Swift source).
pub const KEY_A: u16 = 0;
pub const KEY_S: u16 = 1;
pub const KEY_F: u16 = 3;
pub const KEY_H: u16 = 4;
pub const KEY_Z: u16 = 6;
pub const KEY_X: u16 = 7;
pub const KEY_C: u16 = 8;
pub const KEY_V: u16 = 9;
pub const KEY_W: u16 = 13;
pub const KEY_R: u16 = 15;
pub const KEY_PLUS: u16 = 24;
pub const KEY_MINUS: u16 = 27;
pub const KEY_ZERO: u16 = 29;
pub const KEY_O: u16 = 31;
pub const KEY_P: u16 = 35;
pub const KEY_RETURN: u16 = 36;
pub const KEY_L: u16 = 37;
pub const KEY_J: u16 = 38;
pub const KEY_K: u16 = 40;
pub const KEY_BACKSLASH: u16 = 42;
pub const KEY_SLASH: u16 = 44;
pub const KEY_N: u16 = 45;
pub const KEY_TAB: u16 = 48;
pub const KEY_SPACE: u16 = 49;
pub const KEY_ESC: u16 = 53;
pub const KEY_KEYPAD_PLUS: u16 = 69;
pub const KEY_KEYPAD_MINUS: u16 = 78;
pub const KEY_HOME: u16 = 115;
pub const KEY_PAGE_UP: u16 = 116;
pub const KEY_END: u16 = 119;
pub const KEY_PAGE_DOWN: u16 = 121;
pub const KEY_DOWN: u16 = 125;
pub const KEY_UP: u16 = 126;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CardFrame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Default for CardFrame {
    fn default() -> Self {
        CardFrame { x: 0.0, y: 0.0, width: 250.0, height: 420.0 }
    }
}

#[derive(Clone, Debug)]
pub struct PopupConfig {
    pub name: String,
    pub frame: CardFrame,
    pub corner_radius: f64,
    pub header_close_button: bool,
    pub edit_mode: bool,
    pub esc_close_count: i32,
    pub floating: bool,
    pub tool_panel: bool,

    // Generic card placeholders (parity surface for the window build, not yet
    // consumed by the pure key router beyond the fields listed below).
    pub has_shadow: bool,
    pub enable_resize: bool,
    pub enable_drag: bool,

    // Runtime context carried on the config so `route_key` stays a pure
    // function; these mirror the live guards in `PopupWindow.handleKey`.
    pub enable_navigation: bool,
    pub wrap_navigation: bool,
    pub enable_escape: bool,
    pub selectable_rows: bool,
    pub tabs: bool,
    pub tab_count: usize,
    pub has_cycle_view_hook: bool,
    pub sheet_active: bool,
    pub active_text_editor: bool,
    pub has_file_browser: bool,
    pub browser_has_focus: bool,
    pub browser_rename_active: bool,
    pub vim_focus: bool,
    pub terminal_focus: bool,
    pub vim_normal_mode: bool,
    pub find_bar_shown: bool,
}

impl Default for PopupConfig {
    fn default() -> Self {
        PopupConfig {
            name: "popup".to_string(),
            frame: CardFrame::default(),
            corner_radius: 9.0,
            header_close_button: true,
            edit_mode: false,
            esc_close_count: 1,
            floating: true,
            tool_panel: false,
            has_shadow: true,
            enable_resize: false,
            enable_drag: false,
            enable_navigation: true,
            wrap_navigation: true,
            enable_escape: true,
            selectable_rows: false,
            tabs: false,
            tab_count: 0,
            has_cycle_view_hook: false,
            sheet_active: false,
            active_text_editor: false,
            has_file_browser: false,
            browser_has_focus: false,
            browser_rename_active: false,
            vim_focus: false,
            terminal_focus: false,
            vim_normal_mode: false,
            find_bar_shown: false,
        }
    }
}

impl PopupConfig {
    pub fn new(name: impl Into<String>) -> Self {
        PopupConfig { name: name.into(), ..Default::default() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyInput {
    pub key_code: u16,
    pub chars: Option<char>,
    pub cmd: bool,
    pub ctrl: bool,
    pub opt: bool,
    pub shift: bool,
    /// Consecutive-Esc count INCLUDING this key when `key_code == KEY_ESC`;
    /// the caller maintains the 0.6 s window (`escStreakCloses`).
    pub esc_streak: u32,
}

impl KeyInput {
    pub fn new(key_code: u16) -> Self {
        KeyInput { key_code, chars: None, cmd: false, ctrl: false, opt: false, shift: false, esc_streak: 0 }
    }

    pub fn key_char(&self) -> Option<char> {
        self.chars
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditOp {
    SelectAll,
    Copy,
    Paste,
    Cut,
    Undo,
    Find,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VimOp {
    Copy,
    Paste,
    Cut,
    SelectAll,
    Undo,
    Save,
    Search,
    Close,
    OpenPath,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileBrowserAction {
    MoveDown,
    MoveUp,
    Rename,
    CopyPath,
    SelectAll,
    Copy,
    Paste,
    Cut,
    Undo,
    FocusFilterBar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageMove {
    Home,
    End,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EscapeOutcome {
    Close,
    Handled,
}

/// The result of routing one key event, mirroring what `PopupWindow.handleKey`
/// decides. `Pass` = not consumed (the event propagates, so nothing is
/// swallowed and no action is taken).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyAction {
    FontStep(i32),
    ResizeStep(i32),
    ResizeReset,
    ShowShortcuts,
    SheetEdit(EditOp),
    CycleView(i32),
    CycleTabs(i32),
    ToggleSidebarRail,
    PaneResize(Direction),
    Vim(VimOp),
    Terminal(EditOp),
    FileBrowser(FileBrowserAction),
    Edit(EditOp),
    CommandK,
    CommandF,
    FindNext(i32),
    Escape(EscapeOutcome),
    EditorCommit,
    EditorOpenPath,
    ListMove(i32),
    ListMoveWrap(i32),
    ListAccept,
    ListToggleSelection,
    ListPage(PageMove),
    Pass,
}

// ---------------------------------------------------------------------------
// Ctrl+B prefix (`SharedWindow.prefixKey`)
// ---------------------------------------------------------------------------

/// `kVK_ANSI_B` — the prefix key.
pub const PREFIX_KEY_B: u16 = 11;

/// The prefix stays armed for 1.5 s (`prefixArmedAt` / `Date()` window).
pub const PREFIX_WINDOW_SECS: f64 = 1.5;

/// One key event for the prefix state machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrefixKey {
    pub key_code: u16,
    pub chars: Option<char>,
    pub cmd: bool,
    pub ctrl: bool,
    pub opt: bool,
    pub shift: bool,
    pub is_repeat: bool,
}

impl PrefixKey {
    pub fn new(key_code: u16) -> Self {
        PrefixKey { key_code, chars: None, cmd: false, ctrl: false, opt: false, shift: false, is_repeat: false }
    }
}

/// What `SharedWindow.prefixKey` decides for one key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrefixOutcome {
    /// Swallow the key (Swift `return true`).
    Consumed,
    /// Let the key continue down the normal router (Swift `return false`;
    /// the tabs bar's nav keys have no Rust counterpart yet).
    Passthrough,
    /// The armed prefix resolved: `l` previous view, `w` view switcher,
    /// `t` prose/terminal, `b` sidebar rail.
    Command(char),
    /// Ctrl+H/J/K/L while unarmed: move pane focus (consumed).
    PaneMove(Direction),
}

/// `SharedWindow.prefixKey`'s state: `prefixArmedAt` + `prefixHeldKey`.
#[derive(Clone, Copy, Debug, Default)]
pub struct PrefixState {
    armed_at: Option<f64>,
    held_key: Option<u16>,
}

impl PrefixState {
    pub fn new() -> Self {
        PrefixState::default()
    }

    pub fn armed(&self, now: f64) -> bool {
        self.armed_at.map(|t| now - t < PREFIX_WINDOW_SECS).unwrap_or(false)
    }

    pub fn reset(&mut self) {
        self.armed_at = None;
        self.held_key = None;
    }

    /// The `mods == .control && keyCode == 11` check (`mods` excludes shift).
    fn is_ctrl_b(key: &PrefixKey) -> bool {
        key.ctrl && !key.cmd && !key.opt && key.key_code == PREFIX_KEY_B
    }

    fn pane_dir(key: &PrefixKey) -> Option<Direction> {
        if !(key.ctrl && !key.cmd && !key.opt) {
            return None;
        }
        match key.key_code {
            KEY_H => Some(Direction::Left),
            KEY_L => Some(Direction::Right),
            KEY_K => Some(Direction::Up),
            KEY_J => Some(Direction::Down),
            _ => None,
        }
    }

    /// Route one key through the prefix logic. Mirrors the Swift order:
    /// held-key repeats, Ctrl+B arm / pass-through, Ctrl+H/J/K/L pane moves,
    /// then the armed `l`/`w`/`t`/`b` commands.
    pub fn handle(&mut self, key: &PrefixKey, now: f64) -> PrefixOutcome {
        if let Some(k) = self.held_key {
            if key.is_repeat && key.key_code == k {
                return PrefixOutcome::Consumed;
            }
            self.held_key = None;
        }
        let armed = self.armed(now);
        if Self::is_ctrl_b(key) {
            if armed {
                self.reset();
                return PrefixOutcome::Passthrough;
            }
            self.armed_at = Some(now);
            self.held_key = Some(key.key_code);
            return PrefixOutcome::Consumed;
        }
        if let Some(dir) = Self::pane_dir(key) {
            if armed {
                self.reset();
                return PrefixOutcome::Passthrough;
            }
            return PrefixOutcome::PaneMove(dir);
        }
        if !armed {
            return PrefixOutcome::Passthrough;
        }
        self.reset();
        self.held_key = Some(key.key_code);
        match key.chars {
            Some(c) if matches!(c.to_ascii_lowercase(), 'l' | 'w' | 't' | 'b') => {
                PrefixOutcome::Command(c.to_ascii_lowercase())
            }
            _ => PrefixOutcome::Consumed,
        }
    }
}

/// `escStreakCloses`: a configured count of 0 means Esc never hides.
pub fn esc_streak_closes(esc_streak: u32, esc_close_count: i32) -> bool {
    esc_close_count > 0 && esc_streak as i32 >= esc_close_count
}

/// `windowSizeKey` resets the streak on any non-Esc key before anything else.
pub fn key_resets_esc_streak(key_code: u16) -> bool {
    key_code != KEY_ESC
}

pub fn route_key(config: &PopupConfig, key: KeyInput) -> KeyAction {
    if let Some(a) = overlay_key(config, key) {
        return a;
    }
    if let Some(a) = window_size_key(config, key) {
        return a;
    }
    if key.cmd || key.ctrl {
        if let Some(a) = modified_key(config, key) {
            return a;
        }
    }
    if config.edit_mode {
        editor_key(config, key)
    } else {
        list_key(config, key)
    }
}

fn overlay_key(config: &PopupConfig, key: KeyInput) -> Option<KeyAction> {
    if key.cmd && key.key_code == KEY_SLASH && !config.sheet_active {
        return Some(KeyAction::ShowShortcuts);
    }
    None
}

fn window_size_key(_config: &PopupConfig, key: KeyInput) -> Option<KeyAction> {
    let plus = key.key_code == KEY_PLUS || key.key_code == KEY_KEYPAD_PLUS;
    let minus = key.key_code == KEY_MINUS || key.key_code == KEY_KEYPAD_MINUS;
    if key.cmd && (plus || minus) {
        let sign = if plus { 1 } else { -1 };
        // Documented handleKey order: Cmd+Opt+± = font, Cmd+± = resize.
        if key.opt {
            return Some(KeyAction::FontStep(sign));
        }
        return Some(KeyAction::ResizeStep(sign));
    }
    if key.cmd && key.key_code == KEY_ZERO && !key.opt {
        return Some(KeyAction::ResizeReset);
    }
    None
}

fn modified_key(config: &PopupConfig, key: KeyInput) -> Option<KeyAction> {
    if config.sheet_active {
        return Some(sheet_edit_key(config, key));
    }
    if let Some(a) = view_cycle_key(config, key) {
        return Some(a);
    }
    if key.key_code == KEY_BACKSLASH && key.cmd && !key.ctrl && !key.shift {
        return Some(KeyAction::ToggleSidebarRail);
    }
    if let Some(a) = pane_resize_key(config, key) {
        return Some(a);
    }
    if config.vim_focus {
        return Some(vim_pane_key(key).unwrap_or(KeyAction::Pass));
    }
    if config.terminal_focus {
        return Some(terminal_key(key).unwrap_or(KeyAction::Pass));
    }
    if let Some(a) = host_shortcut_key(config, key) {
        return Some(a);
    }
    if let Some(a) = file_browser_key(config, key) {
        return Some(a);
    }
    edit_key(config, key)
}

fn sheet_edit_key(config: &PopupConfig, key: KeyInput) -> KeyAction {
    if !config.active_text_editor {
        return KeyAction::Pass;
    }
    match key.key_code {
        9 if key.cmd || key.ctrl => KeyAction::SheetEdit(EditOp::Paste),
        0 if key.cmd => KeyAction::SheetEdit(EditOp::SelectAll),
        8 if key.cmd => KeyAction::SheetEdit(EditOp::Copy),
        7 if key.cmd => KeyAction::SheetEdit(EditOp::Cut),
        6 if key.cmd => KeyAction::SheetEdit(EditOp::Undo),
        _ => KeyAction::Pass,
    }
}

fn view_cycle_key(config: &PopupConfig, key: KeyInput) -> Option<KeyAction> {
    if key.ctrl && !key.cmd && key.key_code == KEY_TAB {
        let dir = if key.shift { -1 } else { 1 };
        if config.has_cycle_view_hook {
            return Some(KeyAction::CycleView(dir));
        }
        if config.tabs && config.tab_count > 1 {
            return Some(KeyAction::CycleTabs(dir));
        }
    }
    None
}

fn pane_resize_key(_config: &PopupConfig, key: KeyInput) -> Option<KeyAction> {
    if key.ctrl && key.shift && !key.cmd && !key.opt {
        let d = match key.key_code {
            KEY_H => Direction::Left,
            KEY_L => Direction::Right,
            KEY_K => Direction::Up,
            KEY_J => Direction::Down,
            _ => return None,
        };
        return Some(KeyAction::PaneResize(d));
    }
    None
}

fn vim_pane_key(key: KeyInput) -> Option<KeyAction> {
    let cmd = key.cmd;
    let ctrl = key.ctrl;
    let op = match key.key_code {
        8 if cmd || ctrl => VimOp::Copy,
        9 if cmd || ctrl => VimOp::Paste,
        7 if cmd => VimOp::Cut,
        0 if cmd => VimOp::SelectAll,
        6 if cmd => VimOp::Undo,
        1 if cmd => VimOp::Save,
        3 if cmd => VimOp::Search,
        13 if cmd => VimOp::Close,
        31 if cmd => VimOp::OpenPath,
        _ => return None,
    };
    Some(KeyAction::Vim(op))
}

fn terminal_key(key: KeyInput) -> Option<KeyAction> {
    match key.key_code {
        8 if key.cmd => Some(KeyAction::Terminal(EditOp::Copy)),
        9 if key.cmd || key.ctrl => Some(KeyAction::Terminal(EditOp::Paste)),
        _ => None,
    }
}

fn host_shortcut_key(config: &PopupConfig, key: KeyInput) -> Option<KeyAction> {
    if key.cmd && key.key_code == KEY_K && !(config.has_file_browser && config.browser_has_focus) {
        return Some(KeyAction::CommandK);
    }
    if key.cmd && key.key_code == KEY_F && !config.edit_mode {
        return Some(KeyAction::CommandF);
    }
    if key.cmd && key.key_code == KEY_L && config.has_file_browser {
        return Some(KeyAction::FileBrowser(FileBrowserAction::FocusFilterBar));
    }
    None
}

fn file_browser_key(config: &PopupConfig, key: KeyInput) -> Option<KeyAction> {
    if !(config.has_file_browser && config.browser_has_focus) {
        return None;
    }
    if config.browser_rename_active {
        let op = match key.key_code {
            0 => FileBrowserAction::SelectAll,
            8 => FileBrowserAction::Copy,
            9 => FileBrowserAction::Paste,
            7 => FileBrowserAction::Cut,
            6 => FileBrowserAction::Undo,
            _ => return Some(KeyAction::Pass),
        };
        return Some(KeyAction::FileBrowser(op));
    }
    let op = match key.key_code {
        15 if key.cmd => FileBrowserAction::Rename,
        45 if key.ctrl => FileBrowserAction::MoveDown,
        35 if key.ctrl => FileBrowserAction::MoveUp,
        40 if key.cmd => FileBrowserAction::CopyPath,
        0 => FileBrowserAction::SelectAll,
        8 => FileBrowserAction::Copy,
        9 => FileBrowserAction::Paste,
        7 => FileBrowserAction::Cut,
        6 => FileBrowserAction::Undo,
        _ => return None,
    };
    Some(KeyAction::FileBrowser(op))
}

fn edit_key(config: &PopupConfig, key: KeyInput) -> Option<KeyAction> {
    match key.key_code {
        // Cmd+A/X/C/V pass through to the Edit menu; only Ctrl+C/V and Cmd+Z
        // are routed here (see AGENT_CONTEXT "Edit keys").
        0 | 8 | 9 | 7 => {
            if key.cmd && !key.ctrl {
                Some(KeyAction::Pass)
            } else {
                let op = match key.key_code {
                    0 => EditOp::SelectAll,
                    8 => EditOp::Copy,
                    9 => EditOp::Paste,
                    _ => EditOp::Cut,
                };
                Some(KeyAction::Edit(op))
            }
        }
        6 => Some(KeyAction::Edit(EditOp::Undo)),
        3 if config.edit_mode => Some(KeyAction::Edit(EditOp::Find)),
        _ => None,
    }
}

fn editor_key(config: &PopupConfig, key: KeyInput) -> KeyAction {
    if config.terminal_focus {
        if key.key_code == KEY_ESC {
            return KeyAction::Escape(esc_outcome(config, key.esc_streak));
        }
        return KeyAction::Pass;
    }
    if config.vim_focus {
        if key.key_code == KEY_ESC && !(key.cmd || key.ctrl || key.opt) {
            if esc_streak_closes(key.esc_streak, config.esc_close_count) && config.vim_normal_mode {
                return KeyAction::Escape(EscapeOutcome::Close);
            }
            return KeyAction::Escape(EscapeOutcome::Handled);
        }
        return KeyAction::Pass;
    }
    if config.find_bar_shown {
        if key.key_code == KEY_ESC {
            return KeyAction::Escape(EscapeOutcome::Handled);
        }
        if key.key_code == KEY_RETURN {
            return KeyAction::FindNext(if key.shift { -1 } else { 1 });
        }
        return KeyAction::Pass;
    }
    if key.key_code == KEY_ESC {
        return KeyAction::Escape(esc_outcome(config, key.esc_streak));
    }
    if key.key_code == KEY_S && key.cmd {
        return KeyAction::EditorCommit;
    }
    if key.key_code == KEY_O && key.cmd {
        return KeyAction::EditorOpenPath;
    }
    KeyAction::Pass
}

fn esc_outcome(config: &PopupConfig, esc_streak: u32) -> EscapeOutcome {
    if esc_streak_closes(esc_streak, config.esc_close_count) {
        EscapeOutcome::Close
    } else {
        EscapeOutcome::Handled
    }
}

fn list_key(config: &PopupConfig, key: KeyInput) -> KeyAction {
    if config.selectable_rows && key.ctrl && key.key_code == KEY_SPACE {
        return KeyAction::ListToggleSelection;
    }
    if config.enable_navigation {
        match (key.key_code, key.ctrl) {
            (KEY_DOWN, _) => return KeyAction::ListMove(1),
            (KEY_UP, _) => return KeyAction::ListMove(-1),
            (KEY_TAB, _) => return KeyAction::ListMoveWrap(if key.shift { -1 } else { 1 }),
            (KEY_N, true) => return KeyAction::ListMove(1),
            (KEY_P, true) => return KeyAction::ListMove(-1),
            (KEY_RETURN, _) => return KeyAction::ListAccept,
            (KEY_J, true) => return KeyAction::ListAccept,
            (KEY_HOME, false) => return KeyAction::ListPage(PageMove::Home),
            (KEY_END, false) => return KeyAction::ListPage(PageMove::End),
            (KEY_PAGE_UP, false) => return KeyAction::ListPage(PageMove::Up),
            (KEY_PAGE_DOWN, false) => return KeyAction::ListPage(PageMove::Down),
            _ => {}
        }
    }
    if config.enable_escape && key.key_code == KEY_ESC {
        return KeyAction::Escape(esc_outcome(config, key.esc_streak));
    }
    KeyAction::Pass
}

/// Token returned by [`install_local_monitor`]; dropping it removes the
/// monitor (`NSEvent.removeMonitor`), and [`MonitorHandle::remove`] does so
/// eagerly at teardown time.
pub struct MonitorHandle {
    #[cfg(target_os = "macos")]
    monitor: Option<objc2::rc::Retained<objc2::runtime::AnyObject>>,
}

impl MonitorHandle {
    /// `NSEvent.removeMonitor`; idempotent.
    pub fn remove(&mut self) {
        #[cfg(target_os = "macos")]
        if let Some(m) = self.monitor.take() {
            unsafe { objc2_app_kit::NSEvent::removeMonitor(&m) };
        }
    }

    /// Whether the monitor is still installed.
    #[cfg(target_os = "macos")]
    pub fn is_installed(&self) -> bool {
        self.monitor.is_some()
    }
}

impl Drop for MonitorHandle {
    fn drop(&mut self) {
        self.remove();
    }
}

/// Install the window's keyDown guard, mirroring
/// `PopupWindow.installMonitors`' `addLocalMonitorForEvents(matching: .keyDown)`
/// handler: `handler` sees every keyDown delivered to this app and returns
/// `true` to consume it (Swift's `return nil`) or `false` to pass the event
/// through (Swift's `return event`).
///
/// This is the monitor/seam only: the handler runs the pure [`route_key`]
/// decision (plus the caller's transientEsc / edit-key forwarding) and executes
/// the resulting action. Call on the main thread.
#[cfg(target_os = "macos")]
pub fn install_local_monitor<F>(_mtm: MainThreadMarker, handler: F) -> MonitorHandle
where
    F: Fn(&objc2_app_kit::NSEvent) -> bool + 'static,
{
    use block2::RcBlock;
    use objc2_app_kit::{NSEvent, NSEventMask};
    use std::ptr::NonNull;

    let block = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
        if handler(unsafe { event.as_ref() }) {
            std::ptr::null_mut()
        } else {
            event.as_ptr()
        }
    });
    let monitor = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, &block)
    };
    MonitorHandle { monitor }
}

/// The [`KeyInput`] an `NSEvent` keyDown carries, mirroring the fields
/// `PopupWindow.handleKey` reads. `esc_streak` stays 0: the caller owns the
/// 0.6 s streak window (see [`esc_streak_closes`]).
#[cfg(target_os = "macos")]
pub fn key_input_from_event(event: &objc2_app_kit::NSEvent) -> KeyInput {
    use objc2_app_kit::NSEventModifierFlags as M;
    let mods = event.modifierFlags() & M::DeviceIndependentFlagsMask;
    KeyInput {
        key_code: event.keyCode(),
        chars: event
            .charactersIgnoringModifiers()
            .and_then(|s| s.to_string().chars().next()),
        cmd: mods.contains(M::Command),
        ctrl: mods.contains(M::Control),
        opt: mods.contains(M::Option),
        shift: mods.contains(M::Shift),
        esc_streak: 0,
    }
}

#[cfg(target_os = "macos")]
#[allow(unused_imports)]
pub use appkit::{PopupBaseWindow, PopupPanel};

#[cfg(target_os = "macos")]
mod appkit {
    use super::PopupConfig;
    use objc2::rc::Retained;
    use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSBackingStoreType, NSColor, NSFloatingWindowLevel, NSNormalWindowLevel, NSPanel,
        NSWindow, NSWindowAnimationBehavior, NSWindowButton, NSWindowCollectionBehavior,
        NSWindowStyleMask, NSWindowTitleVisibility,
    };
    use objc2_foundation::{
        NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
    };

    pub struct PopupBaseWindowIvars {
        corner_radius: f64,
    }

    define_class!(
        #[unsafe(super(NSWindow))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSPopupBaseWindow"]
        #[ivars = PopupBaseWindowIvars]
        pub struct PopupBaseWindow;

        impl PopupBaseWindow {
            #[unsafe(method(_cornerRadius))]
            fn _corner_radius(&self) -> f64 {
                self.ivars().corner_radius
            }
        }

        unsafe impl NSObjectProtocol for PopupBaseWindow {}
    );

    pub struct PopupPanelIvars {
        corner_radius: f64,
    }

    define_class!(
        #[unsafe(super(NSPanel))]
        #[thread_kind = MainThreadOnly]
        #[name = "WSPopupPanel"]
        #[ivars = PopupPanelIvars]
        pub struct PopupPanel;

        impl PopupPanel {
            #[unsafe(method(_cornerRadius))]
            fn _corner_radius(&self) -> f64 {
                self.ivars().corner_radius
            }
        }

        unsafe impl NSObjectProtocol for PopupPanel {}
    );

    impl PopupPanel {
        pub fn create(mtm: MainThreadMarker, config: &PopupConfig) -> Retained<PopupPanel> {
            let frame = NSRect::new(
                NSPoint::new(config.frame.x, config.frame.y),
                NSSize::new(config.frame.width, config.frame.height),
            );
            let mut mask = NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::FullSizeContentView;
            if config.enable_resize {
                mask |= NSWindowStyleMask::Resizable;
            }
            if config.tool_panel {
                mask |= NSWindowStyleMask::NonactivatingPanel;
            }

            let this = PopupPanel::alloc(mtm).set_ivars(PopupPanelIvars {
                corner_radius: config.corner_radius,
            });
            let panel: Retained<PopupPanel> = unsafe {
                msg_send![
                    super(this),
                    initWithContentRect: frame,
                    styleMask: mask,
                    backing: NSBackingStoreType::Buffered,
                    defer: false
                ]
            };

            panel.setTitle(&NSString::from_str(&config.name));
            panel.setTitlebarAppearsTransparent(true);
            panel.setTitleVisibility(NSWindowTitleVisibility::Hidden);
            panel.setOpaque(false);
            panel.setBackgroundColor(Some(&NSColor::clearColor()));
            panel.setHasShadow(config.has_shadow);
            panel.setStyleMask(mask);
            panel.setMovableByWindowBackground(true);
            panel.setAcceptsMouseMovedEvents(true);
            panel.setContentMinSize(NSSize::new(320.0, 220.0));
            let level = if config.floating { NSFloatingWindowLevel } else { NSNormalWindowLevel };
            panel.setLevel(level);
            panel.setCollectionBehavior(
                NSWindowCollectionBehavior::CanJoinAllSpaces
                    | NSWindowCollectionBehavior::FullScreenAuxiliary,
            );
            panel.setAnimationBehavior(NSWindowAnimationBehavior::None);
            unsafe { panel.setReleasedWhenClosed(false) };
            if config.tool_panel {
                panel.setHidesOnDeactivate(false);
            }
            for b in [
                NSWindowButton::CloseButton,
                NSWindowButton::MiniaturizeButton,
                NSWindowButton::ZoomButton,
            ] {
                if let Some(btn) = panel.standardWindowButton(b) {
                    btn.setHidden(true);
                }
            }
            panel
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: u16) -> KeyInput {
        KeyInput::new(code)
    }

    fn list_config() -> PopupConfig {
        PopupConfig { edit_mode: false, ..Default::default() }
    }

    #[test]
    fn esc_streak_single_closes() {
        let c = PopupConfig { esc_close_count: 1, ..Default::default() };
        let mut k = key(KEY_ESC);
        k.esc_streak = 1;
        assert_eq!(route_key(&c, k), KeyAction::Escape(EscapeOutcome::Close));
    }

    #[test]
    fn esc_streak_double_needs_two() {
        let c = PopupConfig { esc_close_count: 2, ..Default::default() };
        let mut k = key(KEY_ESC);
        k.esc_streak = 1;
        assert_eq!(route_key(&c, k), KeyAction::Escape(EscapeOutcome::Handled));
        k.esc_streak = 2;
        assert_eq!(route_key(&c, k), KeyAction::Escape(EscapeOutcome::Close));
    }

    #[test]
    fn esc_never_closes_when_disabled() {
        let c = PopupConfig { esc_close_count: 0, ..Default::default() };
        let mut k = key(KEY_ESC);
        k.esc_streak = 9;
        assert_eq!(route_key(&c, k), KeyAction::Escape(EscapeOutcome::Handled));

        let c = PopupConfig { enable_escape: false, ..Default::default() };
        k.esc_streak = 1;
        assert_eq!(route_key(&c, k), KeyAction::Pass);
    }

    #[test]
    fn non_esc_resets_streak() {
        assert!(!key_resets_esc_streak(KEY_ESC));
        assert!(key_resets_esc_streak(KEY_A));
        assert!(key_resets_esc_streak(KEY_DOWN));
    }

    #[test]
    fn cmd_c_passthrough_and_ctrl_c_routed() {
        let c = list_config();
        let mut cmd_c = key(KEY_C);
        cmd_c.cmd = true;
        assert_eq!(route_key(&c, cmd_c), KeyAction::Pass);

        let mut ctrl_c = key(KEY_C);
        ctrl_c.ctrl = true;
        assert_eq!(route_key(&c, ctrl_c), KeyAction::Edit(EditOp::Copy));

        let mut cmd_z = key(KEY_Z);
        cmd_z.cmd = true;
        assert_eq!(route_key(&c, cmd_z), KeyAction::Edit(EditOp::Undo));
    }

    #[test]
    fn ctrl_tab_cycles_tabs() {
        let c = PopupConfig { tabs: true, tab_count: 3, ..Default::default() };
        let mut k = key(KEY_TAB);
        k.ctrl = true;
        assert_eq!(route_key(&c, k), KeyAction::CycleTabs(1));
        k.shift = true;
        assert_eq!(route_key(&c, k), KeyAction::CycleTabs(-1));

        let c = PopupConfig { has_cycle_view_hook: true, ..Default::default() };
        k.shift = false;
        assert_eq!(route_key(&c, k), KeyAction::CycleView(1));
    }

    #[test]
    fn list_navigation() {
        let c = list_config();
        assert_eq!(route_key(&c, key(KEY_DOWN)), KeyAction::ListMove(1));
        assert_eq!(route_key(&c, key(KEY_UP)), KeyAction::ListMove(-1));
        assert_eq!(route_key(&c, key(KEY_RETURN)), KeyAction::ListAccept);
        assert_eq!(route_key(&c, key(KEY_TAB)), KeyAction::ListMoveWrap(1));
        let mut shift_tab = key(KEY_TAB);
        shift_tab.shift = true;
        assert_eq!(route_key(&c, shift_tab), KeyAction::ListMoveWrap(-1));

        let mut ctrl_n = key(KEY_N);
        ctrl_n.ctrl = true;
        assert_eq!(route_key(&c, ctrl_n), KeyAction::ListMove(1));
        let mut ctrl_p = key(KEY_P);
        ctrl_p.ctrl = true;
        assert_eq!(route_key(&c, ctrl_p), KeyAction::ListMove(-1));

        assert_eq!(route_key(&c, key(KEY_HOME)), KeyAction::ListPage(PageMove::Home));
        assert_eq!(route_key(&c, key(KEY_PAGE_DOWN)), KeyAction::ListPage(PageMove::Down));
    }

    #[test]
    fn file_browser_keys() {
        let c = PopupConfig { has_file_browser: true, browser_has_focus: true, ..Default::default() };
        let mut ctrl_n = key(KEY_N);
        ctrl_n.ctrl = true;
        assert_eq!(route_key(&c, ctrl_n), KeyAction::FileBrowser(FileBrowserAction::MoveDown));
        let mut ctrl_p = key(KEY_P);
        ctrl_p.ctrl = true;
        assert_eq!(route_key(&c, ctrl_p), KeyAction::FileBrowser(FileBrowserAction::MoveUp));

        let mut cmd_k = key(KEY_K);
        cmd_k.cmd = true;
        assert_eq!(route_key(&c, cmd_k), KeyAction::FileBrowser(FileBrowserAction::CopyPath));
        let mut cmd_c = key(KEY_C);
        cmd_c.cmd = true;
        assert_eq!(route_key(&c, cmd_c), KeyAction::FileBrowser(FileBrowserAction::Copy));

        let mut cmd_k = key(KEY_K);
        cmd_k.cmd = true;
        let no_browser = list_config();
        assert_eq!(route_key(&no_browser, cmd_k), KeyAction::CommandK);
    }

    #[test]
    fn sheet_edit_keys() {
        let c = PopupConfig { sheet_active: true, active_text_editor: true, ..Default::default() };
        let mut cmd_c = key(KEY_C);
        cmd_c.cmd = true;
        assert_eq!(route_key(&c, cmd_c), KeyAction::SheetEdit(EditOp::Copy));
        let mut cmd_v = key(KEY_V);
        cmd_v.cmd = true;
        assert_eq!(route_key(&c, cmd_v), KeyAction::SheetEdit(EditOp::Paste));
        let mut cmd_a = key(KEY_A);
        cmd_a.cmd = true;
        assert_eq!(route_key(&c, cmd_a), KeyAction::SheetEdit(EditOp::SelectAll));

        let c = PopupConfig { sheet_active: true, active_text_editor: false, ..Default::default() };
        assert_eq!(route_key(&c, cmd_c), KeyAction::Pass);
    }

    #[test]
    fn edit_mode_find_bar_and_editor_keys() {
        let c = PopupConfig { edit_mode: true, find_bar_shown: true, ..Default::default() };
        assert_eq!(route_key(&c, key(KEY_RETURN)), KeyAction::FindNext(1));
        let mut shift_return = key(KEY_RETURN);
        shift_return.shift = true;
        assert_eq!(route_key(&c, shift_return), KeyAction::FindNext(-1));
        assert_eq!(route_key(&c, key(KEY_ESC)), KeyAction::Escape(EscapeOutcome::Handled));

        let c = PopupConfig { edit_mode: true, esc_close_count: 1, ..Default::default() };
        let mut cmd_s = key(KEY_S);
        cmd_s.cmd = true;
        assert_eq!(route_key(&c, cmd_s), KeyAction::EditorCommit);
        let mut cmd_o = key(KEY_O);
        cmd_o.cmd = true;
        assert_eq!(route_key(&c, cmd_o), KeyAction::EditorOpenPath);
        let mut esc = key(KEY_ESC);
        esc.esc_streak = 1;
        assert_eq!(route_key(&c, esc), KeyAction::Escape(EscapeOutcome::Close));
    }

    #[test]
    fn window_size_and_shortcuts() {
        let c = list_config();
        let mut cmd_plus = key(KEY_PLUS);
        cmd_plus.cmd = true;
        assert_eq!(route_key(&c, cmd_plus), KeyAction::ResizeStep(1));
        cmd_plus.opt = true;
        assert_eq!(route_key(&c, cmd_plus), KeyAction::FontStep(1));
        let mut cmd_zero = key(KEY_ZERO);
        cmd_zero.cmd = true;
        assert_eq!(route_key(&c, cmd_zero), KeyAction::ResizeReset);
        let mut cmd_slash = key(KEY_SLASH);
        cmd_slash.cmd = true;
        assert_eq!(route_key(&c, cmd_slash), KeyAction::ShowShortcuts);
    }

    #[test]
    fn pane_resize_and_sidebar() {
        let c = list_config();
        let mut resize = key(KEY_H);
        resize.ctrl = true;
        resize.shift = true;
        assert_eq!(route_key(&c, resize), KeyAction::PaneResize(Direction::Left));
        let mut rail = key(KEY_BACKSLASH);
        rail.cmd = true;
        assert_eq!(route_key(&c, rail), KeyAction::ToggleSidebarRail);
    }

    fn prefix_key(code: u16, ctrl: bool, ch: Option<char>) -> PrefixKey {
        PrefixKey { key_code: code, chars: ch, ctrl, ..PrefixKey::new(code) }
    }

    #[test]
    fn prefix_arms_then_runs_command() {
        let mut s = PrefixState::new();
        let b = prefix_key(PREFIX_KEY_B, true, None);
        assert_eq!(s.handle(&b, 100.0), PrefixOutcome::Consumed);
        assert!(s.armed(100.1));
        let l = prefix_key(KEY_L, false, Some('l'));
        assert_eq!(s.handle(&l, 100.2), PrefixOutcome::Command('l'));
        // The command fires once; the next key passes through.
        let w = prefix_key(KEY_W, false, Some('w'));
        assert_eq!(s.handle(&w, 100.3), PrefixOutcome::Passthrough);
    }

    #[test]
    fn prefix_expires_after_window() {
        let mut s = PrefixState::new();
        let b = prefix_key(PREFIX_KEY_B, true, None);
        s.handle(&b, 0.0);
        let l = prefix_key(KEY_L, false, Some('l'));
        assert_eq!(s.handle(&l, 1.6), PrefixOutcome::Passthrough);
        assert!(!s.armed(1.6));
    }

    #[test]
    fn prefix_double_press_passes_the_real_key() {
        let mut s = PrefixState::new();
        let b = prefix_key(PREFIX_KEY_B, true, None);
        assert_eq!(s.handle(&b, 1.0), PrefixOutcome::Consumed);
        assert_eq!(s.handle(&b, 1.2), PrefixOutcome::Passthrough, "armed Ctrl+B falls through");
        assert!(!s.armed(1.2));
    }

    #[test]
    fn prefix_repeat_of_held_key_is_consumed() {
        let mut s = PrefixState::new();
        let mut b = prefix_key(PREFIX_KEY_B, true, None);
        s.handle(&b, 1.0);
        b.is_repeat = true;
        assert_eq!(s.handle(&b, 1.0), PrefixOutcome::Consumed);
        b.is_repeat = false;
        assert_eq!(s.handle(&b, 1.05), PrefixOutcome::Passthrough, "non-repeat clears + double-pass");
    }

    #[test]
    fn prefix_pane_moves_unarmed_and_passthrough_when_armed() {
        let mut s = PrefixState::new();
        let h = prefix_key(KEY_H, true, None);
        assert_eq!(s.handle(&h, 5.0), PrefixOutcome::PaneMove(Direction::Left));
        let j = prefix_key(KEY_J, true, None);
        assert_eq!(s.handle(&j, 5.1), PrefixOutcome::PaneMove(Direction::Down));

        let b = prefix_key(PREFIX_KEY_B, true, None);
        s.handle(&b, 6.0);
        assert_eq!(s.handle(&h, 6.1), PrefixOutcome::Passthrough, "armed pane key passes");
        assert!(!s.armed(6.1));
    }

    #[test]
    fn prefix_unknown_command_consumes() {
        let mut s = PrefixState::new();
        let b = prefix_key(PREFIX_KEY_B, true, None);
        s.handle(&b, 2.0);
        let x = prefix_key(KEY_X, false, Some('x'));
        assert_eq!(s.handle(&x, 2.1), PrefixOutcome::Consumed);
    }
}
