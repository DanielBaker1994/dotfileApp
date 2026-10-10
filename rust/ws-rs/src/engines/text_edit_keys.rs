//! Port of `TextEditKeys.swift` and `JiraEditKeys` (`JiraSearch.swift`) — the
//! pure Ctrl/Cmd key-routing decision for text fields. The Swift originals are
//! AppKit local key monitors; here the decision is a pure function so it can be
//! tested without NSEvent / NSResponder.

/// macOS virtual key codes used by the two routers.
pub const KEY_A: u16 = 0;
pub const KEY_Z: u16 = 6;
pub const KEY_X: u16 = 7;
pub const KEY_C: u16 = 8;
pub const KEY_V: u16 = 9;
pub const KEY_W: u16 = 13;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditAction {
    Paste,
    Copy,
    Cut,
    SelectAll,
    Undo,
    Redo,
    /// Ctrl+W: delete the previous word (`TextEditKeys.killWordBackward`).
    KillWordBackward,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub ctrl: bool,
    pub cmd: bool,
    pub option: bool,
    pub shift: bool,
}

/// What has focus. `is_text` = first responder is an `NSText`
/// (`JiraEditKeys`); `is_editable` = it is an editable `NSTextView`
/// (`TextEditKeys`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Focus {
    pub is_text: bool,
    pub is_editable: bool,
}

/// `JiraEditKeys.route`: Cmd OR Ctrl fires copy/cut/paste; Cmd-only fires
/// select-all / undo / redo.
pub fn jira_edit_keys_route(key_code: u16, mods: Modifiers, focus: Focus) -> Option<EditAction> {
    if !focus.is_text || !(mods.cmd || mods.ctrl) {
        return None;
    }
    match key_code {
        KEY_V => Some(EditAction::Paste),
        KEY_C => Some(EditAction::Copy),
        KEY_X => Some(EditAction::Cut),
        KEY_A if mods.cmd => Some(EditAction::SelectAll),
        KEY_Z if mods.cmd => Some(if mods.shift {
            EditAction::Redo
        } else {
            EditAction::Undo
        }),
        _ => None,
    }
}

/// `TextEditKeys.route`: only Ctrl+W, with no Cmd/Option/Shift, in an
/// editable text view.
pub fn text_edit_keys_route(key_code: u16, mods: Modifiers, focus: Focus) -> Option<EditAction> {
    if key_code != KEY_W {
        return None;
    }
    if !(mods.ctrl && !mods.cmd && !mods.option && !mods.shift) {
        return None;
    }
    if !(focus.is_text && focus.is_editable) {
        return None;
    }
    Some(EditAction::KillWordBackward)
}

/// Unified router: the two Swift routers handle disjoint keys, so applying the
/// Ctrl+W rule first and then the Cmd/Ctrl clipboard rule matches either path.
pub fn route(key_code: u16, mods: Modifiers, focus: Focus) -> Option<EditAction> {
    text_edit_keys_route(key_code, mods, focus)
        .or_else(|| jira_edit_keys_route(key_code, mods, focus))
}

/// The no-selection branch of `killWordBackward`: walk back over whitespace
/// then over the word, returning the range to delete. `None` when the caret is
/// at the start of the text (nothing to delete). A caller with a live selection
/// just deletes the selection instead.
pub fn kill_word_backward_range(chars: &[char], caret: usize) -> Option<(usize, usize)> {
    let caret = caret.min(chars.len());
    let mut start = caret;
    while start > 0 && is_whitespace(chars[start - 1]) {
        start -= 1;
    }
    while start > 0 && !is_whitespace(chars[start - 1]) {
        start -= 1;
    }
    if start < caret {
        Some((start, caret))
    } else {
        None
    }
}

fn is_whitespace(c: char) -> bool {
    c.is_whitespace()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(ctrl: bool, cmd: bool, option: bool, shift: bool) -> Modifiers {
        Modifiers {
            ctrl,
            cmd,
            option,
            shift,
        }
    }

    fn editable() -> Focus {
        Focus {
            is_text: true,
            is_editable: true,
        }
    }

    fn text_only() -> Focus {
        Focus {
            is_text: true,
            is_editable: false,
        }
    }

    #[test]
    fn ctrl_w_kills_word_only_when_editable_and_bare() {
        let bare = mods(true, false, false, false);
        assert_eq!(
            route(KEY_W, bare, editable()),
            Some(EditAction::KillWordBackward)
        );
        assert_eq!(route(KEY_W, bare, text_only()), None);
        assert_eq!(route(KEY_W, mods(true, true, false, false), editable()), None);
        assert_eq!(route(KEY_W, mods(true, false, true, false), editable()), None);
        assert_eq!(route(KEY_W, mods(true, false, false, true), editable()), None);
        assert_eq!(route(KEY_W, mods(false, false, false, false), editable()), None);
    }

    #[test]
    fn clipboard_keys_fire_on_cmd_or_ctrl() {
        for m in [mods(false, true, false, false), mods(true, false, false, false)] {
            assert_eq!(route(KEY_V, m, editable()), Some(EditAction::Paste));
            assert_eq!(route(KEY_C, m, editable()), Some(EditAction::Copy));
            assert_eq!(route(KEY_X, m, editable()), Some(EditAction::Cut));
        }
        // copy works on a non-editable NSText too
        assert_eq!(
            route(KEY_C, mods(false, true, false, false), text_only()),
            Some(EditAction::Copy)
        );
    }

    #[test]
    fn select_all_and_undo_are_cmd_only() {
        assert_eq!(
            route(KEY_A, mods(false, true, false, false), editable()),
            Some(EditAction::SelectAll)
        );
        assert_eq!(route(KEY_A, mods(true, false, false, false), editable()), None);
        assert_eq!(
            route(KEY_Z, mods(false, true, false, false), editable()),
            Some(EditAction::Undo)
        );
        assert_eq!(
            route(KEY_Z, mods(false, true, false, true), editable()),
            Some(EditAction::Redo)
        );
        assert_eq!(route(KEY_Z, mods(true, false, false, false), editable()), None);
    }

    #[test]
    fn no_modifier_or_non_text_is_none() {
        assert_eq!(route(KEY_V, mods(false, false, false, false), editable()), None);
        assert_eq!(
            route(KEY_V, mods(false, true, false, false), Focus::default()),
            None
        );
        assert_eq!(route(KEY_W, mods(true, false, false, false), Focus::default()), None);
        assert_eq!(route(42, mods(false, true, false, false), editable()), None);
    }

    #[test]
    fn kill_word_backward_range_matches_swift() {
        let chars: Vec<char> = "hello big world".chars().collect();
        // caret after "world": eats the word (no trailing ws)
        assert_eq!(kill_word_backward_range(&chars, 15), Some((10, 15)));
        // caret after "world " (trailing space): eats trailing ws then the word
        let with_ws: Vec<char> = "hello big world ".chars().collect();
        assert_eq!(kill_word_backward_range(&with_ws, 16), Some((10, 16)));
        // caret in the middle of a word
        assert_eq!(kill_word_backward_range(&chars, 9), Some((6, 9)));
        // caret at start
        assert_eq!(kill_word_backward_range(&chars, 0), None);
        // whitespace run before caret is deleted
        let ws: Vec<char> = "   ".chars().collect();
        assert_eq!(kill_word_backward_range(&ws, 3), Some((0, 3)));
    }
}
