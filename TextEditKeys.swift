import AppKit

enum TextEditKeys {
    private static let wKeyCode: UInt16 = 13

    static func route(_ e: NSEvent) -> Bool {
        guard e.type == .keyDown, e.keyCode == wKeyCode else { return false }
        let mods = e.modifierFlags.intersection(.deviceIndependentFlagsMask)
        guard mods.contains(.control), !mods.contains(.command),
              !mods.contains(.option), !mods.contains(.shift) else { return false }
        guard let ed = (e.window ?? NSApp.keyWindow)?.firstResponder as? NSTextView,
              ed.isEditable else { return false }
        killWordBackward(ed)
        return true
    }

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
