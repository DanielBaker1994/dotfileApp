//! Key helpers shared by every standalone popup host (the main daemon's
//! `DaemonUi` and the quick agent): popups are non-activating panels, so the
//! Edit menu of the (inactive) app never sees Cmd+C/V/X/A/Z and the field
//! editors need the keys sent to them directly (rule 1).

use crate::ui::view_base::as_any;
use objc2::runtime::AnyObject;
use objc2::{msg_send, sel, MainThreadMarker};
use objc2_app_kit::{NSApplication, NSEvent};

/// Cmd+A/X/C/V/Z (+ Cmd+Shift+Z) for a popup's text input while
/// another app is frontmost. Popups are non-activating panels — Hyper+S
/// never activates kitchen-sink — and AppKit only routes Cmd edit keys
/// through the Edit menu of the *active* app, so without this they did
/// nothing in the Create File paste box, Text to Table, the palette
/// search, … (rule 1). Sent straight to the first responder: any
/// `NSText` (fields, text views), or the terminal popup's view.
pub fn cmd_edit_in_popup(event: &NSEvent) -> bool {
    use objc2_app_kit::NSEventModifierFlags as F;
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    let flags = event.modifierFlags();
    if !flags.contains(F::Command) || flags.contains(F::Control) || flags.contains(F::Option) {
        return false;
    }
    let shift = flags.contains(F::Shift);
    let selector = match (event.keyCode(), shift) {
        (0, false) => sel!(selectAll:),
        (7, false) => sel!(cut:),
        (8, false) => sel!(copy:),
        (9, false) => sel!(paste:),
        (6, false) => sel!(undo:),
        (6, true) => sel!(redo:),
        _ => return false,
    };
    // We are the frontmost app: the Edit menu works, leave it be.
    let own = std::process::id() as i32;
    if crate::platform::frontmost_app().map_or(true, |a| a.pid == own) {
        return false;
    }
    let app = NSApplication::sharedApplication(mtm);
    let Some(window) = app.keyWindow() else {
        return false;
    };
    let Some(r) = window.firstResponder() else {
        return false;
    };
    let is_text = as_any(&*r).downcast_ref::<objc2_app_kit::NSText>().is_some();
    let is_terminal = window.title().to_string() == "terminal";
    // Find Recent Files: Cmd+C with no text selected in the filter
    // field copies the selected row's path (`paths_key`), without
    // needing a click to activate the app first.
    if selector == sel!(copy:) && window.title().to_string() == "paths" {
        let has_sel = as_any(&*r)
            .downcast_ref::<objc2_app_kit::NSText>()
            .is_some_and(|t| t.selectedRange().length > 0);
        if !has_sel {
            return false;
        }
    }
    let target: &AnyObject = as_any(&*r);
    let responds: bool = unsafe { msg_send![target, respondsToSelector: selector] };
    if !(is_text || is_terminal) || !responds {
        return false;
    }
    unsafe { app.sendAction_to_from(selector, Some(target), None) }
}


/// `TextEditKeys.killWordBackward` for any editable `NSText` first
/// responder (field editors included) in the key window.
pub fn kill_word_in_text_input(event: &NSEvent) -> bool {
    use crate::engines::text_edit_keys::{route, EditAction, Focus, Modifiers, KEY_W};
    use objc2_app_kit::NSEventModifierFlags as F;
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    let flags = event.modifierFlags();
    let mods = Modifiers {
        ctrl: flags.contains(F::Control),
        cmd: flags.contains(F::Command),
        option: flags.contains(F::Option),
        shift: flags.contains(F::Shift),
    };
    if !mods.ctrl || event.keyCode() != KEY_W {
        return false;
    }
    let Some(window) = objc2_app_kit::NSApplication::sharedApplication(mtm).keyWindow() else {
        return false;
    };
    let Some(r) = window.firstResponder() else {
        return false;
    };
    let Some(text) = as_any(&*r).downcast_ref::<objc2_app_kit::NSText>() else {
        return false;
    };
    let focus = Focus { is_text: true, is_editable: text.isEditable() };
    if route(event.keyCode(), mods, focus) != Some(EditAction::KillWordBackward) {
        return false;
    }
    let nil: Option<&AnyObject> = None;
    let _: () = unsafe { msg_send![text, deleteWordBackward: nil] };
    true
}

