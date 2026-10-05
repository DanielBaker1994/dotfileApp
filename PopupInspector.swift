import AppKit

// The list's inspector: a read-only panel on the right that follows the
// highlighted row (Jira: the issue's key, title, state chips, description and
// people). Cmd+I shows / hides it, "Open full detail" = Return. The window
// owns the frame (`PopupWindow.layoutInspector`); the host feeds `content`.

public struct PopupInspectorContent {
    public var key: String
    public var title: String
    public var chips: [(text: String, tone: PopupTone)]
    public var fields: [(label: String, value: String)]
    public var body: String
    public init(key: String, title: String, chips: [(text: String, tone: PopupTone)],
                fields: [(label: String, value: String)], body: String) {
        self.key = key; self.title = title; self.chips = chips; self.fields = fields; self.body = body
    }
}

public final class PopupInspectorView: NSView, PopupThemeable {
    public var colors: PopupColors { didSet { render(); needsDisplay = true } }
    public var content: PopupInspectorContent? { didSet { render() } }
    public var onOpen: (() -> Void)?
    public var emptyText = "Select an issue"
    private let scroll = NSScrollView()
    private let text = NSTextView()
    private let button = ThemedPushButton(title: "Open full detail", target: nil, action: nil)
    private let hint = NSTextField(labelWithString: "⌘I hides this panel")
    private let zoom: () -> CGFloat

    public override var isFlipped: Bool { true }

    public init(colors: PopupColors, zoom: @escaping () -> CGFloat) {
        self.colors = colors
        self.zoom = zoom
        super.init(frame: .zero)
        text.isEditable = false
        text.isSelectable = true
        text.drawsBackground = false
        text.textContainerInset = NSSize(width: 14, height: 14)
        text.isVerticallyResizable = true
        text.isHorizontallyResizable = false
        text.textContainer?.widthTracksTextView = true
        scroll.documentView = text
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        scroll.drawsBackground = false
        scroll.scrollerStyle = .overlay
        addSubview(scroll)
        button.target = self
        button.action = #selector(openClicked)
        button.role = .primary
        button.rowStyle = true
        button.keycap = "⏎"
        button.controlSize = .regular
        button.toolTip = "Open the full issue (Return)"
        addSubview(button)
        hint.font = .systemFont(ofSize: 11)
        hint.alignment = .center
        addSubview(hint)
        render()
    }
    public required init?(coder: NSCoder) { fatalError() }

    public func applyColors(_ c: PopupColors) { colors = c }
    @objc private func openClicked() { onOpen?() }

    public override func layout() {
        super.layout()
        let footer: CGFloat = 70
        scroll.frame = NSRect(x: 1, y: 0, width: bounds.width - 1, height: max(0, bounds.height - footer))
        text.frame.size.width = scroll.contentSize.width
        button.sizeToFit()
        let bw = max(160, bounds.width - 36)
        button.frame = NSRect(x: (bounds.width - bw) / 2, y: bounds.height - footer + 8, width: bw, height: 32)
        hint.frame = NSRect(x: 8, y: bounds.height - footer + 42, width: bounds.width - 16, height: 16)
    }

    public override func draw(_ dirty: NSRect) {
        colors.mantle.withAlphaComponent(0.7).setFill()
        bounds.fill()
        colors.hairline.setFill()
        NSRect(x: 0, y: 0, width: 1, height: bounds.height).fill()
    }

    private func render() {
        hint.textColor = colors.dim
        button.isHidden = content == nil
        hint.isHidden = content == nil
        guard let c = content else {
            text.textStorage?.setAttributedString(NSAttributedString(
                string: emptyText, attributes: [.font: NSFont.systemFont(ofSize: 13), .foregroundColor: colors.dim]))
            return
        }
        let z = max(0.5, zoom())
        func attrs(_ size: CGFloat, _ weight: NSFont.Weight = .regular, _ color: NSColor,
                   after: CGFloat = 0, line: CGFloat = 0) -> [NSAttributedString.Key: Any] {
            let p = NSMutableParagraphStyle()
            p.paragraphSpacing = after
            p.lineSpacing = line
            return [.font: NSFont.systemFont(ofSize: size * z, weight: weight), .foregroundColor: color, .paragraphStyle: p]
        }
        let s = NSMutableAttributedString()
        s.append(NSAttributedString(string: c.key + "\n", attributes: attrs(12.5, .bold, colors.accent, after: 4)))
        s.append(NSAttributedString(string: c.title + "\n", attributes: attrs(16, .semibold, colors.text, after: 10, line: 2)))
        if !c.chips.isEmpty {
            for (i, chip) in c.chips.enumerated() {
                var a = attrs(12, .semibold, colors.tone(chip.tone), after: 12)
                a[.backgroundColor] = colors.tone(chip.tone).withAlphaComponent(0.18)
                s.append(NSAttributedString(string: " \(chip.text) ", attributes: a))
                if i + 1 < c.chips.count { s.append(NSAttributedString(string: "  ", attributes: attrs(12, .regular, colors.text))) }
            }
            s.append(NSAttributedString(string: "\n", attributes: attrs(12, .regular, colors.text, after: 12)))
        }
        if !c.body.isEmpty {
            let body = c.body.count > 700 ? String(c.body.prefix(700)) + "…" : c.body
            s.append(NSAttributedString(string: body + "\n", attributes: attrs(13, .regular, colors.text, after: 14, line: 3)))
        }
        let tab = NSMutableParagraphStyle()
        tab.tabStops = [NSTextTab(textAlignment: .left, location: 84 * z)]
        tab.headIndent = 84 * z
        tab.paragraphSpacing = 5
        for f in c.fields where !f.value.isEmpty {
            s.append(NSAttributedString(string: f.label + "\t", attributes: [
                .font: NSFont.systemFont(ofSize: 12 * z), .foregroundColor: colors.dim, .paragraphStyle: tab]))
            s.append(NSAttributedString(string: f.value + "\n", attributes: [
                .font: NSFont.systemFont(ofSize: 12.5 * z), .foregroundColor: colors.text, .paragraphStyle: tab]))
        }
        text.textStorage?.setAttributedString(s)
        scroll.documentView?.scroll(.zero)
    }
}
