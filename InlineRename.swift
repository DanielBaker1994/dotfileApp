import AppKit

final class InlineRename: NSObject, NSTextFieldDelegate {
    private var field: NSTextField?
    private var path: String?
    var onCommit: (() -> Void)?

    var textField: NSTextField? { field }

    @discardableResult
    static func begin(parent: NSView, nameRect: NSRect, name: String, isDir: Bool,
                      colors: PopupColors, fontSize: CGFloat = 12, onCommit: (() -> Void)? = nil) -> InlineRename {
        let r = InlineRename()
        r.onCommit = onCommit
        r._begin(parent: parent, nameRect: nameRect, name: name, isDir: isDir, colors: colors, fontSize: fontSize)
        return r
    }

    private func _begin(parent: NSView, nameRect: NSRect, name: String, isDir: Bool, colors: PopupColors, fontSize: CGFloat) {
        let f = NSTextField(string: name)
        f.font = NSFont.systemFont(ofSize: fontSize)
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
        path = nil

        let nameStr = f.stringValue as NSString
        let stem = isDir ? nameStr.length : (nameStr.deletingPathExtension as NSString).length
        f.currentEditor()?.selectedRange = NSRange(location: 0, length: stem > 0 ? stem : nameStr.length)
    }

    func setPath(_ p: String) { path = p }

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

    func cancel(window: NSWindow?) {
        guard let f = field else { return }
        let hadFocus = f.currentEditor() != nil
        field = nil
        path = nil
        onCommit = nil
        f.removeFromSuperview()
        if hadFocus, let w = window { w.makeFirstResponder(nil) }
    }

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

    func controlTextDidEndEditing(_ obj: Notification) {
        onCommit?()
    }
}
