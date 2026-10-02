import AppKit

// MARK: - Shared inline rename field
//
// Finder-style rename in place: a text field overlay on a row's name. Used by
// PopupFileBrowser (the Files view) and PathsWindow (/paths shelf). The caller
// handles transientEscape, first-responder setup, and the actual file move.

final class InlineRename: NSObject, NSTextFieldDelegate {
    private var field: NSTextField?
    private var path: String?
    var onCommit: (() -> Void)?

    // The text field while a rename is up (for first responder checks).
    var textField: NSTextField? { field }

    // Begin an inline rename over a row. The field is added as a subview of
    // `parent`, positioned over the name area, and focused.
    //   parent     the list view (FileListPane) to add the field to
    //   nameRect   where the row's name is drawn
    //   name       the current file name (lastPathComponent)
    //   isDir      whether this is a directory (affects stem selection)
    //   colors     theme colors for the field
    //   onCommit   called when the user commits (Return, Tab, click away)
    @discardableResult
    static func begin(parent: NSView, nameRect: NSRect, name: String, isDir: Bool,
                      colors: PopupColors, onCommit: (() -> Void)? = nil) -> InlineRename {
        let r = InlineRename()
        r.onCommit = onCommit
        r._begin(parent: parent, nameRect: nameRect, name: name, isDir: isDir, colors: colors)
        return r
    }

    private func _begin(parent: NSView, nameRect: NSRect, name: String, isDir: Bool, colors: PopupColors) {
        let f = NSTextField(string: name)
        f.font = NSFont.systemFont(ofSize: 12)
        f.isBordered = false
        f.focusRingType = .none
        f.drawsBackground = true
        f.backgroundColor = colors.mantle
        f.textColor = colors.text
        f.usesSingleLineMode = true
        f.cell?.isScrollable = true
        f.cell?.lineBreakMode = .byClipping
        f.wantsLayer = true
        f.layer?.cornerRadius = 3
        f.layer?.borderWidth = 1
        f.layer?.borderColor = colors.accentOn.cgColor
        f.delegate = self
        let nr = nameRect
        f.frame = NSRect(x: nr.minX - 3, y: nr.minY + 2, width: nr.width + 3, height: nr.height - 4)
        parent.addSubview(f)
        field = f
        path = nil // caller sets this

        let nameStr = f.stringValue as NSString
        let stem = isDir ? nameStr.length : (nameStr.deletingPathExtension as NSString).length
        f.currentEditor()?.selectedRange = NSRange(location: 0, length: stem > 0 ? stem : nameStr.length)
    }

    // Set the path that this rename refers to (called by the caller after begin).
    func setPath(_ p: String) { path = p }

    // Take the field down; returns the path + typed name, or nil when no rename is up.
    // Restores first responder to the list if the field had focus.
    func end(list: NSView, window: NSWindow?) -> (path: String, text: String)? {
        guard let f = field, let p = path else { return nil }
        field = nil
        path = nil
        onCommit = nil
        let text = f.stringValue
        let hadFocus = f.currentEditor() != nil
        f.removeFromSuperview()
        if hadFocus, let w = window { w.makeFirstResponder(list) }
        return (p, text)
    }

    // Cancel without committing.
    func cancel(window: NSWindow?) {
        guard let f = field else { return }
        let hadFocus = f.currentEditor() != nil
        field = nil
        path = nil
        onCommit = nil
        f.removeFromSuperview()
        if hadFocus, let w = window { w.makeFirstResponder(nil) }
    }

    // MARK: NSTextFieldDelegate

    // Intercept field-editor commands: Return/Tab end editing (commit), Esc cancels.
    func control(_ control: NSControl, textView: NSTextView,
                 doCommandBy commandSelector: Selector) -> Bool {
        guard control === field else { return false }
        switch commandSelector {
        case #selector(NSResponder.insertNewline(_:)),
             #selector(NSResponder.insertTab(_:)),
             #selector(NSResponder.insertBacktab(_:)):
            onCommit?()
            return true
        case #selector(NSResponder.cancelOperation(_:)):
            cancel(window: NSApp.keyWindow)
            return true
        default:
            return false
        }
    }

    // Clicking away from the rename field commits it (Finder does the same).
    func controlTextDidEndEditing(_ obj: Notification) {
        onCommit?()
    }
}
