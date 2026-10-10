import AppKit

struct SwitchViewRow: PopupRow {
    let navID: Int
    let name: String
    let location: String
    let aliases: [String]
    let number: Int
    let icon: NSImage
    let isCurrent: Bool
    var title: String { name }
    var icons: [NSImage] { [icon] }
    func cellText(_ field: String) -> String? { nil }
}

final class ViewSwitcherPanel {
    let window: PopupWindow
    private var all: [SwitchViewRow] = []
    var onPick: ((Int) -> Void)?

    init(colors: PopupColors) {
        var cfg = PopupConfig(name: "view-switcher")
        cfg.colors = colors
        cfg.toolPanel = true
        cfg.floating = true
        cfg.enableToggle = false
        cfg.dynamicHeight = true
        cfg.width = 520
        cfg.rowHeight = 32
        cfg.searchPlaceholder = "switch view — type to filter · ↩ open · 1–9 jump"
        window = PopupWindow(config: cfg)
        window.onFilter = { [weak self] q in self?.filter(q) ?? [] }
        window.onAccept = { [weak self] row in
            guard let r = row as? SwitchViewRow else { return }
            self?.pick(r)
        }
        window.onEscape = { [weak self] in self?.window.hide(restore: false) }
        window.onKeyPreview = { [weak self] code, mods in self?.key(code, mods) ?? false }
        window.onDrawRow = { [weak self] rect, row, sel in self?.draw(rect, row, sel) }
    }

    func show(_ views: [(id: Int, name: String, icon: NSImage, location: String, aliases: [String])], current: Int?, preselect: Int?,
              over host: NSWindow?) {
        all = views.enumerated().map { i, v in
            SwitchViewRow(navID: v.id, name: v.name, location: v.location, aliases: v.aliases, number: i < 9 ? i + 1 : 0,
                          icon: v.icon, isCurrent: v.id == current)
        }
        window.show()
        if let h = host?.frame {
            let f = window.nativeWindow.frame
            window.nativeWindow.setFrameOrigin(NSPoint(x: h.midX - f.width / 2, y: h.midY - f.height / 2 + h.height * 0.12))
        }
        window.selection = all.firstIndex { $0.navID == preselect } ?? 0
    }

    private func filter(_ q: String) -> [PopupRow] {
        let s = q.trimmingCharacters(in: .whitespaces)
        guard !s.isEmpty else { return all }
        return PopupFuzzy.filter(all, query: s) { "\($0.name) \($0.location) \($0.aliases.joined(separator: " "))" }
    }

    private func key(_ code: UInt16, _ mods: NSEvent.ModifierFlags) -> Bool {
        let m = mods.intersection([.command, .control, .option])
        let digits: [UInt16: Int] = [18: 1, 19: 2, 20: 3, 21: 4, 23: 5, 22: 6, 26: 7, 28: 8, 25: 9]
        if m.isEmpty, window.currentQuery.isEmpty, let n = digits[code],
           let r = all.first(where: { $0.number == n }) {
            pick(r)
            return true
        }
        return false
    }

    private func pick(_ r: SwitchViewRow) {
        window.hide(restore: false)
        onPick?(r.navID)
    }

    private func draw(_ rect: NSRect, _ row: PopupRow, _ selected: Bool) {
        guard let r = row as? SwitchViewRow else { return }
        let c = window.config.colors
        let inner = rect.insetBy(dx: 6, dy: 2)
        if selected {
            c.highlight.setFill()
            NSBezierPath(roundedRect: inner, xRadius: 7, yRadius: 7).fill()
            c.accent.setFill()
            NSBezierPath(roundedRect: NSRect(x: inner.minX, y: inner.minY + 4, width: 3, height: inner.height - 8),
                         xRadius: 1.5, yRadius: 1.5).fill()
        }
        let mono = NSFont.monospacedSystemFont(ofSize: 11, weight: .regular)
        var x = inner.minX + 12
        func text(_ s: String, _ font: NSFont, _ color: NSColor, width: CGFloat) -> CGFloat {
            let para = NSMutableParagraphStyle()
            para.lineBreakMode = .byTruncatingMiddle
            let a: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: color, .paragraphStyle: para]
            let sz = (s as NSString).size(withAttributes: a)
            let w = min(sz.width, max(0, width))
            (s as NSString).draw(in: NSRect(x: x, y: inner.midY - sz.height / 2, width: w, height: sz.height),
                                 withAttributes: a)
            return w
        }
        _ = text(r.number > 0 ? "\(r.number)" : " ", mono, c.dim, width: 14)
        x += 18
        popupDrawImage(r.icon, in: NSRect(x: x, y: inner.midY - 9, width: 18, height: 18))
        x += 26
        let markW: CGFloat = 64
        let nameW = text(r.name, .systemFont(ofSize: 13, weight: .medium), c.text, width: 110)
        x += max(nameW, 96) + 10
        if !r.location.isEmpty {
            _ = text(r.location, .systemFont(ofSize: 12), c.dim, width: inner.maxX - markW - x - 8)
        }
        if r.isCurrent {
            let m = "● here" as NSString
            let a: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 11, weight: .medium), .foregroundColor: c.accent]
            let sz = m.size(withAttributes: a)
            m.draw(at: NSPoint(x: inner.maxX - 12 - sz.width, y: inner.midY - sz.height / 2), withAttributes: a)
        }
    }
}
