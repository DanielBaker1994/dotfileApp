import AppKit

// Ctrl+W: delete the word before the cursor — readline's Ctrl+W
// (unix-word-rubout, the shell / nvim-Insert behaviour) — everywhere the app
// has an editable text field.
//
// The app routes its edit shortcuts by hand (each popup's key monitor,
// `PopupWindow.editKey`, `JiraEditKeys.route`) because a nonactivating
// window does not get reliable Edit-menu key equivalents. Rather than repeat
// Ctrl+W in every one of those routers, ONE local monitor installed at
// launch covers every text surface at once:
//
//   • the notes find / grep popup      (Space s f / Space s g)
//   • the Ctrl+B W view switcher
//   • /paths, the command palette, the file-browser filter / address bar
//   • the in-note find bar and the sheets (New Note, Open Existing, path)
//   • the Jira / Confluence / Compare / AI card fields and the inline renames
//
// The vim pane and the terminal drawer are deliberately EXCLUDED: their
// first responder is the terminal view (not an NSTextView), so Ctrl+W still
// reaches vim (`<C-w>` window commands, word-delete in Insert mode) and the
// shell's readline.
enum TextEditKeys {
    private static let wKeyCode: UInt16 = 13          // ANSI W

    // Returns true when the event was consumed.
    static func route(_ e: NSEvent) -> Bool {
        guard e.type == .keyDown, e.keyCode == wKeyCode else { return false }
        let mods = e.modifierFlags.intersection(.deviceIndependentFlagsMask)
        // Ctrl+W only: Cmd+W closes the window, the Shift / Option variants
        // are not ours
        guard mods.contains(.control), !mods.contains(.command),
              !mods.contains(.option), !mods.contains(.shift) else { return false }
        guard let ed = (e.window ?? NSApp.keyWindow)?.firstResponder as? NSTextView,
              ed.isEditable else { return false }
        killWordBackward(ed)
        return true
    }

    // readline's Ctrl+W: delete from the caret back to the start of the
    // whitespace-delimited word (including the whitespace between it and the
    // caret); with a selection, delete the selection. Routed through
    // insertText(_:replacementRange:) so undo / change tracking stay correct.
    private static func killWordBackward(_ ed: NSTextView) {
        let sel = ed.selectedRange()
        if sel.length > 0 {
            ed.insertText("", replacementRange: sel)
            return
        }
        let text = ed.string as NSString
        let caret = min(sel.location, text.length)
        var start = caret
        while start > 0, isWhitespace(text.character(at: start - 1)) { start -= 1 }
        while start > 0, !isWhitespace(text.character(at: start - 1)) { start -= 1 }
        guard start < caret else { return }
        ed.insertText("", replacementRange: NSRange(location: start, length: caret - start))
    }

    private static func isWhitespace(_ c: unichar) -> Bool {
        guard let scalar = Unicode.Scalar(c) else { return false }
        return CharacterSet.whitespacesAndNewlines.contains(scalar)
    }
}
