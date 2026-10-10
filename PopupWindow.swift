import AppKit
import PDFKit
import Quartz
import SwiftTerm
import Foundation
import Darwin
import UniformTypeIdentifiers

extension NSScreen {
    var gapFrame: NSRect {
        let top = localizedName.lowercased().contains("built-in") ? settings.marginTopBuiltin : settings.marginTop
        var v = visibleFrame
        v.size.height = max(100, v.height - top - settings.marginBottom)
        v.origin.y += settings.marginBottom
        return v
    }
}

func clampToScreen(_ f: NSRect) -> NSRect {
    let screen = NSScreen.screens.first { $0.frame.contains(f.origin) }
        ?? NSScreen.main!
    let vis = screen.gapFrame
    let slack: CGFloat = 2
    var r = f
    if r.width > vis.width + slack { r.size.width = vis.width }
    if r.height > vis.height + slack { r.size.height = vis.height }
    if r.origin.x < vis.minX - slack { r.origin.x = vis.minX }
    if r.origin.y < vis.minY - slack { r.origin.y = vis.minY }
    if r.maxX > vis.maxX + slack { r.origin.x = vis.maxX - r.width }
    if r.maxY > vis.maxY + slack { r.origin.y = vis.maxY - r.height }
    return r
}

protocol PageZoomable: AnyObject { var pageZoom: CGFloat { get set } }

func editorFont(_ name: String?, _ zoom: CGFloat, size: CGFloat = 13) -> NSFont {
    name.flatMap { NSFont(name: $0, size: size * zoom) }
        ?? NSFont.monospacedSystemFont(ofSize: size * zoom, weight: .regular)
}

func parseANSI(_ s: String, baseFont: NSFont, defaultColor: NSColor) -> NSAttributedString {
    let ansiColor: (Int) -> NSColor = { i in
        switch i {
        case 0: return .systemGray
        case 1: return .systemRed
        case 2: return .systemGreen
        case 3: return .systemYellow
        case 4: return .systemBlue
        case 5: return .magenta
        case 6: return .cyan
        default: return .white
        }
    }
    let style = { (fg: NSColor, bold: Bool) -> [NSAttributedString.Key: Any] in
        let f = bold ? NSFontManager.shared.convert(baseFont, toHaveTrait: .boldFontMask) : baseFont
        return [.font: f, .foregroundColor: fg]
    }
    let re = try! NSRegularExpression(pattern: "\u{1B}\\[([0-9;]*)m")
    let ns = s as NSString
    let out = NSMutableAttributedString()
    var fg = defaultColor
    var bold = false
    var pos = 0
    for m in re.matches(in: s, range: NSRange(location: 0, length: ns.length)) {
        if m.range.location > pos {
            out.append(NSAttributedString(
                string: ns.substring(with: NSRange(location: pos, length: m.range.location - pos)),
                attributes: style(fg, bold)))
        }
        for c in ns.substring(with: m.range(at: 1))
            .split(separator: ";").compactMap({ Int($0) }) {
            switch c {
            case 0: fg = defaultColor; bold = false
            case 1: bold = true
            case 22: bold = false
            case 30...37: fg = ansiColor(c - 30)
            case 90...97: fg = ansiColor(c - 90)
            case 39: fg = defaultColor
            default: break
            }
        }
        pos = m.range.location + m.range.length
    }
    if pos < ns.length {
        out.append(NSAttributedString(string: ns.substring(from: pos),
                                      attributes: style(fg, bold)))
    }
    return out
}

public func popupHighlightSyntax(_ text: String, font: NSFont,
                                 colors: PopupColors) -> NSAttributedString {
    let t = text.trimmingCharacters(in: .whitespacesAndNewlines)
    if let f = t.first {
        if f == "<" { return popupHighlightXML(text, font: font, colors: colors) }
        if f == "{" || f == "[" { return popupHighlightJSON(text, font: font, colors: colors) }
    }
    return NSAttributedString(string: text, attributes: [
        .font: font, .foregroundColor: colors.text,
    ])
}

private let popupKeyColor = NSColor(srgbRed: 0.48, green: 0.70, blue: 1.00, alpha: 1)
private let popupStrColor = NSColor(srgbRed: 0.55, green: 0.80, blue: 0.52, alpha: 1)
private let popupNumColor = NSColor(srgbRed: 0.95, green: 0.66, blue: 0.30, alpha: 1)
private let popupKwColor  = NSColor(srgbRed: 0.73, green: 0.62, blue: 0.95, alpha: 1)

private func popupAttr(_ s: String, _ font: NSFont, _ color: NSColor,
                       italic: Bool = false) -> NSAttributedString {
    var attrs: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: color]
    if italic { attrs[.obliqueness] = 0.15 }
    return NSAttributedString(string: s, attributes: attrs)
}

private let popupJSONRegex = try! NSRegularExpression(
    pattern: #"("(?:[^"\\]|\\.)*")|(-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?)|(\btrue\b|\bfalse\b|\bnull\b)"#)

private func popupHighlightJSON(_ text: String, font: NSFont,
                                colors: PopupColors) -> NSAttributedString {
    let out = NSMutableAttributedString()
    let ns = text as NSString
    let len = ns.length
    var pos = 0
    var run = ""
    var runColor = colors.text
    func flush() {
        if !run.isEmpty { out.append(popupAttr(run, font, runColor)); run = "" }
    }
    func pushSegment(_ seg: String) {
        for u in seg.unicodeScalars {
            let isPunct = u == "{" || u == "}" || u == "[" || u == "]"
                || u == "," || u == ":"
            let c = isPunct ? colors.dim : colors.text
            if c != runColor { flush(); runColor = c }
            run += String(u)
        }
    }
    for m in popupJSONRegex.matches(in: text, range: NSRange(location: 0, length: len)) {
        if m.range.location > pos {
            pushSegment(ns.substring(with: NSRange(location: pos,
                                                  length: m.range.location - pos)))
        }
        flush()
        let r = m.range
        let s = ns.substring(with: r)
        if m.range(at: 1).location != NSNotFound {
            var after = NSMaxRange(r)
            while after < len {
                let c = ns.character(at: after)
                if c == 32 || c == 9 { after += 1; continue }
                break
            }
            let isKey = after < len && ns.character(at: after) == 58
            out.append(popupAttr(s, font, isKey ? popupKeyColor : popupStrColor))
        } else if m.range(at: 2).location != NSNotFound {
            out.append(popupAttr(s, font, popupNumColor))
        } else {
            out.append(popupAttr(s, font, popupKwColor))
        }
        pos = NSMaxRange(r)
    }
    if pos < len {
        pushSegment(ns.substring(from: pos))
    }
    flush()
    return out
}

private func popupHighlightXML(_ text: String, font: NSFont,
                               colors: PopupColors) -> NSAttributedString {
    let out = NSMutableAttributedString()
    let ns = text as NSString
    let len = ns.length
    var i = 0
    while i < len {
        if ns.character(at: i) == 60 {
            if ns.substring(with: NSRange(location: i, length: min(4, len - i))) == "<!--" {
                let close = ns.range(of: "-->", options: [],
                                     range: NSRange(location: i, length: len - i))
                if close.location != NSNotFound {
                    let seg = NSRange(location: i, length: NSMaxRange(close) - i)
                    out.append(popupAttr(ns.substring(with: seg), font, colors.dim, italic: true))
                    i = NSMaxRange(seg)
                    continue
                }
            }
            var gt = i + 1
            var quote: unichar = 0
            while gt < len {
                let c = ns.character(at: gt)
                if quote != 0 {
                    if c == quote { quote = 0 }
                } else if c == 34 || c == 39 {
                    quote = c
                } else if c == 62 { break }
                gt += 1
            }
            if gt >= len { gt = len - 1 }
            let tagRange = NSRange(location: i, length: gt - i + 1)
            out.append(popupAttributedTag(ns.substring(with: tagRange), font: font,
                                          colors: colors))
            i = gt + 1
        } else {
            let rest = NSRange(location: i, length: len - i)
            let n = ns.range(of: "<", options: [], range: rest)
            if n.location != NSNotFound {
                out.append(popupAttr(ns.substring(with: NSRange(location: i, length: n.location - i)),
                                     font, colors.text))
                i = n.location
            } else {
                out.append(popupAttr(ns.substring(from: i), font, colors.text))
                break
            }
        }
    }
    return out
}

private func popupAttributedTag(_ tag: String, font: NSFont,
                                colors: PopupColors) -> NSAttributedString {
    let out = NSMutableAttributedString()
    let ns = tag as NSString
    let len = ns.length
    var k = 0
    if len >= 2 {
        let p2 = ns.substring(with: NSRange(location: 0, length: 2))
        if p2 == "</" || p2 == "<?" || p2 == "<!" {
            out.append(popupAttr(p2, font, colors.dim))
            k = 2
        } else {
            out.append(popupAttr("<", font, colors.dim))
            k = 1
        }
    }
    var j = k
    while j < len {
        let c = ns.character(at: j)
        if c == 32 || c == 9 || c == 62 || c == 47 || c == 63 { break }
        j += 1
    }
    if j > k {
        out.append(popupAttr(ns.substring(with: NSRange(location: k, length: j - k)),
                             font, popupKeyColor))
        k = j
    }
    while k < len {
        let c = ns.character(at: k)
        if c == 62 || c == 47 || c == 63 { break }
        if c == 32 || c == 9 {
            var w = k
            while w < len && (ns.character(at: w) == 32 || ns.character(at: w) == 9) { w += 1 }
            out.append(popupAttr(ns.substring(with: NSRange(location: k, length: w - k)),
                                 font, colors.dim))
            k = w
            continue
        }
        var nameEnd = k
        while nameEnd < len && ns.character(at: nameEnd) != 61 { nameEnd += 1 }
        out.append(popupAttr(ns.substring(with: NSRange(location: k, length: nameEnd - k)),
                             font, popupKwColor))
        k = nameEnd
        if k < len && ns.character(at: k) == 61 {
            out.append(popupAttr("=", font, colors.dim))
            k += 1
            let v = ns.character(at: k)
            if v == 34 || v == 39 {
                var e = k + 1
                while e < len && ns.character(at: e) != v { e += 1 }
                if e < len { e += 1 }
                out.append(popupAttr(ns.substring(with: NSRange(location: k, length: e - k)),
                                     font, popupStrColor))
                k = e
            } else {
                var e = k
                while e < len, ns.character(at: e) != 32,
                      ns.character(at: e) != 9, ns.character(at: e) != 62 { e += 1 }
                out.append(popupAttr(ns.substring(with: NSRange(location: k, length: e - k)),
                                     font, popupStrColor))
                k = e
            }
        }
    }
    if k < len {
        out.append(popupAttr(ns.substring(from: k), font, colors.dim))
    }
    return out
}

public func appendToFile(_ path: String, _ text: String) {
    let data = Data(text.utf8)
    if let fh = FileHandle(forWritingAtPath: path) {
        fh.seekToEndOfFile()
        fh.write(data)
        try? fh.close()
    } else {
        FileManager.default.createFile(atPath: path, contents: data)
    }
}

public let debugLogPath = "/tmp/ws-debug.log"

public func popupTmpDir() -> String {
    let t = ProcessInfo.processInfo.environment["TMPDIR"] ?? ""
    let d = t.isEmpty ? "/tmp/" : t
    return d.hasSuffix("/") ? d : d + "/"
}

public func makeUnixSockAddr(_ path: String) -> sockaddr_un {
    var addr = sockaddr_un()
    addr.sun_family = sa_family_t(AF_UNIX)
    let chars = path.utf8CString
    let count = min(chars.count, MemoryLayout.size(ofValue: addr.sun_path))
    withUnsafeMutableBytes(of: &addr.sun_path) { dest in
        chars.withUnsafeBufferPointer { src in
            dest.baseAddress?.copyMemory(from: src.baseAddress!, byteCount: count)
        }
    }
    return addr
}

public func listenUnixSocket(_ path: String) -> Int32? {
    unlink(path)
    let fd = socket(AF_UNIX, SOCK_STREAM, 0)
    guard fd >= 0 else { return nil }
    var addr = makeUnixSockAddr(path)
    let bound = withUnsafePointer(to: &addr) { ptr -> Bool in
        ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) {
            Darwin.bind(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) == 0
        }
    }
    guard bound else { close(fd); return nil }
    listen(fd, 4)
    return fd
}

public struct PopupColors {
    public var background: NSColor
    public var border: NSColor
    public var text: NSColor
    public var dim: NSColor
    public var highlight: NSColor
    public var accent: NSColor
    public var palette: PopupPalette = PopupPalette()

    public init(background: NSColor = NSColor(srgbRed: 36/255, green: 39/255, blue: 58/255, alpha: 1),
                border: NSColor = NSColor(srgbRed: 159/255, green: 200/255, blue: 232/255, alpha: 1),
                text: NSColor = NSColor(srgbRed: 202/255, green: 211/255, blue: 245/255, alpha: 1),
                dim: NSColor = NSColor(srgbRed: 147/255, green: 154/255, blue: 183/255, alpha: 1),
                highlight: NSColor = NSColor(srgbRed: 63/255, green: 74/255, blue: 90/255, alpha: 1),
                accent: NSColor = NSColor(srgbRed: 85/255, green: 104/255, blue: 130/255, alpha: 1),
                palette: PopupPalette = PopupPalette()) {
        self.background = background
        self.border = border
        self.text = text
        self.dim = dim
        self.highlight = highlight
        self.accent = accent
        self.palette = palette
    }
}

public struct PopupPalette {
    public var accent2: NSColor
    public var success: NSColor
    public var warning: NSColor
    public var danger: NSColor
    public var info: NSColor

    public init(accent2: NSColor = NSColor(srgbRed: 138/255, green: 173/255, blue: 244/255, alpha: 1),
                success: NSColor = NSColor(srgbRed: 166/255, green: 218/255, blue: 149/255, alpha: 1),
                warning: NSColor = NSColor(srgbRed: 238/255, green: 212/255, blue: 159/255, alpha: 1),
                danger: NSColor = NSColor(srgbRed: 237/255, green: 135/255, blue: 150/255, alpha: 1),
                info: NSColor = NSColor(srgbRed: 145/255, green: 215/255, blue: 227/255, alpha: 1)) {
        self.accent2 = accent2
        self.success = success
        self.warning = warning
        self.danger = danger
        self.info = info
    }
    public var all: [NSColor] { [accent2, success, warning, danger, info] }
    public init?(_ colors: [NSColor]) {
        guard colors.count == 5 else { return nil }
        self.init(accent2: colors[0], success: colors[1], warning: colors[2],
                  danger: colors[3], info: colors[4])
    }
}

public enum PopupTone { case text, dim, accent, accent2, success, warning, danger, info }

public struct PopupCellStyle {
    public enum Mark { case hollow, half, filled }
    public var tone: PopupTone
    public var mark: Mark?
    public var tinted: Bool
    public var bold: Bool
    public var quietsRow: Bool
    public init(_ tone: PopupTone, mark: Mark? = nil, tinted: Bool = false,
                bold: Bool = false, quietsRow: Bool = false) {
        self.tone = tone; self.mark = mark; self.tinted = tinted
        self.bold = bold; self.quietsRow = quietsRow
    }
}

public extension PopupColors {
    var isLight: Bool { ButtonStyle.luminance(background) > 0.45 }
    var base: NSColor { ButtonStyle.opaque(background) }
    private func deeper(_ f: CGFloat) -> NSColor {
        isLight ? (base.blended(withFraction: f * 0.35, of: ButtonStyle.opaque(text)) ?? base)
                : (base.blended(withFraction: f, of: .black) ?? base)
    }
    var mantle: NSColor { deeper(0.22) }
    var crust: NSColor { deeper(0.42) }
    var surface0: NSColor { base.blended(withFraction: 0.09, of: ButtonStyle.opaque(text)) ?? base }
    var surface1: NSColor { base.blended(withFraction: 0.16, of: ButtonStyle.opaque(text)) ?? base }
    var accentOn: NSColor { ButtonStyle.accent(self) }
    var onAccent: NSColor {
        let a = accentOn
        return ButtonStyle.contrast(crust, a) >= 4.5 ? crust
            : ButtonStyle.contrast(.white, a) >= ButtonStyle.contrast(.black, a) ? .white : .black
    }
    func readable(_ c: NSColor, min ratio: CGFloat = 3) -> NSColor {
        var out = ButtonStyle.opaque(c)
        var step = 0
        while ButtonStyle.contrast(out, base) < ratio, step < 6 {
            out = out.blended(withFraction: 0.2, of: ButtonStyle.opaque(text)) ?? out
            step += 1
        }
        return out
    }
    func tone(_ t: PopupTone) -> NSColor {
        switch t {
        case .text: return text
        case .dim: return dim
        case .accent: return accentOn
        case .accent2: return readable(palette.accent2)
        case .success: return readable(palette.success)
        case .warning: return readable(palette.warning)
        case .danger: return readable(palette.danger)
        case .info: return readable(palette.info)
        }
    }
    func over(_ top: NSColor, _ alpha: CGFloat, on bottom: NSColor) -> NSColor {
        let b = ButtonStyle.opaque(bottom)
        return b.blended(withFraction: alpha, of: ButtonStyle.opaque(top)) ?? b
    }
    func ensure(_ fg: NSColor, on bg: NSColor, _ ratio: CGFloat = 4.5) -> NSColor {
        let b = ButtonStyle.opaque(bg)
        let pole: NSColor = ButtonStyle.contrast(.black, b) >= ButtonStyle.contrast(.white, b) ? .black : .white
        var out = ButtonStyle.opaque(fg)
        var step = 0
        while ButtonStyle.contrast(out, b) < ratio, step < 12 {
            out = out.blended(withFraction: 0.15, of: pole) ?? out
            step += 1
        }
        return out
    }
    var hairline: NSColor { text.withAlphaComponent(isLight ? 0.12 : 0.08) }
    var outline: NSColor {
        (accentOn.blended(withFraction: 0.45, of: base) ?? accentOn).withAlphaComponent(0.85)
    }
}

public struct PopupConfig {
    public var name: String

    public var width: CGFloat = 250
    public var rowHeight: CGFloat = 30
    public var padding: CGFloat = 8
    public var headerHeight: CGFloat = 30
    public var cornerRadius: CGFloat = 9

    public var inputFontSize: CGFloat = 12
    public var rowFontSize: CGFloat = 11

    public var buttonRadius: CGFloat = 4
    public var buttonFontSize: CGFloat = 10.5

    public var zoom: CGFloat = 1.0

    public var tintAlpha: CGFloat = 0.78
    public var opaqueTabs: Bool = true
    public var material: NSVisualEffectView.Material = .hudWindow
    public var hasShadow: Bool = true
    public var colors: PopupColors = PopupColors()
    public var headerColor: NSColor? = nil
    public var titlePill: Bool = true
    public var stretchHeaderButtons: Bool = false

    public var enableSearch: Bool = true
    public var enableNavigation: Bool = true
    public var wrapNavigation: Bool = true
    public var enableEscape: Bool = true
    public var enableToggle: Bool = true
    public var dismissOnClickOff: Bool = true
    public var dynamicHeight: Bool = false
    public var enableResize: Bool = false
    public var enableDrag: Bool = false

    public var sticky: Bool = false

    public var floating: Bool = true

    public var toolPanel: Bool = false

    public var wrapContent: Bool = false
    public var maxHeight: CGFloat = 0
    public var editMode: Bool = false
    public var height: CGFloat = 420

    public var terminal: Bool = false
    public var terminalHeight: CGFloat = 240
    public var terminalDir = "/tmp/"
    public var fileBrowserHeight: CGFloat = 300
    public var browserSort = "name"
    public var browserSortDescending = false
    public var browserSearchLimit = 2000
    public var browserSearchExcludes = ["/Library", "node_modules", ".Trash"]
    public var browserTerminalWords = ["term", "terminal", "cmd"]
    public var fileBrowserBackground = NSColor(srgbRed: 0.31, green: 0.35, blue: 0.43, alpha: 0.55)
    public var terminalBackground = NSColor(srgbRed: 0.31, green: 0.35, blue: 0.43, alpha: 0.78)
    public var terminalForeground: NSColor? = nil
    public var fileBrowserDefault = false
    public var terminalStartsOpen = true
    public var shell = "/opt/homebrew/bin/bash"
    public var terminalFont = "Hack Nerd Font"
    public var shellArgs: [String] = ["--login", "-i"]
    public var terminalFontSize: CGFloat = 13
    public var editorFontSize: CGFloat = 13
    public var vimEditorExecutable: String?
    public var vimEditorArgs: [String] = []
    public var vimEditorSocket: String?
    public var escCloseCount: Int = 1
    public var copyToast: String = "Copied {} to clipboard"
    public var vimImageFile: String?
    public var showCloseButton: Bool = false
    public var headerCloseButton: Bool = true

    public var showSearchBar: Bool = false
    public var searchPlaceholder: String = "search…"
    public var searchWidthFraction: CGFloat = 0.8

    public var filters: Bool = false
    public var filterBarHeight: CGFloat = 26

    public var dragHeader: Bool = false

    public var tabs: Bool = false
    public var tabBarHeight: CGFloat = 30
    public var tabsAddButton: Bool = false
    public var tabsSidebarWidth: CGFloat = 0
    public var tabsSidebarTitle = "Notes"
    public var inspectorWidth: CGFloat = 0

    public var scrollableRows: Bool = false

    public var clickToSelect: Bool = false

    public var selectableRows: Bool = false
    public var copyRowsButton: Bool = true
    public var rowStars: Bool = false
    public var rowLeadInset: CGFloat {
        padding + 10 + (selectableRows ? 22 : 0) + (rowStars ? 20 : 0)
    }

    public var maxRowStretch: CGFloat = 26

    public var highlightMatches: Bool = false

    public var tableColumns: [PopupTableColumn] = []
    public var tableHeaderHeight: CGFloat = 24
    public var tableCellStyle: ((String, String) -> PopupCellStyle?)?

    public var fontName: String?

    public var markdownImages = false

    public func rowFont(_ size: CGFloat, weight: NSFont.Weight = .regular) -> NSFont {
        if let f = fontName, let n = NSFont(name: f, size: size) { return n }
        return NSFont.systemFont(ofSize: size, weight: weight)
    }

    public var bodyMaxLines: Int = 5

    public init(name: String) {
        self.name = name
    }
}

public protocol PopupRow {
    var title: String { get }
    var icons: [NSImage] { get }
    var trailing: String? { get }
    var content: String? { get }
    var detail: String? { get }
    var body: String? { get }
    var loadMore: Bool { get }
    var groupHeader: Bool { get }
    var starred: Bool? { get }
    func cellText(_ field: String) -> String?
}

public extension PopupRow {
    var icons: [NSImage] { [] }
    var trailing: String? { nil }
    var content: String? { nil }
    var detail: String? { nil }
    var body: String? { nil }
    var loadMore: Bool { false }
    var groupHeader: Bool { false }
    var starred: Bool? { nil }
    func cellText(_ field: String) -> String? { nil }
}

public struct PopupTableColumn {
    public var field: String
    public var title: String
    public var width: CGFloat
    public var align: NSTextAlignment
    public var sortable: Bool
    public var filterable: Bool
    public init(field: String, title: String, width: CGFloat = 0,
                align: NSTextAlignment = .left, sortable: Bool = false,
                filterable: Bool = false) {
        self.field = field
        self.title = title
        self.width = width
        self.align = align
        self.sortable = sortable
        self.filterable = filterable
    }
}

extension Array where Element == PopupTableColumn {
    func frames(x0: CGFloat, usable: CGFloat) -> [(x: CGFloat, w: CGFloat)] {
        guard !isEmpty, usable > 0 else { return map { _ in (x0, 0) } }
        let explicit = reduce(CGFloat(0)) { $0 + Swift.max(0, $1.width) }
        let autos = filter { $0.width <= 0 }.count
        let leftover = Swift.max(0, 100 - explicit)
        var pcts = map { $0.width > 0 ? $0.width : (autos > 0 ? leftover / CGFloat(autos) : 0) }
        for i in pcts.indices where pcts[i] <= 0 { pcts[i] = 5 }
        let total = pcts.reduce(0, +)
        let scale = total > 100 ? 100 / total : 1
        var x = x0
        return pcts.map { p in
            let w = usable * p * scale / 100
            defer { x += w }
            return (x, w)
        }
    }
}

public protocol EscapableWindow: AnyObject {
    var onEscape: (() -> Void)? { get set }
}

protocol HeaderClickWindow: NSWindow {
    var headerClickBand: CGFloat { get set }
    var onHeaderClick: ((NSPoint) -> Void)? { get set }
}

struct HeaderClickTracker {
    private var down: NSPoint?

    mutating func track(_ event: NSEvent, in window: NSWindow, band: CGFloat) -> NSPoint? {
        guard band > 0 else { return nil }
        let loc = event.locationInWindow
        switch event.type {
        case .leftMouseDown:
            if loc.y >= window.frame.height - band {
                down = NSEvent.mouseLocation
            }
        case .leftMouseUp:
            if let d = down {
                down = nil
                let up = NSEvent.mouseLocation
                if abs(up.x - d.x) < 4, abs(up.y - d.y) < 4 { return loc }
            }
        default:
            break
        }
        return nil
    }
}

public class PopupBaseWindow: NSWindow, EscapableWindow, HeaderClickWindow {
    public var onEscape: (() -> Void)?

    var selectionAttributes: [NSAttributedString.Key: Any]?
    public override func fieldEditor(_ createFlag: Bool, for object: Any?) -> NSText? {
        let ed = super.fieldEditor(createFlag, for: object)
        if let tv = ed as? NSTextView, let a = selectionAttributes {
            tv.selectedTextAttributes = a
            tv.insertionPointColor = a[.foregroundColor] as? NSColor ?? tv.insertionPointColor
        }
        return ed
    }
    var headerClickBand: CGFloat = 0
    var onHeaderClick: ((NSPoint) -> Void)?
    private var headerTracker = HeaderClickTracker()

    public override var canBecomeKey: Bool { true }
    public override var canBecomeMain: Bool { true }

    public override func sendEvent(_ event: NSEvent) {
        if let p = headerTracker.track(event, in: self, band: headerClickBand) { onHeaderClick?(p) }
        super.sendEvent(event)
    }

    public override func cancelOperation(_ sender: Any?) {
        if let dismiss = PopupWindow.transientEscape { dismiss(); return }
        onEscape?()
    }

    var clickFocusesField = true
    public override func mouseDown(with event: NSEvent) {
        if clickFocusesField, let field = contentView?.subviews.compactMap({ $0 as? NSTextField }).first {
            makeFirstResponder(field)
        }
        super.mouseDown(with: event)
    }
}

public final class PopupPanel: NSPanel, EscapableWindow, HeaderClickWindow {
    public var onEscape: (() -> Void)?

    var headerClickBand: CGFloat = 0
    var onHeaderClick: ((NSPoint) -> Void)?
    private var headerTracker = HeaderClickTracker()

    public override func sendEvent(_ event: NSEvent) {
        if let p = headerTracker.track(event, in: self, band: headerClickBand) { onHeaderClick?(p) }
        super.sendEvent(event)
    }

    var selectionAttributes: [NSAttributedString.Key: Any]?
    public override func fieldEditor(_ createFlag: Bool, for object: Any?) -> NSText? {
        let ed = super.fieldEditor(createFlag, for: object)
        if let tv = ed as? NSTextView, let a = selectionAttributes {
            tv.selectedTextAttributes = a
            tv.insertionPointColor = a[.foregroundColor] as? NSColor ?? tv.insertionPointColor
        }
        return ed
    }

    public override var canBecomeKey: Bool { true }
    public override var canBecomeMain: Bool { true }

    public override func cancelOperation(_ sender: Any?) {
        if let dismiss = PopupWindow.transientEscape { dismiss(); return }
        onEscape?()
    }

    public override func mouseDown(with event: NSEvent) {
        if let field = contentView?.subviews.compactMap({ $0 as? NSTextField }).first {
            makeFirstResponder(field)
        }
        super.mouseDown(with: event)
    }
}

public class PopupPlainWindow: PopupBaseWindow {
    var cornerRadius: CGFloat = 9 { didSet { invalidateShadow() } }
    @objc func _cornerRadius() -> CGFloat { cornerRadius }
}

final class PopupPassThroughView: NSView {
    override var isFlipped: Bool { true }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
}

enum ButtonState { case idle, hover, pressed, on, onHover }

protocol PopupThemeable: AnyObject {
    func applyColors(_ c: PopupColors)
}
extension PopupTabsBar: PopupThemeable {
    func applyColors(_ c: PopupColors) { config.colors = c; needsDisplay = true }
}
extension PopupFilterBar: PopupThemeable {
    func applyColors(_ c: PopupColors) { config.colors = c; needsDisplay = true }
}
extension PopupRowView: PopupThemeable {
    func applyColors(_ c: PopupColors) { config.colors = c; needsDisplay = true }
}
extension PopupStatusBar: PopupThemeable {
    func applyColors(_ c: PopupColors) { config.colors = c; needsDisplay = true }
}
extension PopupChrome: PopupThemeable {
    func applyColors(_ c: PopupColors) { config.colors = c; needsDisplay = true }
}
extension ThemeButton: PopupThemeable {
    func applyColors(_ c: PopupColors) { config.colors = c; needsDisplay = true }
}
extension FileListPane: PopupThemeable {
    func applyColors(_ c: PopupColors) { config.colors = c; needsDisplay = true }
}
extension PopupTableHeaderView: PopupThemeable {
    func applyColors(_ c: PopupColors) { config.colors = c; needsDisplay = true }
}
extension PopupFileBrowser: PopupThemeable {
    func applyColors(_ c: PopupColors) { config.colors = c; retheme() }
}

enum ButtonStyle {
    static func fill(_ st: ButtonState, _ c: PopupColors) -> NSColor {
        switch st {
        case .idle:    return c.text.withAlphaComponent(0.06)
        case .hover:   return c.text.withAlphaComponent(0.11)
        case .pressed: return c.text.withAlphaComponent(0.16)
        case .on:      return c.accentOn.withAlphaComponent(c.isLight ? 0.18 : 0.22)
        case .onHover: return c.accentOn.withAlphaComponent(c.isLight ? 0.25 : 0.30)
        }
    }
    static func stroke(_ st: ButtonState, _ c: PopupColors) -> NSColor {
        .clear
    }
    static func accent(_ c: PopupColors) -> NSColor {
        let card = opaque(c.background)
        var a = opaque(c.accent)
        var step = 0
        while contrast(a, card) < 2.2, step < 6 {
            a = a.blended(withFraction: 0.2, of: opaque(c.text)) ?? a
            step += 1
        }
        return a
    }
    static func indicator(_ rect: NSRect, _ c: PopupColors) {
        let inset = min(8, rect.width * 0.2)
        let bar = NSRect(x: rect.minX + inset, y: rect.maxY - 2.5,
                         width: max(4, rect.width - inset * 2), height: 2)
        accent(c).setFill()
        NSBezierPath(roundedRect: bar, xRadius: 1, yRadius: 1).fill()
    }
    static func inputFill(_ c: PopupColors) -> NSColor {
        c.mantle.withAlphaComponent(c.isLight ? 0.55 : 0.6)
    }
    static func inputStroke(_ c: PopupColors) -> NSColor {
        c.text.withAlphaComponent(0.12)
    }
    static func focusStroke(_ c: PopupColors) -> NSColor { c.accentOn }
    static func text(_ st: ButtonState, _ c: PopupColors) -> NSColor {
        switch st {
        case .idle:                  return c.dim
        case .hover, .pressed:       return c.text.withAlphaComponent(0.92)
        case .on, .onHover:          return c.readable(c.accent, min: 4)
        }
    }
    static func font(_ size: CGFloat, _ st: ButtonState) -> NSFont {
        NSFont.systemFont(ofSize: size, weight: (st == .on || st == .onHover) ? .semibold : .medium)
    }

    static func selection(_ c: PopupColors) -> [NSAttributedString.Key: Any] {
        let card = opaque(c.background)
        var bg = opaque(c.highlight)
        var step = 0
        while contrast(bg, card) < 1.7, step < 6 {
            bg = bg.blended(withFraction: 0.18, of: opaque(c.text)) ?? bg
            step += 1
        }
        return [.backgroundColor: bg, .foregroundColor: readable(on: bg, preferred: c.text)]
    }
    static func opaque(_ c: NSColor) -> NSColor {
        (c.usingColorSpace(.sRGB) ?? c).withAlphaComponent(1)
    }
    static func luminance(_ c: NSColor) -> CGFloat {
        let s = opaque(c)
        func lin(_ v: CGFloat) -> CGFloat { v <= 0.03928 ? v / 12.92 : pow((v + 0.055) / 1.055, 2.4) }
        return 0.2126 * lin(s.redComponent) + 0.7152 * lin(s.greenComponent) + 0.0722 * lin(s.blueComponent)
    }
    static func contrast(_ a: NSColor, _ b: NSColor) -> CGFloat {
        let la = luminance(a), lb = luminance(b)
        return (max(la, lb) + 0.05) / (min(la, lb) + 0.05)
    }
    static func readable(on bg: NSColor, preferred: NSColor) -> NSColor {
        if contrast(preferred, bg) >= 4.5 { return opaque(preferred) }
        return contrast(.white, bg) >= contrast(.black, bg) ? .white : .black
    }

    static func draw(_ rect: NSRect, _ st: ButtonState, _ c: PopupColors, radius: CGFloat,
                     flat: Bool = false, indicator showBar: Bool = false) {
        if flat && st == .idle { return }
        let r = rect.insetBy(dx: 0.5, dy: 0.5)
        let path = NSBezierPath(roundedRect: r, xRadius: radius, yRadius: radius)
        fill(st, c).setFill()
        path.fill()
        if showBar, st == .on || st == .onHover { indicator(r, c) }
    }

    static func symbol(_ name: String, in rect: NSRect, color: NSColor, size: CGFloat) {
        guard let base = NSImage(systemSymbolName: name, accessibilityDescription: nil)?
                .withSymbolConfiguration(.init(pointSize: size, weight: .semibold)) else { return }
        let tinted = NSImage(size: base.size, flipped: false) { r in
            base.draw(in: r)
            color.set()
            r.fill(using: .sourceAtop)
            return true
        }
        let o = NSPoint(x: (rect.midX - base.size.width / 2).rounded(),
                        y: (rect.midY - base.size.height / 2).rounded())
        tinted.draw(in: NSRect(origin: o, size: base.size), from: .zero, operation: .sourceOver,
                    fraction: 1, respectFlipped: true, hints: nil)
    }

    static func label(_ title: String, in rect: NSRect, _ st: ButtonState,
                      _ c: PopupColors, size: CGFloat) {
        let attrs: [NSAttributedString.Key: Any] = [
            .font: font(size, st), .foregroundColor: text(st, c),
        ]
        let s = title as NSString
        let sz = s.size(withAttributes: attrs)
        s.draw(at: NSPoint(x: rect.midX - sz.width / 2, y: rect.midY - sz.height / 2),
               withAttributes: attrs)
    }

    static func plus(in rect: NSRect, color: NSColor, arm: CGFloat) {
        let p = NSBezierPath()
        p.lineWidth = 1.5
        p.lineCapStyle = .round
        p.move(to: NSPoint(x: rect.midX - arm, y: rect.midY))
        p.line(to: NSPoint(x: rect.midX + arm, y: rect.midY))
        p.move(to: NSPoint(x: rect.midX, y: rect.midY - arm))
        p.line(to: NSPoint(x: rect.midX, y: rect.midY + arm))
        color.setStroke()
        p.stroke()
    }
    static func cross(in rect: NSRect, color: NSColor, arm: CGFloat) {
        let p = NSBezierPath()
        p.lineWidth = 1.3
        p.lineCapStyle = .round
        p.move(to: NSPoint(x: rect.midX - arm, y: rect.midY - arm))
        p.line(to: NSPoint(x: rect.midX + arm, y: rect.midY + arm))
        p.move(to: NSPoint(x: rect.midX - arm, y: rect.midY + arm))
        p.line(to: NSPoint(x: rect.midX + arm, y: rect.midY - arm))
        color.setStroke()
        p.stroke()
    }
    static func chevron(in rect: NSRect, color: NSColor) {
        let p = NSBezierPath()
        p.lineWidth = 1.3
        p.lineCapStyle = .round
        p.lineJoinStyle = .round
        p.move(to: NSPoint(x: rect.midX - 3, y: rect.midY - 1.5))
        p.line(to: NSPoint(x: rect.midX, y: rect.midY + 1.5))
        p.line(to: NSPoint(x: rect.midX + 3, y: rect.midY - 1.5))
        color.setStroke()
        p.stroke()
    }
}

final class PopupBackdrop: NSView {
    struct Edge: OptionSet {
        let rawValue: Int
        static let left = Edge(rawValue: 1 << 0)
        static let right = Edge(rawValue: 1 << 1)
        static let top = Edge(rawValue: 1 << 2)
        static let bottom = Edge(rawValue: 1 << 3)

        func resized(_ start: NSRect, dx: CGFloat, dy: CGFloat,
                     minW: CGFloat, minH: CGFloat) -> NSRect {
            var f = start
            if contains(.right) {
                f.size.width = max(minW, start.width + dx)
            } else if contains(.left) {
                f.size.width = max(minW, start.width - dx)
                f.origin.x += start.width - f.width
            }
            if contains(.top) {
                f.size.height = max(minH, start.height + dy)
            } else if contains(.bottom) {
                f.size.height = max(minH, start.height - dy)
                f.origin.y += start.height - f.height
            }
            return f
        }

        static func at(_ p: NSPoint, in size: NSSize, hit: CGFloat = 8) -> Edge {
            var e: Edge = []
            if p.x <= hit { e.insert(.left) }
            if p.x >= size.width - hit { e.insert(.right) }
            if p.y <= hit { e.insert(.top) }
            if p.y >= size.height - hit { e.insert(.bottom) }
            return e
        }
    }

    struct Resize {
        private(set) var edges: Edge = []
        private var startFrame = NSRect.zero
        private var startPoint = NSPoint.zero

        mutating func begin(_ e: Edge, in window: NSWindow) {
            edges = e
            startFrame = window.frame
            startPoint = NSEvent.mouseLocation
        }
        func drag(_ window: NSWindow) {
            guard !edges.isEmpty else { return }
            let m = NSEvent.mouseLocation
            let f = edges.resized(startFrame, dx: m.x - startPoint.x, dy: m.y - startPoint.y,
                                  minW: 120, minH: 100)
            window.setFrame(clampToScreen(f), display: true)
            window.invalidateShadow()
        }
        mutating func end() -> Bool {
            defer { edges = [] }
            return !edges.isEmpty
        }
    }

    let config: PopupConfig
    private var trackingArea: NSTrackingArea?
    private var resize = Resize()

    override var isFlipped: Bool { true }

    init(config: PopupConfig, frame: NSRect) {
        self.config = config
        super.init(frame: frame)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) not supported") }

    private let resizeBand: CGFloat = 3
    override func hitTest(_ point: NSPoint) -> NSView? {
        if let win = window, win.styleMask.contains(.resizable), superview != nil {
            let p = convert(point, from: superview)
            if p.x < resizeBand || p.x > bounds.width - resizeBand
                || p.y < resizeBand || p.y > bounds.height - resizeBand {
                return nil
            }
        }
        return super.hitTest(point)
    }

    private func edges(at p: NSPoint) -> Edge { Edge.at(p, in: bounds.size) }

    private func cursor(for e: Edge) -> NSCursor {
        switch e {
        case [.left, .right]: return .resizeLeftRight
        case [.top, .bottom]: return .resizeUpDown
        case [.top, .left], [.bottom, .right]: return .resizeLeftRight
        case [.top, .right], [.bottom, .left]: return .resizeLeftRight
        default: return .arrow
        }
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let t = trackingArea { removeTrackingArea(t) }
        let ta = NSTrackingArea(rect: bounds,
                                options: [.activeAlways, .mouseMoved, .inVisibleRect],
                                owner: self, userInfo: nil)
        addTrackingArea(ta)
        trackingArea = ta
    }

    override func mouseMoved(with event: NSEvent) {
        guard config.enableResize, !(window?.styleMask.contains(.resizable) ?? false) else { return }
        let e = edges(at: convert(event.locationInWindow, from: nil))
        if e.isEmpty {
            NSCursor.arrow.set()
        } else {
            cursor(for: e).set()
        }
    }

    override func mouseExited(with event: NSEvent) {
        NSCursor.arrow.set()
    }

    override func mouseDown(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        let e = edges(at: p)
        if config.enableResize && !e.isEmpty, let win = window,
           !win.styleMask.contains(.resizable) {
            resize.begin(e, in: win)
            return
        }
        if config.enableDrag, let win = window {
            win.performDrag(with: event)
            return
        }
        super.mouseDown(with: event)
    }

    override func mouseDragged(with event: NSEvent) {
        if let win = window { resize.drag(win) }
    }

    override func mouseUp(with event: NSEvent) {
        _ = resize.end()
        super.mouseUp(with: event)
    }

    override func draw(_ dirtyRect: NSRect) {
        guard config.enableResize else { return }
        let w = bounds.width
        let h = bounds.height
        config.colors.dim.setStroke()
        let path = NSBezierPath()
        path.lineWidth = 1
        for i in 0..<3 {
            let s = CGFloat(i) * 4
            path.move(to: NSPoint(x: w - 15 + s, y: h - 4))
            path.line(to: NSPoint(x: w - 4, y: h - 15 + s))
        }
        path.stroke()
    }
}

public struct PopupTabBadge {
    public var tone: PopupTone
    public var text: String
    public var tip: String
    public init(tone: PopupTone, text: String, tip: String) {
        self.tone = tone
        self.text = text
        self.tip = tip
    }
}

final class PopupTabsBar: NSView {
    var config: PopupConfig
    var zoom: CGFloat = 1.0
    var titles: [String] = [] {
        didSet { needsDisplay = true }
    }
    var badges: [PopupTabBadge?] = [] {
        didSet { needsDisplay = true }
    }
    private func badge(_ i: Int) -> PopupTabBadge? { badges.indices.contains(i) ? badges[i] : nil }
    private var badgeFont: NSFont { .systemFont(ofSize: max(9, config.buttonFontSize * zoom - 1.5), weight: .medium) }
    private var dotW: CGFloat { 7 * zoom }
    private func badgeWidth(_ i: Int) -> CGFloat {
        guard let b = badge(i) else { return 0 }
        let tw = b.text.isEmpty ? 0 : (b.text as NSString).size(withAttributes: [.font: badgeFont]).width + 5 * zoom
        return dotW + 5 * zoom + tw
    }
    var selected = 0 {
        didSet { needsDisplay = true; if vertical, selected != oldValue { revealSelected() } }
    }
    var onSelect: ((Int) -> Void)?
    var onClick: ((Int) -> Void)?
    var onAddTab: (() -> Void)?
    var onCloseTab: ((Int) -> Void)?
    var onCopyPath: ((Int) -> Void)?
    var menuFor: ((Int) -> NSMenu?)?
    var pathTip: ((Int) -> String?)?
    var closable = true
    var vertical = false
    var sectionTitle = "Notes"
    var pinned: [String] = [] { didSet { needsDisplay = true; clampScroll() } }
    var onPinned: ((String) -> Void)?
    var pinnedTitle = "Pinned"
    var pinnedSelected: String? { didSet { needsDisplay = true } }
    var pinnedMenu: ((String) -> NSMenu?)?
    var maxPinnedShown = 5
    var pinnedIcon = "folder"
    var pinnedIconFor: ((String) -> String)?
    var pinnedSection: ((String) -> String)?
    var pinnedMeta: ((String) -> String?)?
    var rowTitleFor: ((Int) -> String?)?
    var quietOKBadges = false
    var statusLine: (text: String, tone: PopupTone)? { didSet { needsDisplay = true } }
    private func shownTitle(_ i: Int) -> String {
        rowTitleFor?(i) ?? (titles.indices.contains(i) ? titles[i] : "")
    }
    static let railWidth: CGFloat = 50
    var onCollapse: ((Bool) -> Void)?
    var collapseKey: String? {
        didSet {
            guard let k = collapseKey else { return }
            let on = UserDefaults.standard.bool(forKey: "sidebarRail." + k)
            if on != collapsed { collapsed = on }
        }
    }
    var collapsed = false {
        didSet {
            guard collapsed != oldValue else { return }
            if let k = collapseKey { UserDefaults.standard.set(collapsed, forKey: "sidebarRail." + k) }
            vScroll = 0
            needsDisplay = true
            window?.invalidateCursorRects(for: self)
            onCollapse?(collapsed)
            superview?.needsLayout = true
            if let w = window { DispatchQueue.main.async { PaneNav.shared.refreshSoon(w) } }
        }
    }
    func width(expanded: CGFloat) -> CGFloat { vertical && collapsed ? Self.railWidth : expanded }
    @discardableResult
    static func toggleRail(in window: NSWindow?) -> Bool {
        func find(_ v: NSView) -> PopupTabsBar? {
            if let b = v as? PopupTabsBar, b.vertical, !b.isHiddenOrHasHiddenAncestor, b.bounds.width > 0 { return b }
            for sub in v.subviews { if let f = find(sub) { return f } }
            return nil
        }
        guard let root = window?.contentView, let bar = find(root) else { return false }
        bar.collapsed.toggle()
        return true
    }
    var pinnedLabel: ((String) -> String)?
    var pinnedTip: ((String) -> String)?
    var rowIcon: ((Int) -> String?)?
    var onWidthChange: ((CGFloat, _ done: Bool) -> Void)?
    var minWidth: CGFloat = 140, maxWidth: CGFloat = 480
    private var vScroll: CGFloat = 0
    private var vRowH: CGFloat { 34 * zoom }
    private var vPinRowH: CGFloat { 30 * zoom }
    private var resizing: (startX: CGFloat, startW: CGFloat)?
    private var vCard: NSRect {
        NSRect(x: 8, y: 6, width: max(0, bounds.width - 8), height: max(0, bounds.height - 14))
    }
    private var vPinnedHeadRect: NSRect? {
        guard !pinned.isEmpty else { return nil }
        let c = vCard
        return NSRect(x: c.minX, y: c.minY + 6, width: c.width, height: 24 * zoom)
    }
    private func vPinnedLayout() -> (rows: [(rect: NSRect, path: String)], heads: [(rect: NSRect, title: String)]) {
        guard let h = vPinnedHeadRect else { return ([], []) }
        let c = vCard
        var rows: [(rect: NSRect, path: String)] = []
        var heads: [(rect: NSRect, title: String)] = []
        var y = h.maxY + 2
        var last: String?
        for p in pinned.prefix(maxPinnedShown) {
            let sec = pinnedSection?(p) ?? pinnedTitle
            if last == nil {
                heads.append((h, sec))
            } else if sec != last {
                y += 6
                heads.append((NSRect(x: c.minX, y: y, width: c.width, height: h.height), sec))
                y += h.height + 2
            }
            last = sec
            rows.append((NSRect(x: c.minX + 6, y: y, width: max(0, c.width - 12), height: vPinRowH), p))
            y += vPinRowH + 1
        }
        return (rows, heads)
    }
    private func vPinnedRows() -> [(rect: NSRect, path: String)] { vPinnedLayout().rows }
    private var vHeadRect: NSRect {
        let c = vCard
        let top = vPinnedRows().last.map { $0.rect.maxY + 10 } ?? c.minY + 6
        return NSRect(x: c.minX, y: top, width: c.width, height: 24 * zoom)
    }
    private var vAddRect: NSRect {
        let h = vHeadRect
        if collapsed { return NSRect(x: h.midX - 11 * zoom, y: h.midY - 11 * zoom, width: 22 * zoom, height: 22 * zoom) }
        return NSRect(x: h.maxX - 8 - 22 * zoom, y: h.midY - 11 * zoom, width: 22 * zoom, height: 22 * zoom)
    }
    private var vListRect: NSRect {
        let c = vCard, top = vHeadRect.maxY + 4
        return NSRect(x: c.minX + 6, y: top, width: max(0, c.width - 12), height: max(0, vToggleRect.minY - 4 - top))
    }
    private var vToggleRect: NSRect {
        let c = vCard, sz = 24 * zoom
        return NSRect(x: collapsed ? c.midX - sz / 2 : c.minX + 8, y: c.maxY - 6 - sz, width: sz, height: sz)
    }
    private var vResizeRect: NSRect { NSRect(x: bounds.maxX - 6, y: 0, width: 6, height: bounds.height) }
    private var vContentH: CGFloat { CGFloat(titles.count) * (vRowH + 1) }
    private func vRows() -> [(rect: NSRect, index: Int, close: NSRect?)] {
        let l = vListRect
        var out: [(NSRect, Int, NSRect?)] = []
        for i in titles.indices {
            let r = NSRect(x: l.minX, y: l.minY + CGFloat(i) * (vRowH + 1) - vScroll, width: l.width, height: vRowH)
            guard r.maxY > l.minY, r.minY < l.maxY else { continue }
            let close = NSRect(x: r.maxX - closeSize - 6 * zoom, y: r.midY - closeSize / 2,
                               width: closeSize, height: closeSize)
            out.append((r, i, closable && onCloseTab != nil ? close : nil))
        }
        return out
    }
    private func clampScroll() {
        vScroll = max(0, min(vScroll, vContentH - vListRect.height))
    }
    func revealSelected() {
        guard vertical, titles.indices.contains(selected) else { return }
        let l = vListRect
        let top = CGFloat(selected) * (vRowH + 1)
        if top < vScroll { vScroll = top }
        else if top + vRowH > vScroll + l.height { vScroll = top + vRowH - l.height }
        clampScroll()
        needsDisplay = true
    }
    private var keyFocusAllowed = false
    private(set) var keyFocused = false
    private(set) var cursor = 0
    override var acceptsFirstResponder: Bool { vertical && keyFocusAllowed }
    override func becomeFirstResponder() -> Bool {
        keyFocused = true
        cursor = currentRow()
        revealCursor()
        needsDisplay = true
        return true
    }
    override func resignFirstResponder() -> Bool {
        keyFocused = false
        keyFocusAllowed = false
        needsDisplay = true
        return true
    }
    func takeKeyboardFocus() {
        guard vertical, let w = window else { return }
        keyFocusAllowed = true
        if !w.makeFirstResponder(self) { keyFocusAllowed = false }
    }
    private var pinnedShown: Int { min(pinned.count, maxPinnedShown) }
    private var rowCount: Int { pinnedShown + titles.count }
    private func currentRow() -> Int {
        if titles.indices.contains(selected) { return pinnedShown + selected }
        if let p = pinnedSelected, let k = pinned.prefix(maxPinnedShown).firstIndex(of: p) { return k }
        return min(cursor, max(0, rowCount - 1))
    }
    func jumpItems() -> [PopupWindow.SidebarJumpItem] {
        var out: [PopupWindow.SidebarJumpItem] = []
        for row in 0..<rowCount {
            if row < pinnedShown {
                let p = pinned[row]
                out.append(.init(section: pinnedSection?(p) ?? pinnedTitle, title: rowTitle(row),
                                 icon: pinnedIconFor?(p) ?? pinnedIcon, row: row))
            } else {
                out.append(.init(section: sectionTitle, title: rowTitle(row),
                                 icon: rowIcon?(row - pinnedShown), row: row))
            }
        }
        return out
    }
    private func rowTitle(_ row: Int) -> String {
        if row < pinnedShown {
            let p = pinned[row]
            return pinnedLabel?(p) ?? (p as NSString).lastPathComponent
        }
        let i = row - pinnedShown
        return shownTitle(i)
    }
    private func revealCursor() {
        guard cursor >= pinnedShown else { needsDisplay = true; return }
        let i = cursor - pinnedShown, l = vListRect
        let top = CGFloat(i) * (vRowH + 1)
        if top < vScroll { vScroll = top }
        else if top + vRowH > vScroll + l.height { vScroll = top + vRowH - l.height }
        clampScroll()
        needsDisplay = true
    }
    private func moveCursor(to row: Int) {
        guard rowCount > 0 else { return }
        cursor = max(0, min(row, rowCount - 1))
        revealCursor()
    }
    func activate(row: Int) {
        if row < pinnedShown {
            onPinned?(pinned[row])
            return
        }
        let i = row - pinnedShown
        guard titles.indices.contains(i) else { return }
        onClick?(i)
        if i != selected {
            selected = i
            onSelect?(i)
        }
    }
    func handleNavKey(_ e: NSEvent) -> Bool {
        guard keyFocused, e.type == .keyDown else { return false }
        let mods = e.modifierFlags.intersection([.command, .control, .option, .shift])
        let page = max(1, Int(vListRect.height / (vRowH + 1)) - 1)
        switch (e.keyCode, mods) {
        case (125, []), (45, .control): moveCursor(to: cursor + 1)
        case (126, []), (35, .control): moveCursor(to: cursor - 1)
        case (115, []), (126, .command): moveCursor(to: 0)
        case (119, []), (125, .command): moveCursor(to: rowCount - 1)
        case (116, []): moveCursor(to: cursor - page)
        case (121, []): moveCursor(to: cursor + page)
        case (36, []), (76, []):
            let w = window
            activate(row: cursor)
            if let w, w.firstResponder === self { _ = PaneNav.shared.move(.right, in: w) }
        case (49, []): activate(row: cursor)
        case (53, []):
            if let w = window { _ = PaneNav.shared.move(.right, in: w) }
        case (51, []), (117, []), (51, .command):
            let i = cursor - pinnedShown
            guard closable, let close = onCloseTab, titles.indices.contains(i) else { return true }
            close(i)
            moveCursor(to: cursor)
        default:
            guard mods.isDisjoint(with: [.command, .control, .option]), e.keyCode != 48 else { return false }
            guard let ch = e.charactersIgnoringModifiers?.lowercased().first, ch.isLetter || ch.isNumber,
                  rowCount > 0 else { return true }
            for step in 1...rowCount {
                let r = (cursor + step) % rowCount
                if rowTitle(r).lowercased().first == ch { moveCursor(to: r); break }
            }
        }
        return true
    }

    override func scrollWheel(with event: NSEvent) {
        guard vertical else { return super.scrollWheel(with: event) }
        let dy = event.hasPreciseScrollingDeltas ? event.scrollingDeltaY : event.scrollingDeltaY * 10
        vScroll -= dy
        clampScroll()
        needsDisplay = true
        VimKeys.shared.repaintHighlight()
    }
    override func resetCursorRects() {
        super.resetCursorRects()
        if vertical, onWidthChange != nil, !collapsed { addCursorRect(vResizeRect, cursor: .resizeLeftRight) }
    }
    private var mtimeCache: [String: (text: String, at: Date)] = [:]
    private func vMeta(_ i: Int) -> String {
        guard let p = pathTip?(i), !p.isEmpty else { return "" }
        if let hit = mtimeCache[p], Date().timeIntervalSince(hit.at) < 5 { return hit.text }
        let full = (p as NSString).expandingTildeInPath
        var text = ""
        if let d = (try? FileManager.default.attributesOfItem(atPath: full))?[.modificationDate] as? Date {
            let f = DateFormatter()
            f.dateFormat = Calendar.current.isDateInToday(d) ? "HH:mm" : "MMM d"
            text = f.string(from: d)
        }
        mtimeCache[p] = (text, Date())
        return text
    }
    private func vIcon(_ i: Int) -> String {
        if let s = rowIcon?(i) { return s }
        switch (titles[i] as NSString).pathExtension.lowercased() {
        case "md", "markdown", "txt", "": return "doc.text"
        case "json", "toml", "yaml", "yml", "cfg", "conf", "ini", "plist": return "gearshape"
        case "log": return "list.bullet.rectangle"
        case "swift", "py", "js", "ts", "sh", "rb", "go", "rs", "c", "h": return "chevron.left.forwardslash.chevron.right"
        default: return "doc"
        }
    }
    private func vSectionLabel(_ text: String, in head: NSRect) {
        let labAttrs: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: 10 * zoom, weight: .semibold),
            .foregroundColor: config.colors.dim, .kern: 0.8,
        ]
        let lab = text.uppercased() as NSString
        let lsz = lab.size(withAttributes: labAttrs)
        lab.draw(at: NSPoint(x: head.minX + 14, y: head.midY - lsz.height / 2), withAttributes: labAttrs)
    }
    private func drawCursor(_ r: NSRect, radius: CGFloat) {
        let path = NSBezierPath(roundedRect: r.insetBy(dx: 0.5, dy: 0.5), xRadius: radius, yRadius: radius)
        path.lineWidth = 1
        PaneNav.ringColor.withAlphaComponent(min(1, PaneNav.ringColor.alphaComponent + 0.25)).setStroke()
        path.stroke()
    }

    private func drawVertical() {
        let c = config.colors
        let card = vCard
        guard card.width > 0, card.height > 0 else { return }
        c.mantle.setFill()
        NSBezierPath(roundedRect: card, xRadius: 12 * zoom, yRadius: 12 * zoom).fill()
        let radius = config.buttonRadius * zoom
        let para = NSMutableParagraphStyle()
        para.lineBreakMode = .byTruncatingMiddle
        if vPinnedHeadRect != nil {
            let layout = vPinnedLayout()
            for (ph, title) in layout.heads {
                if collapsed { vRailRule(in: ph) } else { vSectionLabel(title, in: ph) }
            }
            for (k, (r, path)) in layout.rows.enumerated() {
                let sel = pinnedSelected == path
                if sel || hoverIndex == -10 - k {
                    (sel ? c.surface1 : c.surface0).setFill()
                    NSBezierPath(roundedRect: r, xRadius: radius, yRadius: radius).fill()
                }
                if keyFocused && cursor == k { drawCursor(r, radius: radius) }
                let iconX = collapsed ? r.midX - 8 * zoom : r.minX + 8 * zoom
                ButtonStyle.symbol(pinnedIconFor?(path) ?? pinnedIcon, in: NSRect(x: iconX, y: r.minY, width: 16 * zoom, height: r.height),
                                   color: sel ? c.accentOn : c.dim, size: (collapsed ? 13 : 11) * zoom)
                if collapsed { continue }
                let name = (pinnedLabel?(path) ?? (path as NSString).abbreviatingWithTildeInPath) as NSString
                let a: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 12.5 * zoom, weight: sel ? .semibold : .regular),
                                                        .foregroundColor: c.text, .paragraphStyle: para]
                let sz = name.size(withAttributes: a)
                var tagW: CGFloat = 0
                if let tag = pinnedMeta?(path), !tag.isEmpty {
                    let ta: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 10 * zoom), .foregroundColor: c.dim]
                    let tsz = (tag as NSString).size(withAttributes: ta)
                    (tag as NSString).draw(at: NSPoint(x: r.maxX - 8 * zoom - tsz.width, y: r.midY - tsz.height / 2), withAttributes: ta)
                    tagW = tsz.width + 8 * zoom
                }
                name.draw(in: NSRect(x: r.minX + 30 * zoom, y: r.midY - sz.height / 2,
                                     width: max(0, r.width - 38 * zoom - tagW), height: sz.height), withAttributes: a)
            }
        }
        let head = vHeadRect
        if collapsed { vRailRule(in: head) } else { vSectionLabel(sectionTitle, in: head) }
        if config.tabsAddButton {
            let r = vAddRect
            let st: ButtonState = pressedIndex == -2 ? .pressed : hoverIndex == -2 ? .hover : .idle
            ButtonStyle.draw(r, st, c, radius: radius, flat: true)
            ButtonStyle.plus(in: r, color: ButtonStyle.text(st, c), arm: 4.5 * zoom)
        }
        let list = vListRect
        NSGraphicsContext.saveGraphicsState()
        NSBezierPath(rect: list).addClip()
        for (r, i, close) in vRows() {
            let sel = i == selected, hov = hoverIndex == i
            if sel || hov {
                (sel ? c.surface1 : c.surface0).setFill()
                NSBezierPath(roundedRect: r, xRadius: radius, yRadius: radius).fill()
            }
            if keyFocused && cursor == pinnedShown + i { drawCursor(r, radius: radius) }
            if collapsed {
                ButtonStyle.symbol(vIcon(i), in: NSRect(x: r.midX - 8 * zoom, y: r.minY, width: 16 * zoom, height: r.height),
                                   color: sel ? c.accentOn : c.dim, size: 13 * zoom)
                if let b = badge(i), !(quietOKBadges && b.tone == .success) {
                    let dot = NSRect(x: r.midX + 5 * zoom, y: r.midY - 9 * zoom, width: dotW, height: dotW)
                    c.tone(b.tone).setFill()
                    NSBezierPath(ovalIn: dot).fill()
                }
                continue
            }
            ButtonStyle.symbol(vIcon(i), in: NSRect(x: r.minX + 8 * zoom, y: r.minY, width: 16 * zoom, height: r.height),
                               color: sel ? c.accentOn : c.dim, size: 11 * zoom)
            let showClose = close != nil && hov
            let b = badge(i)
            let quiet = quietOKBadges && b?.tone == .success
            let meta = showClose || quiet ? "" : (b.map { $0.text } ?? vMeta(i))
            let metaAttrs: [NSAttributedString.Key: Any] = [
                .font: NSFont.monospacedDigitSystemFont(ofSize: 10 * zoom, weight: .regular),
                .foregroundColor: c.dim,
            ]
            let msz = (meta as NSString).size(withAttributes: metaAttrs)
            var right = showClose ? close!.minX - 4 : r.maxX - 8 * zoom - (meta.isEmpty ? 0 : msz.width + 8 * zoom)
            if !meta.isEmpty {
                (meta as NSString).draw(at: NSPoint(x: r.maxX - 8 * zoom - msz.width, y: r.midY - msz.height / 2),
                                        withAttributes: metaAttrs)
            }
            if let b, !showClose, !quiet {
                let dot = NSRect(x: right - dotW, y: r.midY - dotW / 2, width: dotW, height: dotW)
                c.tone(b.tone).setFill()
                NSBezierPath(ovalIn: dot).fill()
                right = dot.minX - 6 * zoom
            }
            let tAttrs: [NSAttributedString.Key: Any] = [
                .font: NSFont.systemFont(ofSize: 12.5 * zoom, weight: sel ? .semibold : .regular),
                .foregroundColor: c.text, .paragraphStyle: para,
            ]
            let tx = r.minX + 30 * zoom
            let shown = shownTitle(i) as NSString
            let tsz = shown.size(withAttributes: tAttrs)
            shown.draw(in: NSRect(x: tx, y: r.midY - tsz.height / 2,
                                                    width: max(0, right - tx), height: tsz.height),
                                         withAttributes: tAttrs)
            if showClose, let close {
                if hoverCloseIndex == i {
                    c.tone(.danger).withAlphaComponent(0.22).setFill()
                    NSBezierPath(ovalIn: close).fill()
                }
                ButtonStyle.cross(in: close, color: hoverCloseIndex == i ? c.tone(.danger) : c.dim, arm: 3 * zoom)
            }
        }
        NSGraphicsContext.restoreGraphicsState()
        do {
            let r = vToggleRect
            let st: ButtonState = hoverIndex == -4 ? .hover : .idle
            ButtonStyle.draw(r, st, c, radius: radius, flat: true)
            ButtonStyle.symbol(collapsed ? "sidebar.right" : "sidebar.left", in: r,
                               color: hoverIndex == -4 ? c.text : c.dim, size: 12 * zoom)
        }
        if let st = statusLine, !collapsed {
            let r = vToggleRect
            let dot = NSRect(x: r.maxX + 8 * zoom, y: r.midY - dotW / 2, width: dotW, height: dotW)
            c.tone(st.tone).setFill()
            NSBezierPath(ovalIn: dot).fill()
            let sa: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 10.5 * zoom), .foregroundColor: c.dim,
                                                     .paragraphStyle: para]
            let ssz = (st.text as NSString).size(withAttributes: sa)
            let tx = dot.maxX + 6 * zoom
            (st.text as NSString).draw(in: NSRect(x: tx, y: r.midY - ssz.height / 2,
                                                  width: max(0, card.maxX - 10 * zoom - tx), height: ssz.height),
                                       withAttributes: sa)
        }
        if vContentH > list.height + 1 {
            let frac = list.height / vContentH
            let h = max(20, list.height * frac)
            let y = list.minY + (list.height - h) * (vScroll / max(1, vContentH - list.height))
            c.dim.withAlphaComponent(0.35).setFill()
            NSBezierPath(roundedRect: NSRect(x: card.maxX - 4, y: y, width: 2.5, height: h),
                         xRadius: 1.25, yRadius: 1.25).fill()
        }
        if onWidthChange != nil, !collapsed, resizing != nil || hoverIndex == -3 {
            c.accentOn.withAlphaComponent(0.6).setFill()
            NSBezierPath(roundedRect: NSRect(x: card.maxX - 2, y: card.minY + 12, width: 2, height: card.height - 24),
                         xRadius: 1, yRadius: 1).fill()
        }
    }
    private func vHit(_ p: NSPoint) -> (index: Int, close: Bool)? {
        if vToggleRect.contains(p) { return (-4, false) }
        if onWidthChange != nil, !collapsed, vResizeRect.contains(p) { return (-3, false) }
        if config.tabsAddButton, vAddRect.contains(p) { return (-2, false) }
        for (k, (r, _)) in vPinnedRows().enumerated() where r.contains(p) { return (-10 - k, false) }
        guard vListRect.contains(p) else { return nil }
        for (r, i, close) in vRows() where r.contains(p) {
            if collapsed { return (i, false) }
            return (i, close.map { $0.insetBy(dx: -2, dy: -2).contains(p) } ?? false)
        }
        return nil
    }
    private func vRailRule(in head: NSRect) {
        config.colors.dim.withAlphaComponent(0.25).setFill()
        NSRect(x: head.midX - 10 * zoom, y: head.midY, width: 20 * zoom, height: 1).fill()
    }
    override func mouseDragged(with event: NSEvent) {
        guard vertical, let rs = resizing else { return super.mouseDragged(with: event) }
        let x = convert(event.locationInWindow, from: nil).x
        let w = max(minWidth, min(maxWidth, rs.startW + (x - rs.startX)))
        onWidthChange?(w, false)
        needsDisplay = true
    }
    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        if vertical { clampScroll(); window?.invalidateCursorRects(for: self) }
    }
    private var tabH: CGFloat { 22 * zoom }
    private var vpad: CGFloat { 4 * zoom }
    private let gap: CGFloat = 6
    private var addW: CGFloat { 24 * zoom }
    private var closeSize: CGFloat { 16 * zoom }
    private var hoverIndex: Int?
    private var hoverCloseIndex: Int?
    private var pressedIndex: Int?
    private var trackingArea: NSTrackingArea?
    var fill: NSColor? {
        didSet { needsDisplay = true }
    }

    override var isFlipped: Bool { true }

    init(config: PopupConfig) {
        self.config = config
        super.init(frame: .zero)
        if config.opaqueTabs { fill = config.colors.background.withAlphaComponent(1) }
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) not supported") }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let t = trackingArea { removeTrackingArea(t) }
        let t = NSTrackingArea(rect: bounds,
                               options: [.mouseMoved, .mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect],
                               owner: self, userInfo: nil)
        addTrackingArea(t)
        trackingArea = t
    }
    override func mouseEntered(with event: NSEvent) {}
    override func mouseExited(with event: NSEvent) {
        if hoverCloseIndex != nil || hoverIndex != nil {
            hoverCloseIndex = nil
            hoverIndex = nil
            needsDisplay = true
        }
    }
    override func mouseMoved(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        if vertical {
            let hit = vHit(p)
            let over = hit?.index, overClose = hit?.close == true ? hit?.index : nil
            let tip: String? = over.flatMap { i in
                if i >= 0 {
                    if collapsed { return [shownTitle(i), badge(i)?.tip].compactMap { $0 }.joined(separator: " · ") }
                    return badge(i)?.tip ?? pathTip?(i)
                }
                if i == -2 { return "New" }
                if i == -3 { return "Drag to resize" }
                if i == -4 { return collapsed ? "Show the sidebar  ⌘\\" : "Collapse the sidebar  ⌘\\" }
                let k = -10 - i
                guard pinned.indices.contains(k) else { return nil }
                if collapsed, let l = pinnedLabel?(pinned[k]) { return l }
                return pinnedTip?(pinned[k]) ?? "Open \((pinned[k] as NSString).abbreviatingWithTildeInPath) in the file browser"
            }
            if toolTip != tip { toolTip = tip }
            if over != hoverIndex || overClose != hoverCloseIndex {
                hoverIndex = over
                hoverCloseIndex = overClose
                needsDisplay = true
            }
            return
        }
        var over: Int?
        var overClose: Int?
        for (i, (rect, title, close)) in pillRects().enumerated() where rect.contains(p) {
            over = i
            if title != "+", let close, close.insetBy(dx: -2, dy: -2).contains(p) { overClose = i }
            break
        }
        let tip = over.flatMap { i -> String? in
            let rects = pillRects()
            let t = rects[i].title
            if t == "+" { return nil }
            let idx = i - (config.tabsAddButton ? 1 : 0)
            let b = titles.firstIndex(of: t).flatMap { badge($0)?.tip }
            return b ?? pathTip?(idx)
        }
        if toolTip != tip { toolTip = tip }
        if over != hoverIndex || overClose != hoverCloseIndex {
            hoverIndex = over
            hoverCloseIndex = overClose
            needsDisplay = true
        }
    }

    private func tabWidth(_ title: String, _ index: Int = -1) -> CGFloat {
        let attrs: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: config.buttonFontSize * zoom, weight: .bold),
        ]
        return (title as NSString).size(withAttributes: attrs).width + 12 * zoom + 22 * zoom
            + badgeWidth(index)
    }

    private func rowCount(forWidth width: CGFloat) -> Int {
        var x: CGFloat = config.padding + 4 + (config.tabsAddButton ? addW + gap : 0)
        var rows = 1
        for (i, t) in titles.enumerated() {
            let tw = tabWidth(t, i) + gap
            if x + tw > width - config.padding {
                rows += 1
                x = config.padding + 4 + tw
            } else {
                x += tw
            }
        }
        return max(1, rows)
    }

    func heightNeeded(forWidth width: CGFloat) -> CGFloat {
        if vertical { return frame.height }
        return CGFloat(rowCount(forWidth: width)) * (tabH + gap) - gap + vpad * 2
    }

    private func pillRects() -> [(rect: NSRect, title: String, close: NSRect?)] {
        var out: [(NSRect, String, NSRect?)] = []
        var x: CGFloat = config.padding + 4
        var y: CGFloat = vpad
        if config.tabsAddButton {
            out.append((NSRect(x: x, y: y, width: addW, height: tabH), "+", nil))
            x += addW + gap
        }
        for (i, t) in titles.enumerated() {
            let tw = tabWidth(t, i)
            if x + tw + gap > bounds.width - config.padding {
                y += tabH + gap
                x = config.padding + 4
            }
            let rect = NSRect(x: x, y: y, width: tw, height: tabH)
            let close = NSRect(x: rect.maxX - closeSize - 4 * zoom,
                               y: rect.midY - closeSize / 2,
                               width: closeSize, height: closeSize)
            out.append((rect, t, closable ? close : nil))
            x += tw + gap
        }
        return out
    }

    override func draw(_ dirtyRect: NSRect) {
        if vertical { return drawVertical() }
        let c = config.colors
        let radius = config.buttonRadius * zoom
        if fill != nil {
            c.mantle.setFill()
            bounds.fill()
        } else {
            c.mantle.withAlphaComponent(0.45).setFill()
            bounds.fill()
        }
        for (i, (rect, title, close)) in pillRects().enumerated() {
            let isSelectedTab = title != "+" && titles.firstIndex(of: title) == selected
            let hovered = hoverIndex == i
            let st: ButtonState = pressedIndex == i ? .pressed
                : isSelectedTab ? (hovered ? .onHover : .on)
                : hovered ? .hover : .idle
            let fg: NSColor
            if isSelectedTab {
                if hovered {
                    c.surface0.setFill()
                    NSBezierPath(roundedRect: rect.insetBy(dx: 0.5, dy: 0.5), xRadius: radius, yRadius: radius).fill()
                }
                let u = NSRect(x: rect.minX + 6 * zoom, y: rect.maxY - 2, width: rect.width - 12 * zoom, height: 2)
                c.accentOn.setFill()
                NSBezierPath(roundedRect: u, xRadius: 1, yRadius: 1).fill()
                fg = c.text
            } else {
                ButtonStyle.draw(rect, st, c, radius: radius, flat: true)
                fg = ButtonStyle.text(st, c)
            }
            if title == "+" {
                ButtonStyle.plus(in: rect, color: fg, arm: 4.5 * zoom)
                continue
            }
            let labelRect = NSRect(x: rect.minX + 6 * zoom, y: rect.minY,
                                   width: rect.width - 6 * zoom - 22 * zoom, height: rect.height)
            let attrs: [NSAttributedString.Key: Any] = [
                .font: NSFont.systemFont(ofSize: config.buttonFontSize * zoom,
                                         weight: isSelectedTab ? .bold : .medium),
                .foregroundColor: fg,
            ]
            let ts = title as NSString
            let tsz = ts.size(withAttributes: attrs)
            let ti = titles.firstIndex(of: title) ?? -1
            if let b = badge(ti) {
                let battrs: [NSAttributedString.Key: Any] = [
                    .font: badgeFont, .foregroundColor: fg.withAlphaComponent(isSelectedTab ? 0.75 : 0.65)]
                let bs = b.text as NSString
                let bsz = b.text.isEmpty ? .zero : bs.size(withAttributes: battrs)
                let groupW = dotW + 5 * zoom + tsz.width + (b.text.isEmpty ? 0 : 5 * zoom + bsz.width)
                var x = labelRect.midX - groupW / 2
                let dot = NSRect(x: x, y: labelRect.midY - dotW / 2, width: dotW, height: dotW)
                c.tone(b.tone).setFill()
                NSBezierPath(ovalIn: dot).fill()
                x += dotW + 5 * zoom
                ts.draw(at: NSPoint(x: x, y: labelRect.midY - tsz.height / 2), withAttributes: attrs)
                x += tsz.width + 5 * zoom
                if !b.text.isEmpty {
                    bs.draw(at: NSPoint(x: x, y: labelRect.midY - bsz.height / 2), withAttributes: battrs)
                }
            } else {
                ts.draw(at: NSPoint(x: labelRect.midX - tsz.width / 2, y: labelRect.midY - tsz.height / 2),
                        withAttributes: attrs)
            }
            if let close, isSelectedTab || hovered {
                if hoverCloseIndex == i {
                    c.tone(.danger).withAlphaComponent(0.22).setFill()
                    NSBezierPath(ovalIn: close).fill()
                }
                let xc = hoverCloseIndex == i ? c.tone(.danger) : c.dim.withAlphaComponent(0.8)
                ButtonStyle.cross(in: close, color: xc, arm: 3 * zoom)
            }
        }
    }

    override func mouseUp(with event: NSEvent) {
        if resizing != nil {
            resizing = nil
            onWidthChange?(bounds.width, true)
            needsDisplay = true
        }
        if pressedIndex != nil { pressedIndex = nil; needsDisplay = true }
        super.mouseUp(with: event)
    }

    override func mouseDown(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        if vertical {
            guard let hit = vHit(p) else { return }
            if hit.index == -4 {
                collapsed.toggle()
            } else if hit.index == -3 {
                resizing = (convert(event.locationInWindow, from: nil).x, bounds.width)
                needsDisplay = true
            } else if hit.index <= -10 {
                let k = -10 - hit.index
                if pinned.indices.contains(k) { onPinned?(pinned[k]) }
            } else if hit.index == -2 {
                pressedIndex = -2
                needsDisplay = true
                onAddTab?()
            } else if hit.close {
                onCloseTab?(hit.index)
            } else {
                onClick?(hit.index)
                if hit.index != selected {
                    selected = hit.index
                    onSelect?(hit.index)
                }
            }
            return
        }
        for (i, (rect, title, close)) in pillRects().enumerated() where rect.contains(p) {
            pressedIndex = i
            needsDisplay = true
            if title == "+" {
                onAddTab?()
            } else if let close, close.insetBy(dx: -2, dy: -2).contains(p),
                      let i = titles.firstIndex(of: title) {
                pressedIndex = nil
                onCloseTab?(i)
            } else if let i = titles.firstIndex(of: title) {
                onClick?(i)
                if i != selected {
                    selected = i
                    onSelect?(i)
                }
            }
            return
        }
        super.mouseDown(with: event)
    }

    override func rightMouseDown(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        var target: Int?
        if vertical {
            if let hit = vHit(p), hit.index <= -10, pinned.indices.contains(-10 - hit.index),
               let menu = pinnedMenu?(pinned[-10 - hit.index]) {
                NSMenu.popUpContextMenu(menu, with: event, for: self)
                return
            }
            if let hit = vHit(p), hit.index >= 0 { target = hit.index }
        } else {
            for (rect, title, _) in pillRects() where rect.contains(p) {
                if title != "+" { target = titles.firstIndex(of: title) }
                break
            }
        }
        guard let i = target else { return super.rightMouseDown(with: event) }
        if let menuFor, let menu = menuFor(i) {
            NSMenu.popUpContextMenu(menu, with: event, for: self)
            return
        }
        let menu = NSMenu()
        func add(_ title: String, _ sel: Selector) {
            let item = NSMenuItem(title: title, action: sel, keyEquivalent: "")
            item.target = self
            item.representedObject = i
            menu.addItem(item)
        }
        add("Copy Path", #selector(copyPath(_:)))
        if let path = pathTip?(i), !path.isEmpty { add("Reveal in Finder", #selector(revealPath(_:))) }
        if closable, onCloseTab != nil {
            menu.addItem(.separator())
            add("Close", #selector(closeTabItem(_:)))
        }
        NSMenu.popUpContextMenu(menu, with: event, for: self)
    }
    @objc private func copyPath(_ sender: NSMenuItem) {
        guard let i = sender.representedObject as? Int else { return }
        onCopyPath?(i)
    }
    @objc private func revealPath(_ sender: NSMenuItem) {
        guard let i = sender.representedObject as? Int, let p = pathTip?(i) else { return }
        NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: (p as NSString).expandingTildeInPath)])
    }
    @objc private func closeTabItem(_ sender: NSMenuItem) {
        guard let i = sender.representedObject as? Int else { return }
        onCloseTab?(i)
    }
}

final class PopupFilterBar: NSView {
    var config: PopupConfig
    var zoom: CGFloat = 1.0
    var labels: [String] = []
    var values: [[String]] = []
    var valueLabels: [[String]] = []
    var selections: [Int] = []
    var onSelect: ((Int, Int) -> Void)?
    var onOpen: ((Int, NSRect) -> Void)?
    var summaries: [String] = []
    var active: Set<Int> = []
    private var pillH: CGFloat { 22 * zoom }
    private let sepW: CGFloat = 1
    private var segPad: CGFloat { 12 * zoom }
    private var flashDim = -1

    override var isFlipped: Bool { true }

    init(config: PopupConfig) {
        self.config = config
        super.init(frame: .zero)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) not supported") }

    private var fontAttrs: [NSAttributedString.Key: Any] {
        [.font: NSFont.systemFont(ofSize: config.buttonFontSize * zoom, weight: .semibold)]
    }

    private func optionTitle(_ dim: Int, _ vi: Int) -> String {
        let v = values.indices.contains(dim) && values[dim].indices.contains(vi)
            ? values[dim][vi] : "All"
        if valueLabels.indices.contains(dim) && valueLabels[dim].indices.contains(vi) {
            let l = valueLabels[dim][vi]
            if !l.isEmpty { return l }
        }
        return v
    }

    private func currentTitle(_ dim: Int) -> String {
        let label = labels.indices.contains(dim) ? labels[dim] : "?"
        if summaries.indices.contains(dim), !summaries[dim].isEmpty { return "\(label): \(summaries[dim])" }
        let sel = selections.indices.contains(dim) ? selections[dim] : 0
        return "\(label): \(optionTitle(dim, sel))"
    }

    func naturalWidth() -> CGFloat {
        var w: CGFloat = 0
        for i in 0..<labels.count {
            w += (currentTitle(i) as NSString).size(withAttributes: fontAttrs).width + segPad * 2 + 12
        }
        w += CGFloat(max(0, labels.count - 1)) * sepW
        return w + 2 * barInset
    }

    private var barInset: CGFloat { config.padding + 10 }

    private func pillRects() -> [(NSRect, Int, String)] {
        let inset = barInset
        var x = inset
        var out: [(NSRect, Int, String)] = []
        for (i, t) in (0..<labels.count).map({ currentTitle($0) }).enumerated() {
            let w = (t as NSString).size(withAttributes: fontAttrs).width + segPad * 2 + 12
            out.append((NSRect(x: x, y: (bounds.height - pillH) / 2,
                               width: w, height: pillH), i, t))
            x += w + sepW
        }
        return out
    }

    override func draw(_ dirtyRect: NSRect) {
        for (rect, dim, title) in pillRects() {
            let active = self.active.contains(dim)
                || (selections.indices.contains(dim) && selections[dim] > 0)
            let st: ButtonState = flashDim == dim ? .pressed : active ? .on : .idle
            let r = rect.insetBy(dx: 1, dy: 0)
            ButtonStyle.draw(r, st, config.colors, radius: config.buttonRadius * zoom)
            let size = config.buttonFontSize * zoom
            let attrs: [NSAttributedString.Key: Any] = [
                .font: ButtonStyle.font(size, st),
                .foregroundColor: ButtonStyle.text(st, config.colors),
            ]
            let s = title as NSString
            let sz = s.size(withAttributes: attrs)
            let x = r.midX - (sz.width + 12) / 2
            s.draw(at: NSPoint(x: x, y: r.midY - sz.height / 2), withAttributes: attrs)
            ButtonStyle.chevron(in: NSRect(x: x + sz.width + 4, y: r.midY - 4, width: 8, height: 8),
                                color: ButtonStyle.text(st, config.colors))
        }
    }

    override func mouseDown(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        for (rect, dim, _) in pillRects() where rect.contains(p) {
            if let open = onOpen {
                open(dim, rect)
                return
            }
            let menu = NSMenu()
            let opts = values.indices.contains(dim) ? values[dim] : ["All"]
            for (vi, _) in opts.enumerated() {
                let item = NSMenuItem(title: optionTitle(dim, vi),
                                      action: #selector(pick(_:)), keyEquivalent: "")
                item.target = self
                item.tag = dim * 10000 + vi
                item.state = (selections.indices.contains(dim) && selections[dim] == vi)
                    ? .on : .off
                menu.addItem(item)
            }
            if let win = window {
                let screenP = win.convertPoint(toScreen: convert(rect.origin, to: nil))
                menu.popUp(positioning: nil,
                           at: NSPoint(x: screenP.x, y: screenP.y - pillH), in: nil)
            } else {
                menu.popUp(positioning: nil, at: NSPoint(x: rect.minX, y: rect.minY), in: self)
            }
            return
        }
        super.mouseDown(with: event)
    }

    @objc private func pick(_ sender: NSMenuItem) {
        onSelect?(sender.tag / 10000, sender.tag % 10000)
        let dim = sender.tag / 10000
        flashDim = dim
        needsDisplay = true
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) { [weak self] in
            self?.flashDim = -1
            self?.needsDisplay = true
        }
    }
}

final class PopupSearchFieldCell: NSTextFieldCell {
    var hInset: CGFloat = 0
    private func centeredTextRect(in r: NSRect) -> NSRect {
        let h = cellSize.height
        return NSRect(x: r.minX + hInset, y: r.midY - h / 2,
                      width: max(0, r.width - hInset * 2), height: h)
    }

    override func titleRect(forBounds bounds: NSRect) -> NSRect {
        centeredTextRect(in: super.titleRect(forBounds: bounds))
    }

    override func drawingRect(forBounds bounds: NSRect) -> NSRect {
        centeredTextRect(in: super.drawingRect(forBounds: bounds))
    }

    override func edit(withFrame aRect: NSRect, in controlView: NSView,
                       editor textObj: NSText, delegate: Any?, event: NSEvent?) {
        super.edit(withFrame: centeredTextRect(in: aRect), in: controlView,
                   editor: textObj, delegate: delegate, event: event)
    }

    override func select(withFrame aRect: NSRect, in controlView: NSView,
                         editor textObj: NSText, delegate: Any?,
                         start selStart: Int, length selLength: Int) {
        super.select(withFrame: centeredTextRect(in: aRect), in: controlView,
                     editor: textObj, delegate: delegate,
                     start: selStart, length: selLength)
    }

    override func drawInterior(withFrame cellFrame: NSRect, in controlView: NSView) {
        if stringValue.isEmpty {
            if (controlView as? NSControl)?.currentEditor() == nil,
               let ph = placeholderAttributedString {
                let sz = ph.size()
                ph.draw(at: NSPoint(x: cellFrame.midX - sz.width / 2,
                                    y: cellFrame.midY - sz.height / 2))
            }
            return
        }
        super.drawInterior(withFrame: cellFrame, in: controlView)
    }
}

final class PopupRowView: NSView {
    var config: PopupConfig
    var zoom: CGFloat = 1.0 {
        didSet { invalidateHeightCache(); needsDisplay = true }
    }
    var rows: [PopupRow] = [] {
        didSet { invalidateHeightCache() }
    }
    var selection = 0
    var onDrawRow: ((NSRect, PopupRow, Bool) -> Void)?
    var onRowClick: ((Int) -> Void)?
    var onRowDoubleClick: ((Int) -> Void)?
    private var downIndex = -1
    private var downPoint = NSPoint.zero
    var topInset: CGFloat = 0
    var bottomInset: CGFloat = 0
    var highlightQuery = ""
    var sizingRowCount = 0
    var stretchToFill = false
    var selected: Set<Int> = []
    var onToggleSelect: ((Int) -> Void)?
    var onToggleStar: ((Int) -> Void)?

    override var isFlipped: Bool { true }

    init(config: PopupConfig) {
        self.config = config
        super.init(frame: .zero)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) not supported") }

    private var contentX: CGFloat { config.rowLeadInset }

    static func tableFrames(_ config: PopupConfig, width: CGFloat) -> [(x: CGFloat, w: CGFloat)] {
        let x0 = config.rowLeadInset
        return config.tableColumns.frames(x0: x0, usable: width - x0 - (config.padding + 10))
    }

    private func checkBoxRect(in band: NSRect) -> NSRect {
        let s: CGFloat = 13
        return NSRect(x: config.padding + 2, y: band.midY - s / 2, width: s, height: s)
    }

    private func starRect(in band: NSRect) -> NSRect {
        let s: CGFloat = 14
        return NSRect(x: config.padding + 2 + (config.selectableRows ? 22 : 0) - 1,
                      y: band.midY - s / 2, width: s, height: s)
    }

    private func band(for index: Int) -> NSRect {
        let r = rect(for: index)
        let natural = heights(forWidth: bounds.width)[index]
        let h = min(r.height, natural)
        return NSRect(x: r.minX, y: r.minY + (r.height - h) / 2,
                      width: r.width, height: h)
    }

    override func draw(_ dirtyRect: NSRect) {
        let extra = stretchExtra()
        let hs = heights(forWidth: bounds.width)
        var y: CGFloat = topInset
        for (i, row) in rows.enumerated() {
            let h = hs[i] + extra
            if y >= dirtyRect.maxY { break }
            if y + h > dirtyRect.minY {
                let rect = NSRect(x: 0, y: y, width: bounds.width, height: h)
                if let onDrawRow {
                    onDrawRow(rect, row, i == selection)
                } else {
                    drawDefault(row, in: rect, natural: hs[i], index: i,
                                isSel: i == selection)
                }
            }
            y += h
        }
    }

    private func stretchExtra() -> CGFloat {
        guard stretchToFill, !config.scrollableRows,
              rows.count == sizingRowCount, rows.count > 0 else { return 0 }
        let hs = heights(forWidth: bounds.width)
        let minTotal = topInset + hs.reduce(0, +)
        let usable = max(0, bounds.height - bottomInset)
        let slack = max(0, usable - minTotal)
        return min(slack / CGFloat(rows.count), config.maxRowStretch * zoom)
    }

    func rowHeight(for row: PopupRow, width: CGFloat? = nil) -> CGFloat {
        if !config.tableColumns.isEmpty { return config.rowHeight * zoom }
        guard config.wrapContent, row.content != nil || row.detail != nil else {
            return config.rowHeight * zoom
        }
        let font = config.rowFont(config.rowFontSize * zoom)
        let lineH = font.ascender + abs(font.descender) + font.leading
        let rowW = (width ?? bounds.width) - contentX - (config.padding + 10)
        var h = titleBlockHeight(row, font: font, rowW: rowW)
            + lineH + 14
        if let body = row.body, !body.isEmpty {
            let para = NSMutableParagraphStyle()
            para.lineBreakMode = .byWordWrapping
            let attrs: [NSAttributedString.Key: Any] = [.font: font, .paragraphStyle: para]
            let measureW = max(60, (width ?? bounds.width) - contentX - config.padding - 6)
            let b = (body as NSString).boundingRect(
                with: NSSize(width: measureW, height: .greatestFiniteMagnitude),
                options: [.usesLineFragmentOrigin, .usesFontLeading],
                attributes: attrs)
            let lines = min(max(1, config.bodyMaxLines), max(1, Int(ceil(b.height / lineH))))
            h += CGFloat(lines) * lineH + 2
        }
        return h
    }

    private func titleBlockHeight(_ row: PopupRow, font: NSFont, rowW: CGFloat) -> CGFloat {
        CGFloat(titleBlockLines(row, font: font, rowW: rowW)) * probeLineH(font)
    }

    private func probeLineH(_ font: NSFont) -> CGFloat {
        ("Ag" as NSString).boundingRect(
            with: NSSize(width: 1000, height: CGFloat.greatestFiniteMagnitude),
            options: [.usesLineFragmentOrigin, .usesFontLeading],
            attributes: [.font: font]).height
    }

    private func titleBlockLines(_ row: PopupRow, font: NSFont, rowW: CGFloat) -> Int {
        guard let content = row.content, !content.isEmpty else { return 1 }
        let attrs: [NSAttributedString.Key: Any] = [.font: font]
        let probeH = probeLineH(font)
        let keyW = (row.title as NSString).size(withAttributes: attrs).width
        let drawnW = max(40, rowW - keyW - 2)
        let fullH = (content as NSString).boundingRect(
            with: NSSize(width: drawnW, height: .greatestFiniteMagnitude),
            options: [.usesLineFragmentOrigin, .usesFontLeading], attributes: attrs).height
        return max(1, Int(ceil(fullH / probeH)))
    }

    func rect(for index: Int) -> NSRect {
        guard index >= 0, index < rows.count else { return .zero }
        let hs = heights(forWidth: bounds.width)
        let extra = stretchExtra()
        var y = topInset
        for i in 0..<index {
            y += hs[i] + extra
        }
        return NSRect(x: 0, y: y, width: bounds.width, height: hs[index] + extra)
    }

    func shownRows() -> [(row: Int, rect: NSRect)] {
        let vis = visibleRect
        guard !vis.isEmpty else { return [] }
        let hs = heights(forWidth: bounds.width)
        let extra = stretchExtra()
        var out: [(row: Int, rect: NSRect)] = []
        var y = topInset
        for i in hs.indices {
            let h = hs[i] + extra
            if y > vis.maxY { break }
            if y + h > vis.minY { out.append((i, NSRect(x: 0, y: y, width: bounds.width, height: h))) }
            y += h
        }
        return out
    }

    func contentHeight() -> CGFloat {
        var h = topInset
        for rowH in heights(forWidth: bounds.width) {
            h += rowH
        }
        return h + config.padding + bottomInset
    }

    override func mouseDown(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        if config.selectableRows {
            let i = rowIndex(at: p)
            if i >= 0, rows.indices.contains(i), !rows[i].groupHeader, checkBoxRect(in: band(for: i)).contains(p) {
                onToggleSelect?(i)
                downIndex = -1
                return
            }
        }
        if config.rowStars {
            let i = rowIndex(at: p)
            if i >= 0, rows.indices.contains(i), !rows[i].loadMore, !rows[i].groupHeader, rows[i].starred != nil,
               starRect(in: band(for: i)).insetBy(dx: -3, dy: -3).contains(p) {
                onToggleStar?(i)
                downIndex = -1
                return
            }
        }
        guard config.clickToSelect else {
            super.mouseDown(with: event)
            return
        }
        downIndex = rowIndex(at: p)
        downPoint = p
    }

    override func mouseUp(with event: NSEvent) {
        guard config.clickToSelect else {
            super.mouseUp(with: event)
            return
        }
        let p = convert(event.locationInWindow, from: nil)
        let i = rowIndex(at: p)
        if i >= 0, i == downIndex, abs(p.x - downPoint.x) + abs(p.y - downPoint.y) < 5 {
            if event.clickCount >= 2 {
                onRowDoubleClick?(i)
            } else {
                onRowClick?(i)
            }
        }
        downIndex = -1
    }

    private func rowIndex(at p: NSPoint) -> Int {
        let hs = heights(forWidth: bounds.width)
        var y = topInset
        for (i, h) in hs.enumerated() {
            if p.y >= y, p.y <= y + h { return i }
            y += h
        }
        return -1
    }

    private var cachedHeights: [CGFloat] = []
    private var cacheWidth: CGFloat = 0

    func invalidateHeightCache() {
        cachedHeights = []
        cacheWidth = 0
    }

    private func heights(forWidth w: CGFloat) -> [CGFloat] {
        if cachedHeights.count == rows.count, abs(cacheWidth - w) < 1 {
            return cachedHeights
        }
        cachedHeights = rows.map { rowHeight(for: $0, width: w) }
        cacheWidth = w
        return cachedHeights
    }

    private func drawDefault(_ row: PopupRow, in rect: NSRect, natural: CGFloat,
                             index: Int, isSel: Bool) {
        let bandH = min(rect.height, natural)
        let top = rect.minY + (rect.height - bandH) / 2
        let band = NSRect(x: rect.minX, y: top, width: rect.width, height: bandH)
        if isSel, !config.tableColumns.isEmpty {
            let pill = NSRect(x: 6, y: top + 2, width: rect.width - 12, height: bandH - 4)
            let p = NSBezierPath(roundedRect: pill, xRadius: 6, yRadius: 6)
            config.colors.highlight.setFill()
            p.fill()
            NSGraphicsContext.current?.saveGraphicsState()
            p.addClip()
            config.colors.accentOn.setFill()
            NSRect(x: pill.minX, y: pill.minY, width: 3, height: pill.height).fill()
            NSGraphicsContext.current?.restoreGraphicsState()
        } else if isSel {
            let pill = NSRect(x: 8, y: top + 5,
                              width: rect.width - 16,
                              height: bandH - 10)
            let p = NSBezierPath(roundedRect: pill, xRadius: config.buttonRadius,
                                 yRadius: config.buttonRadius)
            config.colors.highlight.setFill()
            p.fill()
            config.colors.accentOn.withAlphaComponent(0.45).setStroke()
            p.lineWidth = 1
            p.stroke()
            NSGraphicsContext.current?.saveGraphicsState()
            p.addClip()
            config.colors.accentOn.setFill()
            NSRect(x: pill.minX, y: pill.minY, width: 3, height: pill.height).fill()
            NSGraphicsContext.current?.restoreGraphicsState()
        }
        if config.selectableRows, !row.groupHeader, !row.loadMore || config.tableColumns.isEmpty {
            drawCheckBox(checkBoxRect(in: band), on: selected.contains(index))
        }
        if config.rowStars, !row.loadMore, !row.groupHeader, let on = row.starred {
            drawStar(starRect(in: band), on: on)
        }
        if !config.tableColumns.isEmpty {
            drawTableRow(row, band: band)
            return
        }
        let font = config.rowFont(config.rowFontSize * zoom)
        let lineH = font.ascender + abs(font.descender) + font.leading
        let x = contentX
        var y = top + 6
        let titleAttrs: [NSAttributedString.Key: Any] = [
            .font: font, .foregroundColor: config.colors.text,
        ]
        let title = row.title as NSString
        let tsz = title.size(withAttributes: titleAttrs)
        title.draw(at: NSPoint(x: x, y: y), withAttributes: titleAttrs)

        if config.wrapContent {
            let rowW = rect.width - x - (config.padding + 10)
            let blockH = titleBlockHeight(row, font: font, rowW: rowW)
            let accent = config.colors.tone(.accent2)
            let highlight = config.highlightMatches && !highlightQuery.isEmpty
            drawText(row.title, in: NSRect(x: x, y: y, width: tsz.width, height: lineH),
                     font: font, baseColor: config.colors.text, accent: accent,
                     wrap: false, highlight: highlight)
            if let content = row.content, !content.isEmpty {
                let textW = rect.width - (x + tsz.width) - config.padding - 12
                drawText(content,
                         in: NSRect(x: x + tsz.width + 6, y: y,
                                    width: max(40, textW), height: blockH),
                         font: font, baseColor: config.colors.text, accent: accent,
                         wrap: true, highlight: highlight)
            }
            y += blockH + 4
            let dimAttrs: [NSAttributedString.Key: Any] = [
                .font: font, .foregroundColor: config.colors.dim,
            ]
            if let detail = row.detail, !detail.isEmpty {
                drawText(detail,
                         in: NSRect(x: x, y: y,
                                    width: rect.width - x - (config.padding + 10),
                                    height: lineH),
                         font: font, baseColor: config.colors.dim, accent: accent,
                         wrap: false, highlight: highlight)
            }
            if let trailing = row.trailing, !trailing.isEmpty {
                let s = trailing as NSString
                let sz = s.size(withAttributes: dimAttrs)
                drawText(trailing,
                         in: NSRect(x: rect.maxX - config.padding - 10 - sz.width, y: y,
                                    width: sz.width, height: lineH),
                         font: font, baseColor: config.colors.dim, accent: accent,
                         wrap: false, highlight: highlight)
            }
            if let body = row.body, !body.isEmpty {
                let textW = rect.width - x - config.padding - 6
                let bodyH = max(0, band.maxY - (y + lineH + 6))
                drawText(body,
                         in: NSRect(x: x, y: y + lineH + 2, width: textW, height: bodyH),
                         font: font,
                         baseColor: config.colors.dim.withAlphaComponent(0.85),
                         accent: accent, wrap: true, highlight: highlight)
            }
            return
        }

        if let trailing = row.trailing {
            let tAttrs: [NSAttributedString.Key: Any] = [
                .font: font, .foregroundColor: config.colors.dim,
            ]
            let s = trailing as NSString
            let sz = s.size(withAttributes: tAttrs)
            s.draw(at: NSPoint(x: rect.maxX - config.padding - 10 - sz.width, y: y),
                   withAttributes: tAttrs)
        }
        y += tsz.height + 2
        if let content = row.content, !content.isEmpty {
            let para = NSMutableParagraphStyle()
            para.lineBreakMode = .byWordWrapping
            let contentAttrs: [NSAttributedString.Key: Any] = [
                .font: font, .foregroundColor: config.colors.dim,
                .paragraphStyle: para,
            ]
            let textW = rect.width - x - config.padding - 6
            let contentRect = NSRect(x: x, y: y, width: textW,
                                     height: max(0, band.maxY - y - 4))
            NSGraphicsContext.current?.saveGraphicsState()
            NSBezierPath(rect: contentRect).addClip()
            (content as NSString).draw(
                with: contentRect,
                options: [.usesLineFragmentOrigin, .usesFontLeading],
                attributes: contentAttrs)
            NSGraphicsContext.current?.restoreGraphicsState()
        }
    }

    private func drawTableRow(_ row: PopupRow, band: NSRect) {
        let font = config.rowFont(config.rowFontSize * zoom)
        let lineH = font.ascender + abs(font.descender) + font.leading
        let y = band.midY - lineH / 2
        let highlight = config.highlightMatches && !highlightQuery.isEmpty
        if row.loadMore {
            drawText(row.title, in: NSRect(x: contentX, y: y, width: band.width - 2 * contentX,
                                           height: lineH),
                     font: font, baseColor: config.colors.tone(.accent2), accent: config.colors.accentOn,
                     wrap: false, highlight: false)
            return
        }
        if row.groupHeader {
            let r = NSRect(x: 4, y: band.minY + 1, width: band.width - 8, height: band.height - 2)
            config.colors.surface0.withAlphaComponent(0.7).setFill()
            NSBezierPath(roundedRect: r, xRadius: 5, yRadius: 5).fill()
            let bold = NSFontManager.shared.convert(font, toHaveTrait: .boldFontMask)
            let s = NSMutableAttributedString(string: (row.content == "1" ? "▸  " : "▾  "),
                                              attributes: [.font: font, .foregroundColor: config.colors.dim])
            s.append(NSAttributedString(string: row.title, attributes: [.font: bold, .foregroundColor: config.colors.text]))
            s.append(NSAttributedString(string: "   " + (row.trailing ?? ""),
                                        attributes: [.font: font, .foregroundColor: config.colors.dim]))
            s.draw(with: NSRect(x: 14, y: y, width: band.width - 28, height: lineH),
                   options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine])
            return
        }
        let frames = PopupRowView.tableFrames(config, width: band.width)
        let cells: [(text: String, style: PopupCellStyle?)] = config.tableColumns.map { col in
            let flat = (row.cellText(col.field) ?? "").replacingOccurrences(of: "\n", with: " ")
            return (flat, flat.isEmpty ? nil : config.tableCellStyle?(col.field, flat))
        }
        let rowAlpha: CGFloat = cells.contains { $0.style?.quietsRow == true } ? 0.6 : 1
        for (i, col) in config.tableColumns.enumerated() where i < frames.count {
            let f = frames[i]
            guard f.w > 8 else { continue }
            let (flat, style) = cells[i]
            guard !flat.isEmpty else { continue }
            let hue = style.map { config.colors.tone($0.tone) }
            let color = ((style?.tinted == true || style?.tone == .dim) ? hue : nil)
                ?? config.colors.text
            let cellFont = style?.bold == true
                ? NSFontManager.shared.convert(font, toHaveTrait: .boldFontMask) : font
            var cellRect = NSRect(x: f.x + 3, y: y, width: f.w - 8, height: lineH)
            if let style, let mark = style.mark, let hue {
                let side = (9 * zoom).rounded()
                drawStageMark(NSRect(x: cellRect.minX, y: (band.midY - side / 2).rounded(),
                                     width: side, height: side),
                              mark, color: hue.withAlphaComponent(rowAlpha))
                let gap = side + 7 * zoom
                cellRect.origin.x += gap
                cellRect.size.width = max(0, cellRect.width - gap)
            }
            drawText(flat, in: cellRect,
                     font: cellFont, baseColor: color.withAlphaComponent(color.alphaComponent * rowAlpha),
                     accent: config.colors.tone(.accent2),
                     wrap: false, highlight: highlight, align: col.align)
        }
        let line = NSBezierPath()
        line.move(to: NSPoint(x: contentX, y: band.maxY - 0.5))
        line.line(to: NSPoint(x: band.maxX - config.padding - 10, y: band.maxY - 0.5))
        config.colors.hairline.setStroke()
        line.lineWidth = 1
        line.stroke()
    }

    private func drawStageMark(_ r: NSRect, _ mark: PopupCellStyle.Mark, color: NSColor) {
        let lw: CGFloat = 1.5 * zoom
        let box = r.insetBy(dx: lw / 2, dy: lw / 2)
        let path = NSBezierPath(roundedRect: box, xRadius: 2 * zoom, yRadius: 2 * zoom)
        color.set()
        switch mark {
        case .filled:
            NSBezierPath(roundedRect: r, xRadius: 2 * zoom, yRadius: 2 * zoom).fill()
        case .half:
            NSGraphicsContext.saveGraphicsState()
            NSRect(x: r.minX, y: r.minY, width: r.width / 2, height: r.height).clip()
            NSBezierPath(roundedRect: r, xRadius: 2 * zoom, yRadius: 2 * zoom).fill()
            NSGraphicsContext.restoreGraphicsState()
            path.lineWidth = lw
            path.stroke()
        case .hollow:
            path.lineWidth = lw
            path.stroke()
        }
    }

    private func drawStar(_ r: NSRect, on: Bool) {
        let p = NSBezierPath()
        let c = NSPoint(x: r.midX, y: r.midY + 0.5)
        let outer = r.width / 2, inner = outer * 0.45
        for k in 0..<10 {
            let a = (-90 + CGFloat(k) * 36) * .pi / 180
            let rad = k % 2 == 0 ? outer : inner
            let pt = NSPoint(x: c.x + cos(a) * rad, y: c.y + sin(a) * rad)
            if k == 0 { p.move(to: pt) } else { p.line(to: pt) }
        }
        p.close()
        p.lineJoinStyle = .round
        if on {
            config.colors.tone(.warning).setFill()
            p.fill()
        } else {
            config.colors.dim.withAlphaComponent(0.55).setStroke()
            p.lineWidth = 1.1
            p.stroke()
        }
    }

    private func drawCheckBox(_ r: NSRect, on: Bool) {
        let p = NSBezierPath(roundedRect: r, xRadius: 3, yRadius: 3)
        if on {
            config.colors.accentOn.setFill()
            p.fill()
            let tick = NSBezierPath()
            tick.lineWidth = 1.6
            tick.move(to: NSPoint(x: r.minX + 3, y: r.midY - 0.5))
            tick.line(to: NSPoint(x: r.midX - 0.5, y: r.maxY - 3.5))
            tick.line(to: NSPoint(x: r.maxX - 2.5, y: r.minY + 3))
            config.colors.onAccent.setStroke()
            tick.stroke()
        } else {
            config.colors.highlight.withAlphaComponent(0.5).setFill()
            p.fill()
            config.colors.dim.withAlphaComponent(0.8).setStroke()
            p.lineWidth = 1
            p.stroke()
        }
    }

    private func drawText(_ text: String, in rect: NSRect, font: NSFont,
                          baseColor: NSColor, accent: NSColor,
                          wrap: Bool, highlight: Bool, align: NSTextAlignment = .natural) {
        let attr = NSMutableAttributedString(string: text, attributes: [
            .font: font, .foregroundColor: baseColor,
        ])
        if highlight,
           let ranges = PopupFuzzy.matchRanges(highlightQuery, against: text) {
            for r in ranges {
                attr.addAttribute(.foregroundColor, value: accent, range: r)
            }
        }
        let para = NSMutableParagraphStyle()
        para.lineBreakMode = wrap ? .byWordWrapping : .byTruncatingTail
        para.alignment = align
        attr.addAttribute(.paragraphStyle, value: para,
                          range: NSRange(location: 0, length: attr.length))
        NSGraphicsContext.current?.saveGraphicsState()
        NSBezierPath(rect: rect).addClip()
        attr.draw(with: rect, options: [.usesLineFragmentOrigin, .usesFontLeading])
        NSGraphicsContext.current?.restoreGraphicsState()
    }
}

final class PopupTableHeaderView: NSView {
    var config: PopupConfig
    var zoom: CGFloat = 1 { didSet { needsDisplay = true } }
    var sortColumn: Int? { didSet { needsDisplay = true } }
    var sortAscending = true { didSet { needsDisplay = true } }
    var onSort: ((Int) -> Void)?
    var activeFilters: Set<Int> = [] { didSet { needsDisplay = true } }
    var onFilter: ((Int, NSRect) -> Void)?
    var onReorder: ((Int, Int) -> Void)?
    var onFit: (() -> Void)? { didSet { needsDisplay = true } }
    var extraMenu: (() -> [NSMenuItem])?
    private var hoverFit = false
    private var reorder: (from: Int, x: CGFloat, target: Int)?
    private var pressCol: Int?

    private func reorderTarget(from: Int, x: CGFloat) -> Int {
        let fs = frames
        var slot = fs.indices.filter { fs[$0].x + fs[$0].w / 2 < x }.count
        if slot > from { slot -= 1 }
        return max(0, min(fs.count - 1, slot))
    }
    var onResize: (([CGFloat], Bool) -> Void)?
    private var drag: (divider: Int, startX: CGFloat, start: [CGFloat])?
    private var downX: CGFloat = 0
    private var moved = false

    override var isFlipped: Bool { true }

    init(config: PopupConfig) {
        self.config = config
        super.init(frame: .zero)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) not supported") }

    private var frames: [(x: CGFloat, w: CGFloat)] {
        PopupRowView.tableFrames(config, width: bounds.width)
    }
    private var usable: CGFloat {
        let x0 = config.rowLeadInset
        return max(1, bounds.width - x0 - (config.padding + 10))
    }

    private func filterRect(_ i: Int) -> NSRect? {
        guard config.tableColumns.indices.contains(i), config.tableColumns[i].filterable,
              frames.indices.contains(i), frames[i].w > 30 else { return nil }
        let f = frames[i]
        let s: CGFloat = 16
        return NSRect(x: f.x + f.w - s - 5, y: bounds.midY - s / 2, width: s, height: s)
    }

    private var fitRect: NSRect? {
        guard onFit != nil, config.rowLeadInset >= 20 else { return nil }
        let s: CGFloat = 18
        return NSRect(x: ((config.rowLeadInset - s) / 2).rounded(), y: bounds.midY - s / 2,
                      width: s, height: s)
    }

    private func dividerRect(_ i: Int) -> NSRect {
        let f = frames[i]
        return NSRect(x: f.x + f.w - 4, y: 0, width: 8, height: bounds.height)
    }

    override func resetCursorRects() {
        let n = config.tableColumns.count
        for i in 0..<n {
            if let fr = filterRect(i) { addCursorRect(fr, cursor: .pointingHand) }
        }
        if let fr = fitRect { addCursorRect(fr, cursor: .pointingHand) }
        guard n > 1 else { return }
        for i in 0..<(n - 1) { addCursorRect(dividerRect(i), cursor: .resizeLeftRight) }
    }

    private var hoverFilter: Int?
    private var tracking: NSTrackingArea?
    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let t = tracking { removeTrackingArea(t) }
        let t = NSTrackingArea(rect: .zero, options: [.mouseMoved, .mouseEnteredAndExited,
                                                      .activeInKeyWindow, .inVisibleRect],
                               owner: self, userInfo: nil)
        addTrackingArea(t)
        tracking = t
    }
    override func mouseMoved(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        let over = config.tableColumns.indices.first { filterRect($0)?.contains(p) == true }
        let fit = fitRect?.contains(p) == true
        if over != hoverFilter || fit != hoverFit {
            hoverFilter = over
            hoverFit = fit
            toolTip = fit ? "Fit columns to their content"
                : over.map { "Filter \(config.tableColumns[$0].title)" }
            needsDisplay = true
        }
    }
    override func mouseExited(with event: NSEvent) {
        if hoverFilter != nil || hoverFit { hoverFilter = nil; hoverFit = false; needsDisplay = true }
    }

    override func rightMouseDown(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        if let i = frames.firstIndex(where: { p.x >= $0.x && p.x < $0.x + $0.w }),
           let fr = filterRect(i) {
            onFilter?(i, fr)
            return
        }
        let extra = extraMenu?() ?? []
        if onFit != nil || !extra.isEmpty {
            let menu = NSMenu()
            if let fit = onFit { menu.addItem(menuItem("Fit Columns to Content", fit)) }
            if onFit != nil, !extra.isEmpty { menu.addItem(.separator()) }
            extra.forEach(menu.addItem)
            NSMenu.popUpContextMenu(menu, with: event, for: self)
            return
        }
        super.rightMouseDown(with: event)
    }

    override func draw(_ dirtyRect: NSRect) {
        let c = config.colors
        c.mantle.setFill()
        bounds.fill()
        let base = config.rowFont(config.rowFontSize * zoom * 0.86)
        let font = NSFontManager.shared.convert(base, toHaveTrait: .boldFontMask)
        let lineH = font.ascender + abs(font.descender) + font.leading
        let fs = frames
        for (i, col) in config.tableColumns.enumerated() where i < fs.count {
            let f = fs[i]
            var title = col.title.uppercased()
            if sortColumn == i { title += sortAscending ? " ↑" : " ↓" }
            let para = NSMutableParagraphStyle()
            para.lineBreakMode = .byTruncatingTail
            para.alignment = col.align
            let filtered = activeFilters.contains(i)
            let color = sortColumn == i || filtered ? c.accentOn : c.dim
            let fr = filterRect(i)
            let titleW = max(0, f.w - 8 - (fr.map { $0.width + 2 } ?? 0))
            (title as NSString).draw(
                with: NSRect(x: f.x + 3, y: bounds.midY - lineH / 2, width: titleW, height: lineH),
                options: [.usesLineFragmentOrigin, .usesFontLeading],
                attributes: [.font: font, .foregroundColor: color, .paragraphStyle: para, .kern: 0.6])
            if let fr {
                if filtered {
                    c.accentOn.setFill()
                    NSBezierPath(roundedRect: fr, xRadius: 4, yRadius: 4).fill()
                } else if hoverFilter == i {
                    c.text.withAlphaComponent(0.12).setFill()
                    NSBezierPath(roundedRect: fr, xRadius: 4, yRadius: 4).fill()
                }
                ButtonStyle.chevron(in: fr.insetBy(dx: 4, dy: 4),
                                    color: filtered ? c.onAccent : c.dim.withAlphaComponent(0.9))
            }
            if i < fs.count - 1 {
                let d = NSBezierPath()
                d.move(to: NSPoint(x: f.x + f.w - 0.5, y: 6))
                d.line(to: NSPoint(x: f.x + f.w - 0.5, y: bounds.height - 6))
                c.hairline.setStroke()
                d.lineWidth = 1
                d.stroke()
            }
        }
        if let r = reorder, fs.indices.contains(r.from) {
            let f = fs[r.from]
            c.accentOn.withAlphaComponent(0.18).setFill()
            NSBezierPath(roundedRect: NSRect(x: f.x + r.x - downX, y: 2, width: f.w, height: bounds.height - 4),
                         xRadius: 4, yRadius: 4).fill()
            if r.target != r.from {
                let t = fs[r.target]
                let edge = r.target > r.from ? t.x + t.w : t.x
                c.accentOn.setFill()
                NSRect(x: edge - 1.5, y: 3, width: 3, height: bounds.height - 6).fill()
            }
        }
        if let fr = fitRect {
            if hoverFit {
                c.text.withAlphaComponent(0.12).setFill()
                NSBezierPath(roundedRect: fr, xRadius: 4, yRadius: 4).fill()
            }
            ButtonStyle.symbol("arrow.left.and.right", in: fr,
                               color: hoverFit ? c.text : c.dim.withAlphaComponent(0.9), size: 9)
        }
        let rule = NSBezierPath()
        rule.move(to: NSPoint(x: 0, y: bounds.height - 0.5))
        rule.line(to: NSPoint(x: bounds.width, y: bounds.height - 0.5))
        c.hairline.setStroke()
        rule.lineWidth = 1
        rule.stroke()
    }

    override func mouseDown(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        downX = p.x
        moved = false
        let n = config.tableColumns.count
        reorder = nil
        pressCol = nil
        if n > 1, let i = (0..<(n - 1)).first(where: { dividerRect($0).contains(p) }) {
            drag = (i, p.x, frames.map { $0.w / usable * 100 })
        } else {
            drag = nil
            if n > 1, !config.tableColumns.indices.contains(where: { filterRect($0)?.contains(p) == true }) {
                pressCol = frames.firstIndex { p.x >= $0.x && p.x < $0.x + $0.w }
            }
        }
    }

    override func mouseDragged(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        if let from = pressCol, drag == nil {
            if reorder == nil && abs(p.x - downX) < 6 { return }
            reorder = (from, p.x, reorderTarget(from: from, x: p.x))
            NSCursor.closedHand.set()
            needsDisplay = true
            return
        }
        guard let d = drag else { return }
        moved = true
        onResize?(dividerPercents(d, x: p.x), false)
    }

    private func dividerPercents(_ d: (divider: Int, startX: CGFloat, start: [CGFloat]),
                                 x: CGFloat) -> [CGFloat] {
        var pcts = d.start
        let minPct: CGFloat = 3
        var delta = (x - d.startX) / usable * 100
        delta = max(minPct - pcts[d.divider], min(delta, pcts[d.divider + 1] - minPct))
        pcts[d.divider] += delta
        pcts[d.divider + 1] -= delta
        return pcts.map { ($0 * 10).rounded() / 10 }
    }

    override func mouseUp(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        pressCol = nil
        if let r = reorder {
            reorder = nil
            NSCursor.arrow.set()
            needsDisplay = true
            let to = reorderTarget(from: r.from, x: p.x)
            if to != r.from { onReorder?(r.from, to) }
            window?.invalidateCursorRects(for: self)
            return
        }
        if let d = drag {
            drag = nil
            if moved {
                onResize?(dividerPercents(d, x: p.x), true)
                window?.invalidateCursorRects(for: self)
            }
            return
        }
        guard abs(p.x - downX) < 5 else { return }
        if fitRect?.insetBy(dx: -3, dy: -3).contains(p) == true {
            onFit?()
            return
        }
        if let i = config.tableColumns.indices.first(where: {
            filterRect($0)?.insetBy(dx: -2, dy: -3).contains(p) == true }) {
            onFilter?(i, filterRect(i)!)
            return
        }
        if let i = frames.firstIndex(where: { p.x >= $0.x && p.x < $0.x + $0.w }) {
            if config.tableColumns[i].sortable {
                onSort?(i)
            } else if let fr = filterRect(i) {
                onFilter?(i, fr)
            }
        }
    }
}

public func popupDrawImage(_ img: NSImage, in rect: NSRect, fraction: CGFloat = 1) {
    guard let ctx = NSGraphicsContext.current else { return }
    ctx.saveGraphicsState()
    let t = NSAffineTransform()
    t.translateX(by: 0, yBy: rect.origin.y * 2 + rect.height)
    t.scaleX(by: 1, yBy: -1)
    t.concat()
    img.draw(in: rect, from: .zero, operation: .sourceOver, fraction: fraction)
    ctx.restoreGraphicsState()
}

final class PopupTextView: NSTextView {
    var onPasteImage: ((NSImage) -> Void)?
    var onTextChange: (() -> Void)?
    var absolutePathAt: ((Int) -> String?)?
    private static let imageExts = Set(["png", "jpg", "jpeg", "gif", "heic", "webp", "tif", "tiff"])

    override func changeColor(_ sender: Any?) { }

    override func didChangeText() {
        super.didChangeText()
        onTextChange?()
    }

    var onOpenImage: ((String) -> Void)?
    override func mouseDown(with event: NSEvent) {
        if event.clickCount == 1, let lm = layoutManager, let tc = textContainer,
           let path = imagePath(at: convert(event.locationInWindow, from: nil), lm, tc) {
            onOpenImage?(path)
            return
        }
        super.mouseDown(with: event)
    }
    private func imagePath(at pt: NSPoint, _ lm: NSLayoutManager, _ tc: NSTextContainer) -> String? {
        let o = textContainerOrigin
        let p = NSPoint(x: pt.x - o.x, y: pt.y - o.y)
        let g = lm.glyphIndex(for: p, in: tc, fractionOfDistanceThroughGlyph: nil)
        guard lm.boundingRect(forGlyphRange: NSRange(location: g, length: 1), in: tc).contains(p) else { return nil }
        return absolutePathAt?(lm.characterIndexForGlyph(at: g))
    }

    override func menu(for event: NSEvent) -> NSMenu? {
        let m = NSMenu()
        let pt = convert(event.locationInWindow, from: nil)
        let idx = characterIndexForInsertion(at: pt)
        if let path = absolutePathAt?(idx) {
            let full = NSMenuItem(title: "Open Full Size", action: #selector(openFullSize(_:)),
                                  keyEquivalent: "")
            full.target = self
            full.representedObject = path
            m.addItem(full)
            let item = NSMenuItem(title: "copy image path",
                                  action: #selector(copyImagePath(_:)),
                                  keyEquivalent: "")
            item.target = self
            item.representedObject = path
            m.addItem(item)
            m.addItem(.separator())
        }
        if onCopyFilePath != nil {
            let fpath = NSMenuItem(title: "Copy File Path",
                                   action: #selector(copyFilePath(_:)),
                                   keyEquivalent: "")
            fpath.target = self
            m.addItem(fpath)
        }
        let open = NSMenuItem(title: "Open file at path…",
                              action: #selector(openFileAtPath(_:)),
                              keyEquivalent: "")
        open.target = self
        m.addItem(open)
        return m
    }

    var onCopiedImagePath: ((String) -> Void)?
    var onCopyFilePath: (() -> Void)?
    @objc private func copyFilePath(_ sender: NSMenuItem) {
        onCopyFilePath?()
    }

    var onOpenFileAtPath: (() -> Void)?
    @objc private func openFileAtPath(_ sender: NSMenuItem) {
        onOpenFileAtPath?()
    }

    @objc private func openFullSize(_ sender: NSMenuItem) {
        if let path = sender.representedObject as? String { onOpenImage?(path) }
    }

    @objc private func copyImagePath(_ sender: NSMenuItem) {
        guard let path = sender.representedObject as? String else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(path, forType: .string)
        onCopiedImagePath?((path as NSString).abbreviatingWithTildeInPath)
    }

    static func image(from pb: NSPasteboard) -> NSImage? {
        if let imgs = pb.readObjects(forClasses: [NSImage.self], options: nil) as? [NSImage],
           let first = imgs.first, first.size.width > 1 {
            return first
        }
        if let urls = pb.readObjects(forClasses: [NSURL.self],
                                     options: [.urlReadingFileURLsOnly: true]) as? [URL],
           let u = urls.first, imageExts.contains(u.pathExtension.lowercased()),
           let img = NSImage(contentsOf: u), img.size.width > 1 {
            return img
        }
        return nil
    }

    override func paste(_ sender: Any?) {
        if let img = Self.image(from: NSPasteboard.general) {
            onPasteImage?(img)
            return
        }
        super.paste(sender)
    }

    override func draggingEntered(_ sender: NSDraggingInfo) -> NSDragOperation {
        if Self.image(from: sender.draggingPasteboard) != nil { return .copy }
        return super.draggingEntered(sender)
    }

    override func performDragOperation(_ sender: NSDraggingInfo) -> Bool {
        if let img = Self.image(from: sender.draggingPasteboard) {
            onPasteImage?(img)
            return true
        }
        return super.performDragOperation(sender)
    }
}

final class ThemeButton: NSView {
    private var config: PopupConfig
    var title: String { didSet { needsDisplay = true } }
    var symbol: String? { didSet { needsDisplay = true } }
    var flat = false
    var isOn = false { didSet { needsDisplay = true } }
    enum Segment { case alone, first, middle, last }
    var segment: Segment = .alone { didSet { needsDisplay = true } }
    var onClick: (() -> Void)?
    private var down = false
    private var hover = false
    private var trackingArea: NSTrackingArea?

    init(config: PopupConfig, title: String, symbol: String? = nil) {
        self.config = config
        self.title = title
        self.symbol = symbol
        super.init(frame: .zero)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) not supported") }
    override var isFlipped: Bool { true }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let t = trackingArea { removeTrackingArea(t) }
        let t = NSTrackingArea(rect: bounds,
                               options: [.mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect],
                               owner: self, userInfo: nil)
        addTrackingArea(t)
        trackingArea = t
    }
    override func mouseEntered(with event: NSEvent) { hover = true; needsDisplay = true }
    override func mouseExited(with event: NSEvent) { hover = false; needsDisplay = true }

    var chevron = false { didSet { needsDisplay = true } }
    private static let iconW: CGFloat = 12, iconGap: CGFloat = 4, chevW: CGFloat = 12
    static let hPad: CGFloat = 9

    private func contentWidth(_ t: String, _ st: ButtonState) -> CGFloat {
        let tw = t.isEmpty ? 0 : (t as NSString).size(withAttributes: [
            .font: ButtonStyle.font(config.buttonFontSize, st)]).width
        var w = tw
        if symbol != nil { w += Self.iconW + (t.isEmpty ? 0 : Self.iconGap) }
        if chevron { w += Self.chevW }
        return w
    }
    func fittingWidth(for t: String? = nil) -> CGFloat {
        let t = t ?? title
        if t.isEmpty && !chevron { return bounds.height > 0 ? bounds.height + 4 : 28 }
        return ceil(contentWidth(t, .on) + Self.hPad * 2)
    }

    override func draw(_ dirty: NSRect) {
        let st: ButtonState = down ? .pressed
            : isOn ? (hover ? .onHover : .on)
            : hover ? .hover : .idle
        if segment == .alone {
            ButtonStyle.draw(bounds, st, config.colors, radius: config.buttonRadius, flat: flat)
        } else {
            let rad: CGFloat = 8
            let ext = NSRect(x: segment == .first ? 0 : -rad, y: 0,
                             width: bounds.width + (segment == .first ? 0 : rad) + (segment == .last ? 0 : rad),
                             height: bounds.height).insetBy(dx: 0, dy: 0.5)
            NSGraphicsContext.saveGraphicsState()
            NSBezierPath(rect: bounds).setClip()
            ButtonStyle.fill(st, config.colors).setFill()
            NSBezierPath(roundedRect: ext, xRadius: rad, yRadius: rad).fill()
            NSGraphicsContext.restoreGraphicsState()
            if segment != .first {
                config.colors.text.withAlphaComponent(0.14).setFill()
                NSRect(x: 0, y: 5, width: 1, height: bounds.height - 10).fill()
            }
        }
        let color = ButtonStyle.text(st, config.colors)
        if let symbol, title.isEmpty, !chevron {
            ButtonStyle.symbol(symbol, in: bounds, color: color, size: config.buttonFontSize + 0.5)
            return
        }
        let tw = title.isEmpty ? 0 : (title as NSString).size(withAttributes: [
            .font: ButtonStyle.font(config.buttonFontSize, st)]).width
        var x = (bounds.midX - contentWidth(title, st) / 2).rounded()
        if let symbol {
            ButtonStyle.symbol(symbol, in: NSRect(x: x, y: 0, width: Self.iconW, height: bounds.height),
                               color: color, size: config.buttonFontSize)
            x += Self.iconW + (title.isEmpty ? 0 : Self.iconGap)
        }
        if !title.isEmpty {
            ButtonStyle.label(title, in: NSRect(x: x, y: 0, width: tw, height: bounds.height),
                              st, config.colors, size: config.buttonFontSize)
            x += tw
        }
        if chevron {
            ButtonStyle.chevron(in: NSRect(x: x + 3, y: bounds.midY - 4, width: 8, height: 8),
                                color: color)
        }
    }
    override func mouseDown(with e: NSEvent) { down = true; needsDisplay = true }
    override func mouseUp(with e: NSEvent) {
        if bounds.contains(convert(e.locationInWindow, from: nil)) { onClick?() }
        down = false
        needsDisplay = true
    }
}

enum PopupThemeDefaults {
    static var colors = PopupColors()
}

final class ThemedPushButton: NSButton, PopupThemeable {
    enum Role { case normal, primary, danger }
    var colors = PopupThemeDefaults.colors { didSet { needsDisplay = true } }
    var role: Role = .normal { didSet { needsDisplay = true } }
    var capsule = true { didSet { needsDisplay = true } }
    var rowStyle = false { didSet { needsDisplay = true } }
    var keycap: String? { didSet { needsDisplay = true } }
    var segment: ThemeButton.Segment = .alone { didSet { needsDisplay = true } }
    var chipSymbol: String? { didSet { invalidateIntrinsicContentSize(); needsDisplay = true } }
    var chipOn = false { didSet { needsDisplay = true } }
    var keyFocus = false { didSet { needsDisplay = true } }
    private var hover = false
    private var tracking: NSTrackingArea?
    func applyColors(_ c: PopupColors) { colors = c }

    private var labelFont: NSFont {
        .systemFont(ofSize: controlSize == .small ? 11 : 12, weight: .semibold)
    }
    override var intrinsicContentSize: NSSize {
        if chipSymbol != nil { return NSSize(width: 26, height: 26) }
        let w = (title as NSString).size(withAttributes: [.font: labelFont]).width
        return NSSize(width: ceil(w) + (controlSize == .small ? 20 : 26) + (capsule ? 8 : 0),
                      height: controlSize == .small ? 22 : 26)
    }
    override var title: String { didSet { invalidateIntrinsicContentSize(); needsDisplay = true } }
    override var isEnabled: Bool { didSet { needsDisplay = true } }
    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let t = tracking { removeTrackingArea(t) }
        let t = NSTrackingArea(rect: bounds, options: [.mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect],
                               owner: self, userInfo: nil)
        addTrackingArea(t)
        tracking = t
    }
    override func mouseEntered(with event: NSEvent) { hover = true; needsDisplay = true }
    override func mouseExited(with event: NSEvent) { hover = false; needsDisplay = true }

    override func draw(_ dirtyRect: NSRect) {
        let c = colors
        let st: ButtonState = !isEnabled ? .idle : isHighlighted ? .pressed : hover ? .hover : .idle
        let r = bounds.insetBy(dx: keyFocus ? 3.5 : 1, dy: keyFocus ? 3.5 : 1)
        let rad: CGFloat = rowStyle ? 9 : capsule || chipSymbol != nil ? r.height / 2 : (keyFocus ? 4 : 6)
        let path = NSBezierPath(roundedRect: r, xRadius: rad, yRadius: rad)
        var fg: NSColor
        if segment != .alone {
            let ext = NSRect(x: segment == .first ? 0 : -rad, y: 0,
                             width: bounds.width + (segment == .first ? 0 : rad) + (segment == .last ? 0 : rad),
                             height: bounds.height).insetBy(dx: 0, dy: 0.5)
            NSGraphicsContext.saveGraphicsState()
            NSBezierPath(rect: bounds).setClip()
            c.text.withAlphaComponent(st == .pressed ? 0.18 : st == .hover ? 0.13 : 0.08).setFill()
            NSBezierPath(roundedRect: ext, xRadius: r.height / 2, yRadius: r.height / 2).fill()
            NSGraphicsContext.restoreGraphicsState()
            if segment == .last { c.text.withAlphaComponent(0.14).setFill(); NSRect(x: 0, y: 5, width: 1, height: bounds.height - 10).fill() }
            let a: [NSAttributedString.Key: Any] = [.font: labelFont, .foregroundColor: c.text.withAlphaComponent(isEnabled ? 1 : 0.4)]
            let sz = (title as NSString).size(withAttributes: a)
            (title as NSString).draw(at: NSPoint(x: (bounds.midX - sz.width / 2).rounded(), y: (bounds.midY - sz.height / 2).rounded()), withAttributes: a)
            return
        }
        if let sym = chipSymbol {
            let a = c.accentOn
            a.withAlphaComponent((chipOn ? 0.30 : 0.18) + (st == .hover ? 0.08 : 0) + (st == .pressed ? 0.14 : 0)).setFill()
            path.fill()
            ButtonStyle.symbol(sym, in: bounds, color: c.readable(c.accent, min: 4).withAlphaComponent(isEnabled ? 1 : 0.4), size: 12)
            return
        }
        switch role {
        case .primary:
            let a = c.accentOn
            a.withAlphaComponent(rowStyle ? (st == .pressed ? 0.30 : st == .hover ? 0.22 : 0.13)
                                          : (st == .pressed ? 0.34 : st == .hover ? 0.26 : 0.16)).setFill()
            path.fill()
            if !rowStyle {
                a.withAlphaComponent(0.55).setStroke()
                path.lineWidth = 1
                path.stroke()
            }
            fg = c.readable(c.accent, min: 4)
        case .danger:
            let d = c.tone(.danger)
            if st != .idle {
                d.withAlphaComponent(st == .pressed ? 0.2 : 0.1).setFill()
                path.fill()
            }
            d.withAlphaComponent(0.55).setStroke()
            path.lineWidth = 1
            path.stroke()
            fg = d
        case .normal:
            if st != .idle {
                (st == .pressed ? c.surface1 : c.surface0).setFill()
                path.fill()
            }
            c.dim.withAlphaComponent(0.4).setStroke()
            path.lineWidth = 1
            path.stroke()
            fg = c.text
        }
        if !isEnabled { fg = fg.withAlphaComponent(0.4) }
        if keyFocus {
            ButtonStyle.focusStroke(c).setStroke()
            let ring = NSBezierPath(roundedRect: bounds.insetBy(dx: 1, dy: 1), xRadius: 7, yRadius: 7)
            ring.lineWidth = 2
            ring.stroke()
        }
        let attrs: [NSAttributedString.Key: Any] = [.font: labelFont, .foregroundColor: fg]
        let sz = (title as NSString).size(withAttributes: attrs)
        if rowStyle {
            (title as NSString).draw(at: NSPoint(x: 14, y: (bounds.midY - sz.height / 2).rounded()), withAttributes: attrs)
            if let k = keycap {
                let ka: [NSAttributedString.Key: Any] = [.font: NSFont.monospacedSystemFont(ofSize: 10.5, weight: .medium), .foregroundColor: fg]
                let ks = (k as NSString).size(withAttributes: ka)
                let kr = NSRect(x: bounds.maxX - 14 - ks.width - 10, y: bounds.midY - 9, width: ks.width + 10, height: 18)
                fg.withAlphaComponent(0.16).setFill()
                NSBezierPath(roundedRect: kr, xRadius: 5, yRadius: 5).fill()
                (k as NSString).draw(at: NSPoint(x: kr.minX + 5, y: kr.midY - ks.height / 2), withAttributes: ka)
            }
            return
        }
        (title as NSString).draw(at: NSPoint(x: (bounds.midX - sz.width / 2).rounded(),
                                             y: (bounds.midY - sz.height / 2).rounded()),
                                 withAttributes: attrs)
    }
}

final class ThemedPopUpButton: NSPopUpButton, PopupThemeable {
    var colors = PopupThemeDefaults.colors { didSet { needsDisplay = true } }
    private var hover = false
    private var tracking: NSTrackingArea?
    func applyColors(_ c: PopupColors) { colors = c }
    private var labelFont: NSFont { .systemFont(ofSize: controlSize == .small ? 11 : 12, weight: .medium) }
    private var shownTitle: String {
        pullsDown ? (itemArray.first?.title ?? "") : (titleOfSelectedItem ?? "")
    }
    override var intrinsicContentSize: NSSize {
        let titles = pullsDown ? [shownTitle] : itemTitles
        let w = titles.map { ($0 as NSString).size(withAttributes: [.font: labelFont]).width }.max() ?? 40
        return NSSize(width: ceil(w) + 38, height: controlSize == .small ? 22 : 26)
    }
    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let t = tracking { removeTrackingArea(t) }
        let t = NSTrackingArea(rect: bounds, options: [.mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect],
                               owner: self, userInfo: nil)
        addTrackingArea(t)
        tracking = t
    }
    override func mouseEntered(with event: NSEvent) { hover = true; needsDisplay = true }
    override func mouseExited(with event: NSEvent) { hover = false; needsDisplay = true }
    override func draw(_ dirtyRect: NSRect) {
        let c = colors
        let r = bounds.insetBy(dx: 1, dy: 1)
        let path = NSBezierPath(roundedRect: r, xRadius: 6, yRadius: 6)
        ButtonStyle.fill(hover ? .pressed : .hover, c).setFill()
        path.fill()
        ButtonStyle.inputStroke(c).setStroke()
        path.lineWidth = 1
        path.stroke()
        let fg = isEnabled ? c.text : c.text.withAlphaComponent(0.4)
        let attrs: [NSAttributedString.Key: Any] = [.font: labelFont, .foregroundColor: fg]
        let t = shownTitle as NSString
        let sz = t.size(withAttributes: attrs)
        let avail = r.width - 34
        let para = NSMutableParagraphStyle()
        para.lineBreakMode = .byTruncatingTail
        t.draw(with: NSRect(x: r.minX + 11, y: (bounds.midY - sz.height / 2).rounded(),
                            width: max(10, avail), height: sz.height),
               options: [.usesLineFragmentOrigin],
               attributes: attrs.merging([.paragraphStyle: para]) { $1 })
        ButtonStyle.chevron(in: NSRect(x: r.maxX - 19, y: bounds.midY - 4, width: 8, height: 8),
                            color: c.dim)
    }
}

final class PopupTableRowView: NSTableRowView {
    var colors = PopupThemeDefaults.colors
    var striped = false
    override func drawSelection(in dirtyRect: NSRect) {
        guard selectionHighlightStyle != .none else { return }
        let r = bounds.insetBy(dx: 6, dy: 1)
        let p = NSBezierPath(roundedRect: r, xRadius: 5, yRadius: 5)
        colors.highlight.setFill()
        p.fill()
        NSGraphicsContext.saveGraphicsState()
        p.addClip()
        colors.accentOn.setFill()
        NSRect(x: r.minX, y: r.minY, width: 3, height: r.height).fill()
        NSGraphicsContext.restoreGraphicsState()
    }
    override func drawBackground(in dirtyRect: NSRect) {
        if isGroupRowStyle { return }
        if striped {
            colors.text.withAlphaComponent(colors.isLight ? 0.04 : 0.035).setFill()
            bounds.fill()
        }
    }
    override var interiorBackgroundStyle: NSView.BackgroundStyle { .normal }
}

final class FileListPane: NSView, NSDraggingSource {
    private var config: PopupConfig
    var rows: [PopupFileBrowser.Entry] = [] {
        didSet {
            if let h = hover, !rows.indices.contains(h) { hover = nil }
            marked = []
            needsDisplay = true
        }
    }
    var selection = 0 { didSet { needsDisplay = true } }
    private(set) var marked: Set<Int> = [] { didSet { needsDisplay = true } }
    private var anchor = 0
    var selectedRows: [Int] {
        marked.isEmpty ? (rows.indices.contains(selection) ? [selection] : []) : marked.sorted()
    }
    func selectAll() {
        let all = rows.indices.filter { rows[$0].name != ".." }
        guard !all.isEmpty else { return }
        marked = Set(all)
        if !marked.contains(selection) { selection = all[0] }
        anchor = all[0]
    }
    func extendSelection(to i: Int) {
        guard rows.indices.contains(i) else { return }
        if marked.isEmpty { anchor = selection }
        let a = min(max(0, anchor), rows.count - 1)
        marked = Set((min(a, i)...max(a, i)).filter { rows[$0].name != ".." })
        selection = i
        onSelect?(i)
    }
    enum Action {
        case trash, duplicate, copy, cut, paste, newFolder, newFile, quickLook, enclosing, toggleHidden
    }
    var onAction: ((Action) -> Void)?
    static var onCompare: ((String, String) -> Void)?
    static var comparePick: String?
    var canPerform: ((Action) -> Bool)?
    var hiddenShown: (() -> Bool)?
    private(set) var hover: Int?
    var onSelect: ((Int) -> Void)?
    var onOpen: ((Int) -> Void)?
    var onParent: (() -> Void)?
    var onFocusSearch: ((String) -> Void)?
    var onHover: ((Int?) -> Void)?
    var onCopyPath: ((Int) -> Void)?
    var onOpenInNotes: ((Int) -> Void)?
    var onOpenTerminal: ((Int) -> Void)?
    var onRename: ((Int) -> Void)?
    var dropDirectory: (() -> String?)?
    var onFilesChanged: ((String) -> Void)?

    private var dragStart: (point: NSPoint, row: Int)?
    private var dropRow: Int? { didSet { if dropRow != oldValue { needsDisplay = true } } }
    private var dropWhole = false { didSet { if dropWhole != oldValue { needsDisplay = true } } }

    var textZoom: CGFloat = 1 { didSet { if oldValue != textZoom { needsDisplay = true } } }
    var rowHeight: CGFloat { rowH }
    private var rowH: CGFloat { (22 * textZoom).rounded() }
    private static let iconSize: CGFloat = 16
    private var iconSz: CGFloat { (Self.iconSize * textZoom).rounded() }
    private func trailW(_ e: PopupFileBrowser.Entry) -> CGFloat { e.trailingWidth * textZoom }
    private static let trailingInset: CGFloat = 16
    private var trackingArea: NSTrackingArea?

    init(config: PopupConfig) {
        self.config = config
        super.init(frame: .zero)
        registerForDraggedTypes([.fileURL] + NSFilePromiseReceiver.readableDraggedTypes.map { NSPasteboard.PasteboardType($0) })
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) not supported") }
    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { true }
    override var mouseDownCanMoveWindow: Bool { false }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let t = trackingArea { removeTrackingArea(t) }
        let t = NSTrackingArea(rect: bounds, options: [.mouseMoved, .mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect], owner: self, userInfo: nil)
        addTrackingArea(t)
        trackingArea = t
    }

    override func mouseEntered(with event: NSEvent) {}
    override func mouseExited(with event: NSEvent) {
        hover = nil
        onHover?(nil)
    }
    override func mouseMoved(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        let idx = Int(p.y / rowH)
        let newHover = rows.indices.contains(idx) ? idx : nil
        if newHover != hover {
            hover = newHover
            onHover?(newHover)
            needsDisplay = true
        }
    }

    func rowRect(_ i: Int) -> NSRect {
        NSRect(x: 0, y: CGFloat(i) * rowH, width: bounds.width, height: rowH)
    }
    func nameRect(_ i: Int) -> NSRect {
        let r = rowRect(i)
        var tx = 10 + iconSz + 6
        var trailing: CGFloat = 8
        if rows.indices.contains(i) {
            let e = rows[i]
            if e.isDir && e.name != ".." { tx += 4 }
            if e.trailingWidth > 0 { trailing += trailW(e) + Self.trailingInset + 4 }
        }
        return NSRect(x: tx, y: r.minY, width: max(60, r.width - tx - trailing), height: rowH)
    }

    override func draw(_ dirty: NSRect) {
        let w = bounds.width
        if dropWhole {
            config.colors.accentOn.setStroke()
            let ring = NSBezierPath(roundedRect: visibleRect.insetBy(dx: 2, dy: 2),
                                    xRadius: config.buttonRadius, yRadius: config.buttonRadius)
            ring.lineWidth = 2
            ring.stroke()
        }
        let first = max(0, Int(dirty.minY / rowH))
        let last = min(rows.count - 1, Int(dirty.maxY / rowH))
        guard first <= last else { return }
        for i in first...last {
            let e = rows[i]
            let r = rowRect(i)
            let pillR = r.insetBy(dx: 4, dy: 1)
            if i == selection || marked.contains(i) {
                let p = NSBezierPath(roundedRect: pillR, xRadius: config.buttonRadius,
                                     yRadius: config.buttonRadius)
                config.colors.highlight.setFill()
                p.fill()
                if i == selection {
                    NSGraphicsContext.saveGraphicsState()
                    p.addClip()
                    config.colors.accentOn.setFill()
                    NSRect(x: pillR.minX, y: pillR.minY, width: 3, height: pillR.height).fill()
                    NSGraphicsContext.restoreGraphicsState()
                }
            } else if let h = hover, h == i {
                ButtonStyle.fill(.hover, config.colors).setFill()
                NSBezierPath(roundedRect: pillR, xRadius: config.buttonRadius,
                             yRadius: config.buttonRadius).fill()
            }
            if i == dropRow {
                config.colors.accentOn.setStroke()
                let ring = NSBezierPath(roundedRect: pillR, xRadius: config.buttonRadius,
                                        yRadius: config.buttonRadius)
                ring.lineWidth = 2
                ring.stroke()
            }
            let icon = e.icon ?? NSWorkspace.shared.icon(forFile: e.path)
            var ir = r
            ir.origin.x += 10
            ir.size.width = iconSz
            let img = icon
            NSGraphicsContext.saveGraphicsState()
            let clip = NSBezierPath(roundedRect: ir.insetBy(dx: 1, dy: (rowH - iconSz) / 2),
                                    xRadius: 2, yRadius: 2)
            clip.addClip()
            img.draw(in: ir.insetBy(dx: 1, dy: (rowH - iconSz) / 2))
            NSGraphicsContext.restoreGraphicsState()

            let nameAttrs: [NSAttributedString.Key: Any] = [
                .font: NSFont.systemFont(ofSize: 12 * textZoom, weight: .regular),
                .foregroundColor: i == selection || marked.contains(i)
                    ? config.colors.text : config.colors.text.withAlphaComponent(0.85),
            ]
            var tx = ir.maxX + 6
            if e.isDir && e.name != ".." { tx += 4 }
            let name = e.name as NSString
            let avail = w - tx - 8 - (e.trailingWidth > 0 ? trailW(e) + Self.trailingInset + 4 : 0)
            let lineH = (15 * textZoom).rounded(.up)
            name.draw(with: NSRect(x: tx, y: r.minY + (rowH - lineH) / 2, width: max(20, avail), height: lineH),
                      options: [.truncatesLastVisibleLine, .usesLineFragmentOrigin],
                      attributes: nameAttrs)
            if e.isDir && e.name != ".." {
                let dirAttrs: [NSAttributedString.Key: Any] = [
                    .font: NSFont.systemFont(ofSize: 11 * textZoom, weight: .bold),
                    .foregroundColor: config.colors.tone(.accent2),
                ]
                let d = (e.name as NSString).size(withAttributes: nameAttrs)
                let g = "/" as NSString
                g.draw(at: NSPoint(x: tx + d.width + 2, y: r.minY + (rowH - 14 * textZoom) / 2), withAttributes: dirAttrs)
            }
            if e.trailingWidth > 0 {
                let szAttrs: [NSAttributedString.Key: Any] = [
                    .font: NSFont.systemFont(ofSize: 10 * textZoom, weight: .regular),
                    .foregroundColor: config.colors.dim,
                ]
                let s = e.trailingText as NSString
                s.draw(at: NSPoint(x: w - trailW(e) - Self.trailingInset, y: r.minY + (rowH - 12 * textZoom) / 2),
                       withAttributes: szAttrs)
            }
        }
    }

    override func mouseDown(with e: NSEvent) {
        let p = convert(e.locationInWindow, from: nil)
        let idx = Int(p.y / rowH)
        dragStart = nil
        renameClick?.cancel()
        renameClick = nil
        collapseOnUp = nil
        guard rows.indices.contains(idx) else { marked = []; return }
        let mods = e.modifierFlags
        if mods.contains(.shift) {
            extendSelection(to: idx)
            return
        }
        if mods.contains(.command) {
            guard rows[idx].name != ".." else { return }
            var m = marked.isEmpty && rows.indices.contains(selection) && rows[selection].name != ".."
                ? [selection] : marked
            if m.contains(idx) { m.remove(idx) } else { m.insert(idx) }
            if m.contains(idx) {
                selection = idx
            } else if let first = m.min() {
                selection = first
            }
            marked = m.count > 1 ? m : []
            anchor = selection
            onSelect?(selection)
            return
        }
        if marked.count > 1, marked.contains(idx) {
            selection = idx
            onSelect?(idx)
            if e.clickCount >= 2 { marked = []; onOpen?(idx); return }
            dragStart = (p, idx)
            collapseOnUp = idx
            return
        }
        let onName = e.clickCount == 1 && idx == selection && marked.isEmpty && onRename != nil
            && rows[idx].name != ".." && nameTextRect(idx).contains(p)
        marked = []
        anchor = idx
        selection = idx
        onSelect?(idx)
        if e.clickCount >= 2 { onOpen?(idx); return }
        if rows[idx].name != ".." { dragStart = (p, idx) }
        if onName {
            let path = rows[idx].path
            let work = DispatchWorkItem { [weak self] in
                guard let self else { return }
                self.renameClick = nil
                guard self.selection == idx, self.rows.indices.contains(idx),
                      self.rows[idx].path == path, self.window?.isKeyWindow == true else { return }
                self.onRename?(idx)
            }
            renameClick = work
            DispatchQueue.main.asyncAfter(deadline: .now() + NSEvent.doubleClickInterval, execute: work)
        }
    }

    private var renameClick: DispatchWorkItem?
    private var collapseOnUp: Int?
    private func nameTextRect(_ i: Int) -> NSRect {
        var r = nameRect(i)
        let w = (rows[i].name as NSString).size(withAttributes: [.font: NSFont.systemFont(ofSize: 12 * textZoom)]).width
        r.size.width = min(r.width, ceil(w) + 6)
        return r
    }

    override func mouseDragged(with e: NSEvent) {
        guard let start = dragStart, rows.indices.contains(start.row) else { return }
        let p = convert(e.locationInWindow, from: nil)
        guard hypot(p.x - start.point.x, p.y - start.point.y) > 4 else { return }
        dragStart = nil
        collapseOnUp = nil
        renameClick?.cancel()
        renameClick = nil
        let entry = rows[start.row]
        let more = marked.contains(start.row)
            ? marked.sorted().filter { $0 != start.row && rows[$0].name != ".." }.map { rows[$0].path } : []
        let img = Self.dragImage(entry, count: more.count + 1, width: min(bounds.width, 320),
                                 rowH: rowH, config: config)
        FileDrag.begin(path: entry.path, more: more, image: img,
                       frame: NSRect(origin: rowRect(start.row).origin, size: img.size),
                       view: self, event: e, source: self)
    }

    override func mouseUp(with e: NSEvent) {
        dragStart = nil
        if let i = collapseOnUp {
            collapseOnUp = nil
            marked = []
            anchor = i
        }
    }

    private static func dragImage(_ e: PopupFileBrowser.Entry, count: Int = 1, width: CGFloat, rowH: CGFloat,
                                  config: PopupConfig) -> NSImage {
        let icon = e.icon ?? NSWorkspace.shared.icon(forFile: e.path)
        let attrs: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: 12),
            .foregroundColor: config.colors.text,
        ]
        let label = count > 1 ? "\(e.name)  +\(count - 1)" : e.name
        let nameW = min((label as NSString).size(withAttributes: attrs).width, width - 40)
        let size = NSSize(width: 10 + iconSize + 6 + nameW + 10, height: rowH)
        return NSImage(size: size, flipped: true) { _ in
            config.colors.highlight.withAlphaComponent(0.85).setFill()
            NSBezierPath(roundedRect: NSRect(origin: .zero, size: size).insetBy(dx: 1, dy: 1),
                         xRadius: 5, yRadius: 5).fill()
            FileDrag.drawFlipped(icon, in: NSRect(x: 10, y: (rowH - iconSize) / 2,
                                                  width: iconSize, height: iconSize))
            (label as NSString).draw(with: NSRect(x: 10 + iconSize + 6, y: (rowH - 15) / 2,
                                                  width: nameW, height: 15),
                                      options: [.truncatesLastVisibleLine, .usesLineFragmentOrigin],
                                      attributes: attrs)
            return true
        }
    }

    func draggingSession(_ session: NSDraggingSession,
                         sourceOperationMaskFor context: NSDraggingContext) -> NSDragOperation {
        FileDrag.sourceMask(context)
    }

    func draggingSession(_ session: NSDraggingSession, endedAt screenPoint: NSPoint,
                         operation: NSDragOperation) {
        if operation.contains(.move) || operation.contains(.delete) {
            onFilesChanged?("moved out")
        }
    }

    private func dropTarget(_ info: NSDraggingInfo) -> (dir: String, row: Int?)? {
        let p = convert(info.draggingLocation, from: nil)
        let idx = Int(p.y / rowH)
        if p.y >= 0, rows.indices.contains(idx), rows[idx].isDir {
            return ((rows[idx].path as NSString).standardizingPath, idx)
        }
        guard let d = dropDirectory?() else { return nil }
        return ((d as NSString).standardizingPath, nil)
    }

    private func updateDrop(_ info: NSDraggingInfo) -> NSDragOperation {
        guard let t = dropTarget(info) else {
            dropRow = nil; dropWhole = false
            return []
        }
        let op = FileDrag.operation(info, into: t.dir)
        dropRow = op.isEmpty ? nil : t.row
        dropWhole = !op.isEmpty && t.row == nil
        return op
    }

    override func draggingEntered(_ info: NSDraggingInfo) -> NSDragOperation { updateDrop(info) }
    override func draggingUpdated(_ info: NSDraggingInfo) -> NSDragOperation {
        autoscroll(with: NSApp.currentEvent ?? NSEvent())
        return updateDrop(info)
    }
    override func draggingExited(_ info: NSDraggingInfo?) { dropRow = nil; dropWhole = false }
    override func draggingEnded(_ info: NSDraggingInfo) { dropRow = nil; dropWhole = false }

    override func performDragOperation(_ info: NSDraggingInfo) -> Bool {
        defer { dropRow = nil; dropWhole = false }
        guard let t = dropTarget(info) else { return false }
        let op = FileDrag.operation(info, into: t.dir)
        guard !op.isEmpty else { return false }
        FileDrag.perform(info, into: t.dir, op: op) { [weak self] msg in
            self?.onFilesChanged?(msg)
        }
        return true
    }

    override func rightMouseDown(with e: NSEvent) {
        let p = convert(e.locationInWindow, from: nil)
        let idx = Int(p.y / rowH)
        let menu = NSMenu()
        menu.autoenablesItems = false
        func can(_ a: Action) -> Bool { onAction != nil && (canPerform?(a) ?? true) }
        func add(_ title: String, _ a: Action) {
            guard can(a) else { return }
            let i = NSMenuItem(title: title, action: #selector(rowAction(_:)), keyEquivalent: "")
            i.target = self
            i.representedObject = a
            menu.addItem(i)
        }
        func addFolderItems() {
            add("New Folder", .newFolder)
            add("New File", .newFile)
            add("Paste", .paste)
            add(hiddenShown?() == true ? "Hide Hidden Files" : "Show Hidden Files", .toggleHidden)
        }
        guard p.y >= 0, rows.indices.contains(idx) else {
            marked = []
            addFolderItems()
            if !menu.items.isEmpty { NSMenu.popUpContextMenu(menu, with: e, for: self) }
            return
        }
        if !marked.contains(idx) { marked = [] }
        selection = idx
        onSelect?(idx)
        let many = marked.count > 1
        let real = rows[idx].name != ".."
        if !many {
            menu.addItem(menuItem("Open", #selector(rowOpen(_:)), idx))
            if real, !rows[idx].isDir { addOpenWith(menu, idx) }
            menu.addItem(menuItem("Open in Notes", #selector(rowOpenInNotes(_:)), idx))
            if real { add("Quick Look", .quickLook) }
            menu.addItem(NSMenuItem.separator())
            menu.addItem(menuItem("Copy Path", #selector(rowCopyPath(_:)), idx))
        }
        if real {
            add(many ? "Copy \(marked.count) Items" : "Copy", .copy)
            add(many ? "Cut \(marked.count) Items" : "Cut", .cut)
            add("Duplicate", .duplicate)
        }
        if !many {
            menu.addItem(NSMenuItem.separator())
            menu.addItem(menuItem("Reveal in Finder", #selector(rowRevealInFinder(_:)), idx))
            if real { add("Show in Enclosing Folder", .enclosing) }
            menu.addItem(menuItem("Open Terminal Here", #selector(rowOpenTerminal(_:)), idx))
        }
        addCompareItems(menu, idx, many: many, real: real)
        if real {
            menu.addItem(NSMenuItem.separator())
            if !many, onRename != nil { menu.addItem(menuItem("Rename…", #selector(rowRename(_:)), idx)) }
            add(many ? "Move \(marked.count) Items to Trash" : "Move to Trash", .trash)
        }
        let before = menu.items.count
        menu.addItem(NSMenuItem.separator())
        addFolderItems()
        if menu.items.count == before + 1 { menu.removeItem(at: before) }
        NSMenu.popUpContextMenu(menu, with: e, for: self)
    }

    private func addCompareItems(_ menu: NSMenu, _ idx: Int, many: Bool, real: Bool) {
        guard let compare = Self.onCompare, real else { return }
        var items: [NSMenuItem] = []
        func item(_ t: String, _ f: @escaping () -> Void) {
            let i = ClosureMenuItem(t, f)
            items.append(i)
        }
        func isDir(_ p: String) -> Bool {
            var d: ObjCBool = false
            return FileManager.default.fileExists(atPath: p, isDirectory: &d) && d.boolValue
        }
        if many {
            let sel = marked.sorted().compactMap { rows.indices.contains($0) ? rows[$0] : nil }
            if sel.count == 2, sel[0].isDir == sel[1].isDir { item("Compare") { compare(sel[0].path, sel[1].path) } }
        } else {
            let path = rows[idx].path
            if let pick = Self.comparePick, pick != path, isDir(pick) == rows[idx].isDir {
                item("Compare to “\((pick as NSString).lastPathComponent)”") {
                    Self.comparePick = nil
                    compare(pick, path)
                }
            }
            item("Select for Compare") { Self.comparePick = path }
        }
        guard !items.isEmpty else { return }
        menu.addItem(NSMenuItem.separator())
        items.forEach(menu.addItem)
    }

    @objc private func rowAction(_ sender: NSMenuItem) {
        guard let a = sender.representedObject as? Action else { return }
        onAction?(a)
    }

    private func addOpenWith(_ menu: NSMenu, _ idx: Int) {
        let url = URL(fileURLWithPath: rows[idx].path)
        var apps = NSWorkspace.shared.urlsForApplications(toOpen: url)
        guard !apps.isEmpty else { return }
        if let def = NSWorkspace.shared.urlForApplication(toOpen: url) {
            apps.removeAll { $0 == def }
            apps.insert(def, at: 0)
        }
        let sub = NSMenu()
        for app in apps.prefix(12) {
            let i = NSMenuItem(title: FileManager.default.displayName(atPath: app.path),
                               action: #selector(rowOpenWith(_:)), keyEquivalent: "")
            i.target = self
            i.representedObject = [url, app]
            let icon = NSWorkspace.shared.icon(forFile: app.path)
            icon.size = NSSize(width: 16, height: 16)
            i.image = icon
            sub.addItem(i)
        }
        let item = NSMenuItem(title: "Open With", action: nil, keyEquivalent: "")
        item.submenu = sub
        menu.addItem(item)
    }

    @objc private func rowOpenWith(_ sender: NSMenuItem) {
        guard let pair = sender.representedObject as? [URL], pair.count == 2 else { return }
        NSWorkspace.shared.open([pair[0]], withApplicationAt: pair[1],
                                configuration: NSWorkspace.OpenConfiguration())
    }

    private func menuItem(_ title: String, _ sel: Selector, _ idx: Int) -> NSMenuItem {
        let i = NSMenuItem(title: title, action: sel, keyEquivalent: "")
        i.target = self
        i.representedObject = idx
        return i
    }

    @objc private func rowOpenInNotes(_ sender: NSMenuItem) {
        guard let idx = sender.representedObject as? Int else { return }
        onOpenInNotes?(idx)
    }

    @objc private func rowCopyPath(_ sender: NSMenuItem) {
        guard let idx = sender.representedObject as? Int else { return }
        onCopyPath?(idx)
    }

    @objc private func rowOpen(_ sender: NSMenuItem) {
        guard let idx = sender.representedObject as? Int else { return }
        onOpen?(idx)
    }

    @objc private func rowOpenTerminal(_ sender: NSMenuItem) {
        guard let idx = sender.representedObject as? Int else { return }
        onOpenTerminal?(idx)
    }

    @objc private func rowRename(_ sender: NSMenuItem) {
        guard let idx = sender.representedObject as? Int else { return }
        onRename?(idx)
    }

    @objc private func rowRevealInFinder(_ sender: NSMenuItem) {
        guard let idx = sender.representedObject as? Int, rows.indices.contains(idx) else { return }
        NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: rows[idx].path)])
    }

    override func keyDown(with e: NSEvent) {
        let mods = e.modifierFlags
        if !mods.intersection([.command, .control]).isEmpty {
            super.keyDown(with: e)
            return
        }
        if mods.contains(.shift), e.keyCode == 126 || e.keyCode == 125 {
            extendSelection(to: min(max(0, rows.count - 1), max(0, selection + (e.keyCode == 125 ? 1 : -1))))
            return
        }
        switch e.keyCode {
        case 126:
            moveSelection(-1)
        case 125:
            moveSelection(1)
        case 115, 116:
            moveSelection(e.keyCode == 115 ? -rows.count : -pageRows)
        case 119, 121:
            moveSelection(e.keyCode == 119 ? rows.count : pageRows)
        case 49 where onAction != nil:
            onAction?(.quickLook)
        case 36:
            onOpen?(selection)
        case 120 where onRename != nil:
            onRename?(selection)
        case 123:
            onParent?()
        case 124:
            onOpen?(selection)
        case 53:
            super.keyDown(with: e)
        default:
            if let chars = e.charactersIgnoringModifiers, !chars.isEmpty {
                onFocusSearch?(chars)
            } else {
                super.keyDown(with: e)
            }
        }
    }

    func moveSelection(_ delta: Int) {
        guard !rows.isEmpty else { return }
        marked = []
        selection = min(max(0, rows.count - 1), max(0, selection + delta))
        anchor = selection
        onSelect?(selection)
    }
    private var pageRows: Int { max(1, Int(visibleRect.height / rowH) - 1) }
}

enum FileDrag {
    private static let queue = DispatchQueue(label: "file-drop", qos: .userInitiated)
    static var onFileOp: ((_ from: String?, _ to: String) -> Void)?

    static var onDragOut: (([String]) -> Void)?

    static func begin(path: String, more: [String] = [], image: NSImage, frame: NSRect, view: NSView,
                      event: NSEvent, source: NSDraggingSource) {
        onDragOut?([path] + more)
        let item = NSDraggingItem(pasteboardWriter: URL(fileURLWithPath: path) as NSURL)
        item.setDraggingFrame(frame, contents: image)
        let rest = more.map { p -> NSDraggingItem in
            let i = NSDraggingItem(pasteboardWriter: URL(fileURLWithPath: p) as NSURL)
            i.setDraggingFrame(frame, contents: nil)
            return i
        }
        let session = view.beginDraggingSession(with: [item] + rest, event: event, source: source)
        session.animatesToStartingPositionsOnCancelOrFail = true
    }

    static func sourceMask(_ context: NSDraggingContext) -> NSDragOperation {
        context == .outsideApplication ? [.copy, .move, .link, .generic] : [.copy, .move, .generic]
    }

    static func drawFlipped(_ img: NSImage, in r: NSRect) {
        img.draw(in: r, from: .zero, operation: .sourceOver, fraction: 1,
                 respectFlipped: true, hints: nil)
    }

    static func fileURLs(_ info: NSDraggingInfo) -> [URL] {
        info.draggingPasteboard.readObjects(forClasses: [NSURL.self],
                                            options: [.urlReadingFileURLsOnly: true]) as? [URL] ?? []
    }

    static func promises(_ info: NSDraggingInfo) -> [NSFilePromiseReceiver] {
        info.draggingPasteboard.readObjects(forClasses: [NSFilePromiseReceiver.self],
                                            options: nil) as? [NSFilePromiseReceiver] ?? []
    }

    private static func sameVolume(_ a: URL, _ dir: String) -> Bool {
        let k: Set<URLResourceKey> = [.volumeIdentifierKey]
        guard let va = try? a.resourceValues(forKeys: k).volumeIdentifier as? NSObject,
              let vb = try? URL(fileURLWithPath: dir).resourceValues(forKeys: k).volumeIdentifier as? NSObject
        else { return false }
        return va.isEqual(vb)
    }

    static func operation(_ info: NSDraggingInfo, into dir: String) -> NSDragOperation {
        let mask = info.draggingSourceOperationMask
        let canCopy = mask.contains(.copy) || mask.contains(.generic)
        let urls = fileURLs(info)
        if urls.isEmpty { return !promises(info).isEmpty && canCopy ? .copy : [] }
        for u in urls {
            let p = u.standardizedFileURL.path
            if dir == p || dir.hasPrefix(p + "/") { return [] }
        }
        let mods = NSEvent.modifierFlags
        let move = mods.contains(.option) ? false
            : mods.contains(.command) ? true
            : sameVolume(urls[0], dir)
        if move, mask.contains(.move) || mask.contains(.generic) {
            let home = urls.allSatisfy { $0.standardizedFileURL.deletingLastPathComponent().path == dir }
            return home ? [] : .move
        }
        return canCopy ? .copy : []
    }

    static func perform(_ info: NSDraggingInfo, into dir: String, op: NSDragOperation,
                        done: @escaping (String) -> Void) {
        let dest = URL(fileURLWithPath: dir, isDirectory: true)
        let shown = (dir as NSString).abbreviatingWithTildeInPath
        let urls = fileURLs(info)
        if urls.isEmpty {
            let ops = OperationQueue()
            ops.qualityOfService = .userInitiated
            for r in promises(info) {
                r.receivePromisedFiles(atDestination: dest, options: [:], operationQueue: ops) { url, err in
                    let msg = err.map { "drop failed: \($0.localizedDescription)" }
                        ?? "received \(url.lastPathComponent) → \(shown)"
                    DispatchQueue.main.async { done(msg) }
                }
            }
            return
        }
        let move = op == .move
        queue.async {
            let out = FileOps.transfer(urls, into: dir, move: move)
            for c in out.changes { onFileOp?(c.from, c.to) }
            let ok = out.changes.count
            let verb = move ? "moved" : "copied"
            let what = urls.count == 1 ? urls[0].lastPathComponent : "\(ok) items"
            let msg = out.failed.map { "\(verb) \(ok)/\(urls.count) — \($0)" } ?? "\(verb) \(what) → \(shown)"
            DispatchQueue.main.async { done(msg) }
        }
    }
}

final class FileDragImageView: NSImageView, NSDraggingSource {
    override var acceptsFirstResponder: Bool { image != nil }
    override var focusRingType: NSFocusRingType { get { .none } set {} }
    var path: String?
    private var downAt: NSPoint?

    override init(frame: NSRect) {
        super.init(frame: frame)
        unregisterDraggedTypes()
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) not supported") }
    override var mouseDownCanMoveWindow: Bool { false }

    override func mouseDown(with e: NSEvent) { downAt = path == nil ? nil : e.locationInWindow }
    override func mouseUp(with e: NSEvent) { downAt = nil }
    override func mouseDragged(with e: NSEvent) {
        guard let start = downAt, let path, let image else { return }
        let p = e.locationInWindow
        guard hypot(p.x - start.x, p.y - start.y) > 4 else { return }
        downAt = nil
        let s = image.size
        let k = min(1, 160 / max(1, max(s.width, s.height)))
        let sz = NSSize(width: max(1, s.width * k), height: max(1, s.height * k))
        let thumb = NSImage(size: sz, flipped: false) { r in
            image.draw(in: r, from: .zero, operation: .sourceOver, fraction: 0.85)
            return true
        }
        let at = convert(p, from: nil)
        FileDrag.begin(path: path, image: thumb,
                       frame: NSRect(x: at.x - sz.width / 2, y: at.y - sz.height / 2,
                                     width: sz.width, height: sz.height),
                       view: self, event: e, source: self)
    }

    func draggingSession(_ session: NSDraggingSession,
                         sourceOperationMaskFor context: NSDraggingContext) -> NSDragOperation {
        FileDrag.sourceMask(context)
    }
}

final class BrowserSearchField: NSTextField {
    override class var cellClass: AnyClass? {
        get { PopupSearchFieldCell.self }
        set {}
    }
}

final class PaneSplitter: NSView {
    var onFractionChange: ((CGFloat) -> Void)?
    override var isFlipped: Bool { true }
    override func resetCursorRects() {
        super.resetCursorRects()
        addCursorRect(bounds, cursor: .resizeLeftRight)
    }
    override func mouseDown(with e: NSEvent) {
        NSCursor.closedHand.push()
    }
    override func mouseDragged(with e: NSEvent) {
        guard let sv = superview else { return }
        let x = sv.convert(e.locationInWindow, from: nil).x
        let frac = min(0.8, max(0.2, x / max(1, sv.bounds.width)))
        onFractionChange?(frac)
    }
    override func mouseUp(with e: NSEvent) {
        NSCursor.pop()
    }
}

final class DocxTextExtractor: NSObject, XMLParserDelegate {
    private(set) var text = ""
    private var inText = false
    func parser(_ parser: XMLParser, didStartElement elementName: String,
                namespaceURI: String?, qualifiedName qName: String?,
                attributes attributeDict: [String: String] = [:]) {
        switch elementName {
        case "w:p": text += "\n"
        case "w:t": inText = true
        default: break
        }
    }
    func parser(_ parser: XMLParser, foundCharacters string: String) {
        if inText { text += string }
    }
    func parser(_ parser: XMLParser, didEndElement elementName: String,
                namespaceURI: String?, qualifiedName qName: String?) {
        if elementName == "w:t" { inText = false }
    }
}

final class PreviewTextView: NSTextView {
    override func keyDown(with e: NSEvent) {
        let mods = e.modifierFlags.intersection([.command, .control, .option, .shift])
        if !isEditable, e.keyCode == 49, mods.isSubset(of: .shift) {
            mods.contains(.shift) ? scrollPageUp(nil) : scrollPageDown(nil)
            return
        }
        super.keyDown(with: e)
    }
}

final class PopupFileBrowser: NSView, NSTextFieldDelegate, QLPreviewPanelDataSource, QLPreviewPanelDelegate {
    var config: PopupConfig
    var onOpen: ((String) -> Void)?
    var onDirChange: ((String) -> Void)?
    var onCopyPath: ((String) -> Void)?
    var onOpenInNotes: ((String) -> Void)?
    var onStatus: ((String) -> Void)?
    var onCopied: ((String) -> Void)?
    var onOpenTerminal: ((String) -> Void)?
    var onSortChange: ((String, Bool) -> Void)?

    struct Entry {
        let name: String
        let path: String
        let isDir: Bool
        let size: Int
        var created = Date.distantPast
        var modified = Date.distantPast
        var icon: NSImage?
        var trailingText: String = ""
        var trailingWidth: CGFloat = 0
    }

    private static var iconCache: [String: NSImage] = [:]

    enum SortKey: String, CaseIterable {
        case name, modified, created, size, kind
        var label: String {
            switch self {
            case .name: return "Name"
            case .modified: return "Date Modified"
            case .created: return "Date Created"
            case .size: return "Size"
            case .kind: return "Kind"
            }
        }
        var short: String {
            switch self {
            case .name: return "Name"
            case .modified: return "Modified"
            case .created: return "Created"
            case .size: return "Size"
            case .kind: return "Kind"
            }
        }
        var naturalDescending: Bool { self == .modified || self == .created || self == .size }
    }
    private var sortKey: SortKey
    private var sortDescending: Bool

    enum QueryMode {
        case all
        case terminal(String)
        case local(String)
        case dir(String, String)
        case recursive(String, String)
    }
    private var mode: QueryMode = .all
    private var hiddenAll: [Entry]?
    private var dirCache: (String, Bool, [Entry])?
    private var searchGen = 0
    private var searchProcess: Process?
    private var searchWork: DispatchWorkItem?

    private let favURL: URL
    private var pinnedFavorites: [String] = []
    private let staticFavorites: [String]
    public struct VirtualList {
        public var title: String
        public var symbol: String
        public var status: String
        public var provider: () -> [(path: String, at: Date, note: String?)]
        public init(title: String, symbol: String, status: String,
                    provider: @escaping () -> [(path: String, at: Date, note: String?)]) {
            self.title = title
            self.symbol = symbol
            self.status = status
            self.provider = provider
        }
    }
    public var virtualLists: [VirtualList] = [] {
        didSet { rebuildPills() }
    }
    private(set) var virtualIndex: Int?
    var inRecent: Bool { virtualIndex != nil }
    private var shownFavorites: [String] = []
    private(set) var cwd: String
    var whereText: String {
        if let v = virtualIndex, virtualLists.indices.contains(v) { return virtualLists[v].title }
        return (cwd as NSString).abbreviatingWithTildeInPath
    }
    private var all: [Entry] = []
    private var rows: [Entry] = []
    private var selection = 0
    private var query = ""

    private let searchField = BrowserSearchField()
    private let parentButton: ThemeButton
    private let starButton: ThemeButton
    private let sortButton: ThemeButton
    private let orderButton: ThemeButton
    private let statusLine = NSTextField(labelWithString: "")
    private let listPane: FileListPane
    private let listScroll = NSScrollView()
    private let splitter = PaneSplitter()
    private var splitFraction: CGFloat = 0.56
    private let previewScroll = NSScrollView()
    private let previewText = PreviewTextView()
    private let previewImage = FileDragImageView(frame: .zero)
    private let previewHint = NSTextField(labelWithString: "")
    private let previewList: FileListPane
    private let previewListScroll = NSScrollView()
    private var favPills: [ThemeButton] = []
    private var sidebar: PopupTabsBar?
    private var sidebarWide: CGFloat = 0
    private var sidebarW: CGFloat {
        get { sidebar?.width(expanded: sidebarWide) ?? sidebarWide }
        set { sidebarWide = newValue }
    }
    public var onSidebarWidthChange: ((CGFloat) -> Void)?
    private lazy var places: [(title: String, symbol: String, path: String)] = {
        let h = NSHomeDirectory()
        return [("Home", "house", h), ("Desktop", "menubar.dock.rectangle", h + "/Desktop"),
                ("Documents", "doc", h + "/Documents"), ("Downloads", "arrow.down.circle", h + "/Downloads")]
            .filter { FileManager.default.fileExists(atPath: $0.2) }
    }()
    public func useSidebar(width: CGFloat) {
        guard width > 0, sidebar == nil else { return }
        sidebarW = width
        var cfg = config
        cfg.tabsAddButton = false
        let bar = PopupTabsBar(config: cfg)
        bar.vertical = true
        bar.closable = false
        bar.sectionTitle = "Places"
        bar.collapseKey = "files"
        bar.onCollapse = { [weak self] _ in self?.layoutPanes() }
        bar.maxPinnedShown = 12
        bar.rowIcon = { [weak self] i in
            guard let self else { return nil }
            if i < self.virtualLists.count { return self.virtualLists[i].symbol }
            let k = i - self.virtualLists.count
            return self.places.indices.contains(k) ? self.places[k].symbol : "folder"
        }
        bar.onSelect = { [weak self] i in
            guard let self else { return }
            if i < self.virtualLists.count { self.showVirtual(i); return }
            let k = i - self.virtualLists.count
            if self.places.indices.contains(k) { self.cd(self.places[k].path) }
        }
        bar.onPinned = { [weak self] path in
            guard let self, FileManager.default.fileExists(atPath: path) else { return }
            self.cd(path)
        }
        bar.pinnedMenu = { [weak self] path in
            let m = NSMenu()
            m.addItem(menuItem("Open") { self?.cd(path) })
            m.addItem(menuItem("Reveal in Finder") {
                NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)])
            })
            m.addItem(menuItem("Copy Path") {
                copyText(path)
                self?.onCopied?((path as NSString).abbreviatingWithTildeInPath)
            })
            if self?.pinnedFavorites.contains(path) == true {
                m.addItem(.separator())
                m.addItem(menuItem("Unpin") { self?.unpin(path) })
            }
            return m
        }
        bar.onWidthChange = { [weak self] w, done in
            guard let self else { return }
            self.sidebarW = w.rounded()
            self.needsLayout = true
            if done { self.onSidebarWidthChange?(self.sidebarW) }
        }
        addSubview(bar)
        sidebar = bar
        rebuildPills()
    }
    private func unpin(_ path: String) {
        pinnedFavorites.removeAll { $0 == path }
        saveFavorites()
        rebuildPills()
        updateStarTitle()
        onFavoritesChanged?()
    }
    private func syncSidebar() {
        guard let bar = sidebar else { return }
        bar.titles = virtualLists.map(\.title) + places.map(\.title)
        bar.pinned = shownFavorites
        bar.pinnedSelected = inRecent ? nil : cwd
        if let v = virtualIndex { bar.selected = v }
        else if let k = places.firstIndex(where: { $0.path == cwd }) { bar.selected = virtualLists.count + k }
        else { bar.selected = -1 }
        bar.needsDisplay = true
    }
    private static let imageExts = Set(["png", "jpg", "jpeg", "gif", "heic", "webp", "tif", "tiff", "pdf"])
    private static let textLimit = 262144
    private enum PreviewContent { case image(NSImage), text(String), hint(String) }
    private var previewGen = 0
    private let previewLatest = PreviewGen()
    private final class PreviewGen: @unchecked Sendable {
        private let lock = NSLock()
        private var v = 0
        var value: Int {
            get { lock.lock(); defer { lock.unlock() }; return v }
            set { lock.lock(); v = newValue; lock.unlock() }
        }
    }
    private static let previewQueue = DispatchQueue(label: "file-preview", qos: .userInitiated)
    private static var previewCache: [String: PreviewContent] = [:]
    private static var previewCacheOrder: [String] = []

    enum FocusPart { case filter, list, preview }
    private let partRing = PopupPassThroughView()
    private var focusObservation: NSKeyValueObservation?
    private var keyObservers: [NSObjectProtocol] = []
    private(set) var focusedPart: FocusPart?

    var listView: FileListPane { listPane }
    var searchView: NSTextField { searchField }

    func setBackground(_ c: NSColor) {
        config.fileBrowserBackground = c
        layer?.backgroundColor = c.cgColor
        needsDisplay = true
    }

    func retheme() {
        let c = config.colors
        searchField.textColor = c.text
        searchField.placeholderAttributedString = NSAttributedString(
            string: "filter…",
            attributes: [.font: NSFont.systemFont(ofSize: 11), .foregroundColor: c.dim])
        searchField.layer?.backgroundColor = ButtonStyle.inputFill(c).cgColor
        searchField.layer?.borderColor = ButtonStyle.inputStroke(c).cgColor
        updatePartFocus()
        previewText.textColor = c.text
        previewText.selectedTextAttributes = ButtonStyle.selection(c)
        previewHint.textColor = c.dim
        statusLine.textColor = c.dim
        splitter.layer?.backgroundColor = c.hairline.cgColor
        setBackground(config.fileBrowserBackground)
        listPane.needsDisplay = true
        previewList.needsDisplay = true
        needsDisplay = true
    }

    init(config: PopupConfig, startDir: String, favoritesURL: URL? = nil,
         staticFavorites: [String] = []) {
        self.config = config
        self.cwd = (startDir as NSString).standardizingPath
        self.staticFavorites = staticFavorites
        let home = NSHomeDirectory()
        self.favURL = favoritesURL ?? URL(fileURLWithPath: home + "/.cache/kitchen-sink/files-favorites.json")
        self.parentButton = ThemeButton(config: config, title: "", symbol: "arrow.up")
        self.starButton = ThemeButton(config: config, title: "Pin", symbol: "star")
        self.sortKey = SortKey(rawValue: config.browserSort.lowercased()) ?? .name
        self.sortDescending = config.browserSortDescending
        self.sortButton = ThemeButton(config: config, title: "", symbol: "line.3.horizontal.decrease")
        self.sortButton.chevron = true
        self.orderButton = ThemeButton(config: config, title: "", symbol: "arrow.up")
        self.starButton.segment = .first
        self.sortButton.segment = .middle
        self.orderButton.segment = .last
        self.listPane = FileListPane(config: config)
        self.previewList = FileListPane(config: config)
        super.init(frame: .zero)
        wantsLayer = true
        layer?.backgroundColor = config.fileBrowserBackground.cgColor

        listPane.onSelect = { [weak self] i in
            guard let self else { return }
            self.selection = i
            self.previewSelection()
            self.scrollSelectionVisible()
            let n = self.listPane.marked.count
            if n > 1 { self.setStatus("\(n) selected") }
            self.refreshQuickLook()
        }
        listPane.onAction = { [weak self] a in self?.perform(a) }
        listPane.canPerform = { [weak self] a in self?.canPerform(a) ?? false }
        listPane.hiddenShown = { [weak self] in self?.showHidden ?? false }
        listPane.onOpen = { [weak self] i in
            self?.openIndex(i)
        }
        listPane.onParent = { [weak self] in
            self?.cdParent()
        }
        listPane.onCopyPath = { [weak self] i in
            guard let self, self.rows.indices.contains(i) else { return }
            let p = self.rows[i].path
            self.onCopyPath?(p)
            self.onStatus?("copied \(p)")
            self.onCopied?((p as NSString).abbreviatingWithTildeInPath)
        }
        listPane.onOpenInNotes = { [weak self] i in
            guard let self, self.rows.indices.contains(i) else { return }
            self.onOpenInNotes?(self.rows[i].path)
        }
        listPane.onOpenTerminal = { [weak self] i in
            guard let self, self.rows.indices.contains(i) else { return }
            self.openTerminal(self.terminalDir(for: self.rows[i]))
        }
        listPane.onRename = { [weak self] i in
            self?.beginRename(i)
        }
        listPane.dropDirectory = { [weak self] in
            guard let self, !self.inRecent else { return nil }
            switch self.mode {
            case .all, .local: return self.cwd
            default: return nil
            }
        }
        listPane.onFilesChanged = { [weak self] msg in
            self?.reload()
            self?.setStatus(msg)
        }
        listPane.onFocusSearch = { [weak self] chars in
            guard let self else { return }
            if !self.window!.makeFirstResponder(self.searchField) { return }
            if let ed = self.searchField.currentEditor() {
                let range = ed.selectedRange
                let ns = self.searchField.stringValue as NSString
                self.searchField.stringValue = ns.replacingCharacters(in: range, with: chars)
                self.query = self.searchField.stringValue
                self.refilter()
                ed.selectedRange = NSRange(location: range.location + (chars as NSString).length, length: 0)
            } else {
                self.searchField.stringValue += chars
                self.query = self.searchField.stringValue
                self.refilter()
            }
        }

        searchField.delegate = self
        searchField.isBezeled = false
        searchField.drawsBackground = false
        searchField.focusRingType = .none
        searchField.font = NSFont.systemFont(ofSize: 11)
        searchField.textColor = config.colors.text
        searchField.placeholderAttributedString = NSAttributedString(
            string: "filter…",
            attributes: [.font: NSFont.systemFont(ofSize: 11), .foregroundColor: config.colors.dim])
        searchField.wantsLayer = true
        (searchField.cell as? PopupSearchFieldCell)?.hInset = 7
        searchField.layer?.backgroundColor = ButtonStyle.inputFill(config.colors).cgColor
        searchField.layer?.cornerRadius = config.buttonRadius
        searchField.layer?.borderWidth = 1
        searchField.layer?.borderColor = ButtonStyle.inputStroke(config.colors).cgColor

        parentButton.onClick = { [weak self] in self?.cdParent() }
        starButton.onClick = { [weak self] in self?.toggleStar() }
        sortButton.onClick = { [weak self] in self?.showSortMenu() }
        orderButton.onClick = { [weak self] in self?.toggleSortOrder() }
        parentButton.toolTip = "Parent folder"
        starButton.toolTip = "Pin this folder to the favorites row"
        sortButton.toolTip = "Sort by name, date modified, date created, size or kind"
        orderButton.toolTip = "Toggle ascending / descending"
        updateSortTitle()
        searchField.toolTip = """
            Filter this folder, or type a path (~/notes/todo) to look inside it — \
            ⇥ completes. Wildcards: *.md, .* (dotfiles), ~/src/**/*.swift searches \
            subfolders. Type "term" + ↵ to open a terminal here.
            """

        statusLine.font = NSFont.systemFont(ofSize: 10.5)
        statusLine.textColor = config.colors.dim
        statusLine.lineBreakMode = .byTruncatingMiddle
        statusLine.isSelectable = false

        previewText.isEditable = false
        previewText.isSelectable = true
        previewText.drawsBackground = false
        previewText.textContainerInset = NSSize(width: 8, height: 8)
        previewText.font = NSFont.monospacedSystemFont(ofSize: 12, weight: .regular)
        previewText.textColor = config.colors.text
        previewText.selectedTextAttributes = ButtonStyle.selection(config.colors)
        previewScroll.documentView = previewText
        previewScroll.hasVerticalScroller = true
        previewScroll.autohidesScrollers = true
        previewScroll.drawsBackground = false
        previewScroll.borderType = .noBorder

        previewImage.imageScaling = .scaleProportionallyUpOrDown
        previewImage.imageAlignment = .alignCenter
        previewImage.wantsLayer = true
        previewImage.layer?.backgroundColor = NSColor.clear.cgColor

        previewHint.font = NSFont.systemFont(ofSize: 12)
        previewHint.textColor = config.colors.dim
        previewHint.alignment = .center
        previewHint.isEditable = false
        previewHint.isSelectable = false

        splitter.wantsLayer = true
        splitter.layer?.backgroundColor = config.colors.hairline.cgColor
        splitter.onFractionChange = { [weak self] frac in
            guard let self else { return }
            self.splitFraction = frac
            self.layoutPanes()
        }

        listScroll.hasVerticalScroller = true
        listScroll.autohidesScrollers = true
        listScroll.drawsBackground = false
        listScroll.borderType = .noBorder
        listScroll.documentView = listPane

        previewListScroll.hasVerticalScroller = true
        previewListScroll.autohidesScrollers = true
        previewListScroll.drawsBackground = false
        previewListScroll.borderType = .noBorder
        previewListScroll.documentView = previewList
        previewList.onOpen = { [weak self] i in self?.previewOpen(i) }
        previewList.onParent = { [weak self] in self?.cdParent() }
        previewList.dropDirectory = { [weak self] in
            guard let self, self.rows.indices.contains(self.selection),
                  self.rows[self.selection].isDir else { return nil }
            return self.rows[self.selection].path
        }
        previewList.onFilesChanged = { [weak self] msg in
            self?.reload()
            self?.previewSelection()
            self?.setStatus(msg)
        }
        previewList.onCopyPath = { [weak self] i in
            guard let self, self.previewList.rows.indices.contains(i) else { return }
            let p = self.previewList.rows[i].path
            self.onCopyPath?(p)
            self.onStatus?("copied \(p)")
            self.onCopied?((p as NSString).abbreviatingWithTildeInPath)
        }
        previewList.onOpenInNotes = { [weak self] i in
            guard let self, self.previewList.rows.indices.contains(i) else { return }
            self.onOpenInNotes?(self.previewList.rows[i].path)
        }
        previewList.onOpenTerminal = { [weak self] i in
            guard let self, self.previewList.rows.indices.contains(i) else { return }
            self.openTerminal(self.terminalDir(for: self.previewList.rows[i]))
        }

        addSubview(parentButton)
        addSubview(searchField)
        addSubview(starButton)
        addSubview(sortButton)
        addSubview(orderButton)
        addSubview(statusLine)
        addSubview(listScroll)
        addSubview(splitter)
        addSubview(previewScroll)
        addSubview(previewImage)
        addSubview(previewHint)
        addSubview(previewListScroll)
        partRing.wantsLayer = true
        partRing.layer?.borderWidth = 2
        partRing.layer?.borderColor = ButtonStyle.focusStroke(config.colors).withAlphaComponent(0.9).cgColor
        partRing.layer?.cornerRadius = 5
        partRing.isHidden = true
        addSubview(partRing)

        loadFavorites()
        rebuildPills()
        reload()
        showCwdInFilter()
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) not supported") }
    override var isFlipped: Bool { true }

    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        needsLayout = true
    }

    override func layout() {
        super.layout()
        layoutPanes()
        updatePartFocus()
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        focusObservation = nil
        keyObservers.forEach { NotificationCenter.default.removeObserver($0) }
        keyObservers = []
        guard let win = window else { return }
        focusObservation = win.observe(\.firstResponder, options: [.new]) { [weak self] _, _ in
            DispatchQueue.main.async { self?.updatePartFocus() }
        }
        for n in [NSWindow.didBecomeKeyNotification, NSWindow.didResignKeyNotification] {
            keyObservers.append(NotificationCenter.default.addObserver(
                forName: n, object: win, queue: .main) { [weak self] _ in self?.updatePartFocus() })
        }
        updatePartFocus()
    }

    private func currentPart() -> FocusPart? {
        guard let win = window, win.isKeyWindow, !isHidden else { return nil }
        if searchField.currentEditor() != nil { return .filter }
        guard let v = win.firstResponder as? NSView else { return nil }
        if v.isDescendant(of: listScroll) { return .list }
        if v.isDescendant(of: previewScroll) || v.isDescendant(of: previewListScroll) { return .preview }
        return nil
    }

    func updatePartFocus() {
        let part = currentPart()
        focusedPart = part
        let c = config.colors
        searchField.layer?.borderWidth = part == .filter ? 2 : 1
        searchField.layer?.borderColor = (part == .filter ? ButtonStyle.focusStroke(c)
                                          : ButtonStyle.inputStroke(c)).cgColor
        partRing.isHidden = true
    }
    private func layoutPanes() {
        let full = bounds.width
        let left: CGFloat = sidebar != nil ? sidebarW + 4 : 0
        sidebar?.frame = NSRect(x: 0, y: 0, width: sidebarW, height: bounds.height)
        let w = full
        let toolbarY: CGFloat = 4
        let toolbarH: CGFloat = 24
        let x0: CGFloat = 8 + left
        parentButton.frame = NSRect(x: x0, y: toolbarY, width: toolbarH + 4, height: toolbarH)
        starButton.frame = NSRect(x: parentButton.frame.maxX + 10, y: toolbarY,
                                  width: max(starButton.fittingWidth(for: "Pin"),
                                             starButton.fittingWidth(for: "Pinned")),
                                  height: toolbarH)
        let sortW = SortKey.allCases.map { sortButton.fittingWidth(for: $0.short) }.max() ?? 80
        sortButton.frame = NSRect(x: starButton.frame.maxX, y: toolbarY, width: sortW, height: toolbarH)
        orderButton.frame = NSRect(x: sortButton.frame.maxX, y: toolbarY,
                                   width: max(orderButton.fittingWidth(for: "Asc"),
                                              orderButton.fittingWidth(for: "Desc")),
                                   height: toolbarH)
        searchField.frame = NSRect(x: orderButton.frame.maxX + 10, y: toolbarY,
                                   width: max(60, w - orderButton.frame.maxX - 10 - 8),
                                   height: toolbarH)
        let favY = toolbarY + toolbarH + 5
        let favH = layoutFavorites(from: favY)
        let listY = favY + favH + 4
        let bottomH: CGFloat = 18
        statusLine.frame = NSRect(x: 8 + left, y: bounds.height - bottomH + 1,
                                  width: max(0, w - 16 - left), height: bottomH - 3)
        let splitW: CGFloat = 6
        let splitX = min(max(splitFraction * w, left + 200), max(left + 200, w - 200))
        let contentH = max(0, bounds.height - listY - bottomH)
        listScroll.frame = NSRect(x: left, y: listY, width: splitX - left, height: contentH)
        splitter.frame = NSRect(x: splitX, y: listY, width: splitW, height: contentH)
        let previewX = splitX + splitW
        let previewW = max(0, w - previewX)
        previewScroll.frame = NSRect(x: previewX, y: listY, width: previewW, height: contentH)
        previewImage.frame = NSRect(x: previewX, y: listY, width: previewW, height: contentH)
        previewHint.frame = NSRect(x: previewX, y: listY, width: previewW, height: contentH)
        previewListScroll.frame = NSRect(x: previewX, y: listY, width: previewW, height: contentH)
        layoutListDocument()
        layoutPreviewListDocument()
    }
    var textZoom: CGFloat = 1 {
        didSet {
            guard oldValue != textZoom else { return }
            listPane.textZoom = textZoom
            previewList.textZoom = textZoom
            layoutListDocument()
            layoutPreviewListDocument()
            scrollSelectionVisible()
        }
    }
    private func layoutListDocument() {
        let clipH = max(0, listScroll.bounds.height)
        let docH = max(clipH, CGFloat(listPane.rows.count) * listPane.rowHeight)
        listPane.frame = NSRect(x: 0, y: 0, width: max(0, listScroll.bounds.width), height: docH)
        listPane.needsDisplay = true
    }
    private func layoutPreviewListDocument() {
        let clipH = max(0, previewListScroll.bounds.height)
        let docH = max(clipH, CGFloat(previewList.rows.count) * previewList.rowHeight)
        previewList.frame = NSRect(x: 0, y: 0, width: max(0, previewListScroll.bounds.width), height: docH)
        previewList.needsDisplay = true
    }
    private func scrollSelectionVisible() {
        guard listPane.rows.indices.contains(listPane.selection) else { return }
        let clip = listScroll.contentView
        let visible = clip.bounds
        let r = listPane.rowRect(listPane.selection)
        var target = visible.origin.y
        if r.maxY > visible.maxY {
            target = r.maxY - visible.height
        } else if r.minY < visible.minY {
            target = r.minY
        }
        if target != visible.origin.y {
            clip.scroll(to: NSPoint(x: 0, y: target))
            listScroll.reflectScrolledClipView(clip)
        }
    }
    private func layoutFavorites(from y0: CGFloat) -> CGFloat {
        guard !favPills.isEmpty else { return 0 }
        let pillH: CGFloat = 22
        let gap: CGFloat = 4
        let x0: CGFloat = 8
        var x = x0
        var y = y0
        for p in favPills {
            let pw = p.fittingWidth()
            if x + pw > bounds.width - x0, x > x0 {
                x = x0
                y += pillH + gap
            }
            p.frame = NSRect(x: x, y: y, width: pw, height: pillH)
            x += pw + gap
        }
        return (y - y0) + pillH
    }
    private func displayPath(_ p: String) -> String {
        let home = NSHomeDirectory()
        if p == home { return "~" }
        if p.hasPrefix(home + "/") { return "~" + p.dropFirst(home.count) }
        return p
    }

    private func listDir(_ dir: String, hidden: Bool = false) -> [Entry] {
        let fm = FileManager.default
        guard let names = try? fm.contentsOfDirectory(atPath: dir) else { return [] }
        var out: [Entry] = []
        if (dir as NSString).pathComponents.count > 1 {
            let parent = (dir as NSString).deletingLastPathComponent
            var up = Entry(name: "..", path: parent, isDir: true, size: 0)
            up.icon = Self.iconCache[parent] ?? NSWorkspace.shared.icon(forFile: parent)
            out.append(up)
        }
        for n in names where hidden || showHidden || !n.hasPrefix(".") {
            let p = (dir as NSString).appendingPathComponent(n)
            guard var e = Self.makeEntry(name: n, path: p) else { continue }
            if let cached = Self.iconCache[p] {
                e.icon = cached
            } else {
                let img = NSWorkspace.shared.icon(forFile: p)
                e.icon = img
                Self.iconCache[p] = img
            }
            decorate(&e)
            out.append(e)
        }
        return sortEntries(out)
    }
    fileprivate static func makeEntry(name: String, path: String) -> Entry? {
        var st = Darwin.stat()
        guard stat(path, &st) == 0 else { return nil }
        let isDir = (st.st_mode & S_IFMT) == S_IFDIR
        func date(_ t: timespec) -> Date {
            Date(timeIntervalSince1970: TimeInterval(t.tv_sec) + TimeInterval(t.tv_nsec) / 1e9)
        }
        var e = Entry(name: name, path: path, isDir: isDir, size: isDir ? 0 : Int(st.st_size))
        e.created = date(st.st_birthtimespec)
        e.modified = date(st.st_mtimespec)
        return e
    }
    private func decorate(_ e: inout Entry) {
        let size = e.isDir ? "" : Self.humanSize(e.size)
        func withSize(_ date: String) -> String { size.isEmpty ? date : date + "  ·  " + size }
        switch sortKey {
        case .modified: e.trailingText = e.name == ".." ? "" : withSize(Self.shortDate(e.modified))
        case .created: e.trailingText = e.name == ".." ? "" : withSize(Self.shortDate(e.created))
        default: e.trailingText = size
        }
        e.trailingWidth = e.trailingText.isEmpty ? 0 : (e.trailingText as NSString)
            .size(withAttributes: [.font: NSFont.systemFont(ofSize: 10)]).width
    }
    private static let sameYearFormat: DateFormatter = {
        let f = DateFormatter(); f.dateFormat = "MMM d HH:mm"; return f
    }()
    private static let otherYearFormat: DateFormatter = {
        let f = DateFormatter(); f.dateFormat = "MMM d yyyy"; return f
    }()
    private static func shortDate(_ d: Date) -> String {
        guard d != .distantPast else { return "" }
        let cal = Calendar.current
        return cal.component(.year, from: d) == cal.component(.year, from: Date())
            ? sameYearFormat.string(from: d) : otherYearFormat.string(from: d)
    }
    private func sortEntries(_ list: [Entry]) -> [Entry] {
        let key = sortKey, desc = sortDescending
        let parent = list.filter { $0.name == ".." }
        var rest = list.filter { $0.name != ".." }
        func cmp<T: Comparable>(_ a: T, _ b: T) -> ComparisonResult {
            a == b ? .orderedSame : (a < b ? .orderedAscending : .orderedDescending)
        }
        rest.sort { a, b in
            if a.isDir != b.isDir { return a.isDir }
            var r: ComparisonResult
            switch key {
            case .name: r = a.name.localizedStandardCompare(b.name)
            case .modified: r = cmp(a.modified, b.modified)
            case .created: r = cmp(a.created, b.created)
            case .size: r = cmp(a.size, b.size)
            case .kind:
                r = (a.name as NSString).pathExtension.lowercased()
                    .compare((b.name as NSString).pathExtension.lowercased())
            }
            if r == .orderedSame {
                return a.name.localizedStandardCompare(b.name) == .orderedAscending
            }
            return desc ? r == .orderedDescending : r == .orderedAscending
        }
        return parent + rest
    }
    static func humanSize(_ bytes: Int) -> String {
        let units = ["B", "KB", "MB", "GB", "TB"]
        var v = Double(bytes)
        var i = 0
        while v >= 1024, i < units.count - 1 { v /= 1024; i += 1 }
        return i == 0 ? "\(bytes) B" : String(format: "%.1f %@", v, units[i])
    }

    func reload() {
        all = inRecent ? recentEntries() : listDir(cwd)
        hiddenAll = nil
        dirCache = nil
        refilter()
    }

    private static let globChars = CharacterSet(charactersIn: "*?[")
    private static func hasGlob(_ s: String) -> Bool {
        s.rangeOfCharacter(from: globChars) != nil
    }
    private func resolvePath(_ s: String) -> String {
        var p = Self.expandTilde(s)
        if !p.hasPrefix("/") { p = (cwd as NSString).appendingPathComponent(p) }
        return (p as NSString).standardizingPath
    }

    private func parseQuery(_ raw: String) -> QueryMode {
        let q = raw.trimmingCharacters(in: .whitespaces)
        if q.isEmpty { return .all }
        let words = q.split(separator: " ", maxSplits: 1).map(String.init)
        if let w = words.first?.lowercased(),
           config.browserTerminalWords.contains(where: { $0.lowercased() == w }) {
            if words.count == 1 { return .terminal(cwd) }
            let target = resolvePath(words[1].trimmingCharacters(in: .whitespaces))
            var isDir: ObjCBool = false
            if FileManager.default.fileExists(atPath: target, isDirectory: &isDir) {
                return .terminal(isDir.boolValue ? target : (target as NSString).deletingLastPathComponent)
            }
        }
        let pathLike = q.hasPrefix("/") || q.hasPrefix("~") || q.contains("/")
        guard pathLike else {
            return q.contains("**") ? .recursive(cwd, q) : .local(q)
        }
        let slash = q.range(of: "/", options: .backwards)
        let head = slash.map { String(q[..<$0.upperBound]) } ?? ""
        let tail = slash.map { String(q[$0.upperBound...]) } ?? q
        if Self.hasGlob(head) {
            var base = head.hasPrefix("/") ? "/" : (head.hasPrefix("~") ? NSHomeDirectory() : cwd)
            var rest: [String] = []
            var literal = true
            for comp in head.split(separator: "/").map(String.init) {
                if literal && comp == "~" && base == NSHomeDirectory() { continue }
                if literal && !Self.hasGlob(comp) {
                    base = ((base as NSString).appendingPathComponent(comp) as NSString).standardizingPath
                } else {
                    literal = false
                    rest.append(comp)
                }
            }
            return .recursive(base, (rest + [tail.isEmpty ? "*" : tail]).joined(separator: "/"))
        }
        let dir = head.isEmpty ? cwd : resolvePath(head)
        if tail.contains("**") { return .recursive(dir, tail) }
        return .dir(dir, tail)
    }

    private static func nameFilter(_ pattern: String) -> (Entry) -> Bool {
        if pattern.isEmpty { return { $0.name != ".." } }
        if hasGlob(pattern) {
            let re = globRegex(pattern)
            return { $0.name != ".." && globMatch(re, $0.name) }
        }
        let needle = pattern.lowercased()
        return { $0.name != ".." && $0.name.lowercased().contains(needle) }
    }

    private func refilter() {
        mode = parseQuery(query)
        switch mode {
        case .all, .terminal:
            cancelSearch()
            setRows(all)
        case .local(let pat):
            cancelSearch()
            var source = all
            if pat.hasPrefix(".") {
                if hiddenAll == nil { hiddenAll = listDir(cwd, hidden: true) }
                source = hiddenAll ?? all
            }
            setRows(source.filter(Self.nameFilter(pat)))
        case .dir(let dir, let pat):
            cancelSearch()
            let hidden = pat.hasPrefix(".")
            if dirCache?.0 != dir || dirCache?.1 != hidden {
                dirCache = (dir, hidden, listDir(dir, hidden: hidden))
            }
            setRows((dirCache?.2 ?? []).filter(Self.nameFilter(pat)))
        case .recursive(let base, let glob):
            scheduleSearch(base: base, glob: glob)
        }
        updateStatus()
    }
    private func setRows(_ r: [Entry]) {
        rows = r
        if selection >= rows.count { selection = max(0, rows.count - 1) }
        listPane.rows = rows
        listPane.selection = selection
        layoutListDocument()
        scrollListToTop()
        previewSelection()
    }

    private static let rgPath: String? = {
        var cands = ["/opt/homebrew/bin/rg", "/usr/local/bin/rg"]
        for d in (ProcessInfo.processInfo.environment["PATH"] ?? "").split(separator: ":") {
            cands.append(String(d) + "/rg")
        }
        return cands.first { FileManager.default.isExecutableFile(atPath: $0) }
    }()

    private func cancelSearch() {
        searchWork?.cancel()
        searchWork = nil
        searchGen += 1
        searchProcess?.terminate()
        searchProcess = nil
    }
    private func scheduleSearch(base: String, glob: String) {
        cancelSearch()
        setRows([])
        setStatus("searching \(displayPath(base))…")
        let work = DispatchWorkItem { [weak self] in self?.startSearch(base: base, glob: glob) }
        searchWork = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.15, execute: work)
    }
    private func startSearch(base: String, glob rawGlob: String) {
        searchGen += 1
        let gen = searchGen
        var glob = rawGlob
        let last = (glob as NSString).lastPathComponent
        if !Self.hasGlob(last) && !last.isEmpty {
            glob = String(glob.dropLast(last.count)) + "*" + last + "*"
        }
        let limit = max(1, config.browserSearchLimit)
        let excludes = config.browserSearchExcludes
        let hidden = glob.hasPrefix(".") || glob.contains("/.")
        var proc: Process?
        if let rg = Self.rgPath {
            let p = Process()
            p.executableURL = URL(fileURLWithPath: rg)
            var args = ["--files", "--no-messages", "--color", "never",
                        "--glob-case-insensitive", "-g", glob]
            for x in excludes where !x.isEmpty { args += ["-g", "!" + x] }
            if hidden { args += ["--hidden", "-g", "!.git"] }
            p.arguments = args
            p.currentDirectoryURL = URL(fileURLWithPath: base)
            proc = p
            searchProcess = p
        }
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            var rels: [String] = []
            var truncated = false
            if let proc {
                let pipe = Pipe()
                proc.standardOutput = pipe
                proc.standardError = FileHandle.nullDevice
                if (try? proc.run()) != nil {
                    let h = pipe.fileHandleForReading
                    var pending = Data()
                    while true {
                        let chunk = h.availableData
                        if chunk.isEmpty { break }
                        pending.append(chunk)
                        while let nl = pending.firstIndex(of: 0x0A) {
                            let line = pending[pending.startIndex..<nl]
                            pending.removeSubrange(pending.startIndex...nl)
                            if let s = String(data: line, encoding: .utf8), !s.isEmpty { rels.append(s) }
                        }
                        if rels.count >= limit {
                            truncated = true
                            proc.terminate()
                            break
                        }
                    }
                    proc.waitUntilExit()
                }
            } else {
                truncated = Self.walk(base: base, glob: glob, hidden: hidden,
                                      excludes: excludes, limit: limit, into: &rels)
            }
            let entries = rels.prefix(limit).compactMap { rel in
                Self.makeEntry(name: rel, path: (base as NSString).appendingPathComponent(rel))
            }
            DispatchQueue.main.async {
                guard let self, gen == self.searchGen else { return }
                self.searchProcess = nil
                let decorated: [Entry] = entries.map {
                    var e = $0
                    e.icon = Self.typeIcon(e)
                    self.decorate(&e)
                    return e
                }
                self.selection = 0
                self.setRows(self.sortEntries(decorated))
                let n = decorated.count
                var msg = "\(n) match\(n == 1 ? "" : "es") in \(self.displayPath(base))"
                if truncated { msg += " — first \(limit) shown" }
                if Self.rgPath == nil { msg += " (install ripgrep for faster search)" }
                self.setStatus(msg)
            }
        }
    }
    private static func walk(base: String, glob: String, hidden: Bool, excludes: [String],
                             limit: Int, into out: inout [String]) -> Bool {
        guard let re = pathGlobRegex(glob) else { return false }
        let skip = Set(excludes.map { $0.trimmingCharacters(in: CharacterSet(charactersIn: "/")) })
        let opts: FileManager.DirectoryEnumerationOptions = hidden
            ? [.skipsPackageDescendants] : [.skipsHiddenFiles, .skipsPackageDescendants]
        guard let en = FileManager.default.enumerator(
            at: URL(fileURLWithPath: base), includingPropertiesForKeys: [.isDirectoryKey],
            options: opts) else { return false }
        let prefix = (base as NSString).standardizingPath + "/"
        for case let url as URL in en {
            if skip.contains(url.lastPathComponent) { en.skipDescendants(); continue }
            if (try? url.resourceValues(forKeys: [.isDirectoryKey]))?.isDirectory == true { continue }
            let rel = url.path.hasPrefix(prefix) ? String(url.path.dropFirst(prefix.count)) : url.path
            if globMatch(re, rel) {
                out.append(rel)
                if out.count >= limit { return true }
            }
        }
        return false
    }
    private static func pathGlobRegex(_ glob: String) -> NSRegularExpression? {
        var out = glob.contains("/") ? "^" : "^(?:.*/)?"
        let chars = Array(glob)
        var i = 0
        while i < chars.count {
            let ch = chars[i]
            if ch == "*", i + 1 < chars.count, chars[i + 1] == "*" {
                if i + 2 < chars.count, chars[i + 2] == "/" { out += "(?:.*/)?"; i += 3 } else { out += ".*"; i += 2 }
                continue
            }
            switch ch {
            case "*": out += "[^/]*"
            case "?": out += "[^/]"
            default: out += NSRegularExpression.escapedPattern(for: String(ch))
            }
            i += 1
        }
        return try? NSRegularExpression(pattern: out + "$", options: [.caseInsensitive])
    }
    private static var typeIcons: [String: NSImage] = [:]
    private static func typeIcon(_ e: Entry) -> NSImage {
        let ext = (e.path as NSString).pathExtension.lowercased()
        if let i = typeIcons[ext] { return i }
        let img = NSWorkspace.shared.icon(for: UTType(filenameExtension: ext) ?? .data)
        typeIcons[ext] = img
        return img
    }

    private static func globRegex(_ glob: String) -> NSRegularExpression {
        var out = "^"
        for ch in glob.lowercased() {
            switch ch {
            case "*": out += ".*"
            case "?": out += "."
            case ".", "(", ")", "[", "]", "{", "}", "+", "^", "$", "|", "\\":
                out += "\\\(ch)"
            default: out += String(ch)
            }
        }
        out += "$"
        return try! NSRegularExpression(pattern: out, options: [.caseInsensitive])
    }
    private static func globMatch(_ re: NSRegularExpression, _ s: String) -> Bool {
        re.firstMatch(in: s, options: [],
                      range: NSRange(0..<(s as NSString).length)) != nil
    }
    private static func expandTilde(_ s: String) -> String {
        s.hasPrefix("~") ? (s as NSString).expandingTildeInPath : s
    }

    private func showSortMenu() {
        let menu = NSMenu()
        for (i, k) in SortKey.allCases.enumerated() {
            let item = NSMenuItem(title: k.label, action: #selector(pickSortKey(_:)), keyEquivalent: "")
            item.target = self
            item.tag = i
            item.state = k == sortKey ? .on : .off
            menu.addItem(item)
        }
        menu.popUp(positioning: nil,
                   at: NSPoint(x: sortButton.frame.minX, y: sortButton.frame.maxY + 2), in: self)
    }
    @objc private func pickSortKey(_ sender: NSMenuItem) {
        let k = SortKey.allCases[sender.tag]
        if k != sortKey { sortDescending = k.naturalDescending }
        sortKey = k
        applySort()
    }
    private func toggleSortOrder() {
        sortDescending.toggle()
        applySort()
    }
    private func applySort() {
        updateSortTitle()
        func resort(_ list: [Entry]) -> [Entry] {
            sortEntries(list.map { var e = $0; decorate(&e); return e })
        }
        all = resort(all)
        hiddenAll = hiddenAll.map(resort)
        if let c = dirCache { dirCache = (c.0, c.1, resort(c.2)) }
        if case .recursive = mode {
            setRows(resort(rows))
        } else {
            refilter()
        }
        onSortChange?(sortKey.rawValue, sortDescending)
        needsLayout = true
    }
    private func updateSortTitle() {
        sortButton.title = sortKey.short
        orderButton.symbol = sortDescending ? "arrow.down" : "arrow.up"
        orderButton.title = sortDescending ? "Desc" : "Asc"
    }

    private func scrollListToTop() {
        let clip = listScroll.contentView
        if clip.bounds.origin.y != 0 {
            clip.scroll(to: NSPoint(x: 0, y: 0))
            listScroll.reflectScrolledClipView(clip)
        }
    }

    public func showRecent() { showVirtual(0) }

    public func showVirtual(_ i: Int) {
        guard virtualLists.indices.contains(i) else { return }
        if virtualIndex != i { remember() }
        virtualIndex = i
        query = ""
        selection = 0
        cancelSearch()
        reload()
        rebuildPills()
        showCwdInFilter()
        needsLayout = true
    }

    public func recentChanged() {
        guard inRecent, query.isEmpty else { return }
        let keep = rows.indices.contains(selection) ? rows[selection].path : nil
        let fresh = recentEntries()
        guard fresh.map({ $0.path + $0.trailingText }) != all.map({ $0.path + $0.trailingText }) else { return }
        all = fresh
        rows = all
        selection = keep.flatMap { k in rows.firstIndex { $0.path == k } } ?? 0
        listPane.rows = rows
        listPane.selection = selection
        layoutListDocument()
        updateStatus()
    }

    private func recentEntries() -> [Entry] {
        let now = Date()
        guard let vi = virtualIndex, virtualLists.indices.contains(vi) else { return [] }
        return virtualLists[vi].provider().compactMap { item in
            let name = (item.path as NSString).lastPathComponent
            guard var e = Self.makeEntry(name: name, path: item.path) else { return nil }
            e.icon = Self.iconCache[item.path] ?? NSWorkspace.shared.icon(forFile: item.path)
            Self.iconCache[item.path] = e.icon
            var dir = displayPath((item.path as NSString).deletingLastPathComponent)
            if dir.hasPrefix("/private/tmp") { dir.removeFirst("/private".count) }
            e.trailingText = ([dir] + [item.note].compactMap { $0 }.map { "↓ " + $0 }
                              + (e.isDir ? [] : [Self.humanSize(e.size)])
                              + [Self.ago(now.timeIntervalSince(item.at))]).joined(separator: " · ")
            e.trailingWidth = (e.trailingText as NSString)
                .size(withAttributes: [.font: NSFont.systemFont(ofSize: 10)]).width
            return e
        }
    }

    private static func ago(_ secs: TimeInterval) -> String {
        let s = max(0, Int(secs))
        if s < 60 { return "just now" }
        if s < 3600 { return "\(s / 60)m ago" }
        if s < 86400 { return "\(s / 3600)h ago" }
        return "\(s / 86400)d ago"
    }

    func cd(_ dir: String) {
        let dest = (dir as NSString).standardizingPath
        if inRecent || dest != cwd { remember() }
        virtualIndex = nil
        cwd = dest
        query = ""
        selection = 0
        cancelSearch()
        reload()
        updateStarTitle()
        onDirChange?(cwd)
        rebuildPills()
        showCwdInFilter()
        needsLayout = true
    }
    func cdParent() {
        if inRecent { cd(cwd); return }
        let parent = (cwd as NSString).deletingLastPathComponent
        if parent != cwd { cd(parent) }
    }
    func showCwdInFilter() {
        guard searchField.currentEditor() == nil else { return }
        if let vi = virtualIndex, virtualLists.indices.contains(vi) {
            searchField.stringValue = ""
            searchField.placeholderString = "\(virtualLists[vi].title) · type to filter"
            return
        }
        searchField.stringValue = cwd
        searchField.placeholderString = nil
    }

    private var rename: InlineRename?
    var renameEditor: NSText? { rename?.textField?.currentEditor() }

    func beginRename(_ index: Int? = nil) {
        let i = index ?? listPane.selection
        guard rows.indices.contains(i), rows[i].name != "..", let w = window else { return }
        commitRename()
        let e = rows[i]
        selection = i
        listPane.selection = i
        scrollSelectionVisible()
        let r = InlineRename.begin(parent: listPane, nameRect: listPane.nameRect(i),
                                   name: (e.path as NSString).lastPathComponent,
                                   isDir: e.isDir, colors: config.colors, fontSize: 12 * textZoom,
                                   onCommit: { [weak self] in self?.commitRename() })
        r.setPath(e.path)
        rename = r
        PopupWindow.transientEscape = { [weak self] in self?.cancelRename() }
        guard w.makeFirstResponder(r.textField!) else { cancelRename(); return }
    }

    private func endRename() -> (path: String, text: String)? {
        guard let r = rename else { return nil }
        let result = r.end(list: listPane, window: window)
        rename = nil
        PopupWindow.transientEscape = nil
        return result
    }

    func cancelRename() {
        rename?.cancel(window: window)
        rename = nil
        PopupWindow.transientEscape = nil
    }

    func commitRename() {
        guard let (path, text) = endRename() else { return }
        let old = (path as NSString).lastPathComponent
        let new = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !new.isEmpty, new != old else { return }
        guard !new.contains("/"), new != ".", new != ".." else {
            setStatus("can't rename: “\(new)” is not a valid name")
            return
        }
        let dst = ((path as NSString).deletingLastPathComponent as NSString).appendingPathComponent(new)
        if new.lowercased() != old.lowercased(), FileManager.default.fileExists(atPath: dst) {
            setStatus("can't rename: “\(new)” already exists")
            return
        }
        do {
            try FileManager.default.moveItem(atPath: path, toPath: dst)
        } catch {
            setStatus("rename failed: \(error.localizedDescription)")
            return
        }
        FileDrag.onFileOp?(path, dst)
        FileOps.recordRename(from: path, to: dst)
        reload()
        if let i = rows.firstIndex(where: { $0.path == dst }) {
            selection = i
            listPane.selection = i
            scrollSelectionVisible()
        }
        previewSelection()
        setStatus("renamed “\(old)” to “\(new)”")
    }

    private struct Place: Equatable {
        var virtual: Int?
        var dir: String
    }
    private var backStack: [Place] = []
    private var forwardStack: [Place] = []
    private var travelling = false
    private var place: Place { Place(virtual: virtualIndex, dir: cwd) }

    private func remember() {
        guard !travelling else { return }
        if backStack.last != place { backStack.append(place) }
        if backStack.count > 50 { backStack.removeFirst() }
        forwardStack = []
    }

    private func travel(back: Bool) {
        guard let to = back ? backStack.popLast() : forwardStack.popLast() else {
            setStatus(back ? "no earlier folder" : "no later folder")
            return
        }
        if back { forwardStack.append(place) } else { backStack.append(place) }
        travelling = true
        defer { travelling = false }
        if let v = to.virtual, virtualLists.indices.contains(v) { showVirtual(v) } else { cd(to.dir) }
    }
    func goBack() { travel(back: true) }
    func goForward() { travel(back: false) }

    private var showHidden = false
    private static var cutChange: Int?

    private func selectedPaths() -> [String] {
        listPane.selectedRows.filter { rows.indices.contains($0) && rows[$0].name != ".." }.map { rows[$0].path }
    }

    private var opsDirectory: String? {
        guard !inRecent else { return nil }
        switch mode {
        case .all, .local, .terminal: return cwd
        case .dir(let d, _): return d
        case .recursive: return nil
        }
    }

    private static func clipboardFiles() -> [URL] {
        NSPasteboard.general.readObjects(forClasses: [NSURL.self],
                                         options: [.urlReadingFileURLsOnly: true]) as? [URL] ?? []
    }

    func canPerform(_ a: FileListPane.Action) -> Bool {
        switch a {
        case .newFolder, .newFile: return opsDirectory != nil
        case .paste: return opsDirectory != nil && !Self.clipboardFiles().isEmpty
        case .toggleHidden: return !inRecent
        case .enclosing:
            if inRecent { return true }
            if case .recursive = mode { return true }
            return false
        case .trash, .duplicate, .copy, .cut, .quickLook: return !selectedPaths().isEmpty
        }
    }

    func perform(_ a: FileListPane.Action) {
        switch a {
        case .trash: trashSelection()
        case .duplicate: duplicateSelection()
        case .copy: copyFiles(cut: false)
        case .cut: copyFiles(cut: true)
        case .paste: pasteFiles(move: false)
        case .newFolder: newItem(folder: true)
        case .newFile: newItem(folder: false)
        case .quickLook: toggleQuickLook()
        case .enclosing: showEnclosing()
        case .toggleHidden: toggleHidden()
        }
    }

    private func run(_ work: @escaping () -> FileOps.Outcome,
                     then: @escaping (FileOps.Outcome) -> String) {
        DispatchQueue.global(qos: .userInitiated).async {
            let out = work()
            for c in out.changes { FileDrag.onFileOp?(c.from, c.to) }
            DispatchQueue.main.async { [weak self] in
                guard let self else { return }
                self.reload()
                let msg = then(out)
                self.scrollSelectionVisible()
                self.setStatus(out.failed.map { out.changes.isEmpty ? "failed — \($0)" : "\(msg) — \($0)" } ?? msg)
            }
        }
    }

    @discardableResult
    private func select(path: String) -> Bool {
        guard let i = rows.firstIndex(where: { $0.path == path }) else { return false }
        selection = i
        listPane.selection = i
        scrollSelectionVisible()
        previewSelection()
        return true
    }

    private static func items(_ n: Int, _ one: String) -> String {
        n == 1 ? "“\(one)”" : "\(n) items"
    }
    private static func leaf(_ p: String?) -> String { ((p ?? "") as NSString).lastPathComponent }

    func trashSelection() {
        let paths = selectedPaths()
        guard !paths.isEmpty else { return }
        commitRename()
        run({ FileOps.trash(paths) }) { out in
            "moved \(Self.items(out.changes.count, Self.leaf(out.changes.first?.from))) to the Trash · ⌘Z undoes"
        }
    }

    func duplicateSelection() {
        let paths = selectedPaths()
        guard !paths.isEmpty else { return }
        run({ FileOps.duplicate(paths) }) { [weak self] out in
            if let first = out.paths.first { self?.select(path: first) }
            return "duplicated \(Self.items(out.changes.count, Self.leaf(paths.first)))"
        }
    }

    func copyFiles(cut: Bool) {
        let paths = selectedPaths()
        guard !paths.isEmpty else { return }
        let pb = NSPasteboard.general
        pb.clearContents()
        pb.writeObjects(paths.map { p -> NSPasteboardItem in
            let item = NSPasteboardItem()
            item.setString(URL(fileURLWithPath: p).absoluteString, forType: .fileURL)
            item.setString(p, forType: .string)
            return item
        })
        Self.cutChange = cut ? pb.changeCount : nil
        let what = Self.items(paths.count, Self.leaf(paths.first))
        setStatus(cut ? "cut \(what) · ⌘V moves it here" : "copied \(what) · ⌘V pastes the file, or its path as text")
        if !cut {
            onCopied?(paths.count == 1 ? (paths[0] as NSString).abbreviatingWithTildeInPath : "\(paths.count) items")
        }
    }

    func pasteFiles(move forceMove: Bool) {
        let urls = Self.clipboardFiles()
        guard !urls.isEmpty else { return }
        guard let dir = opsDirectory else {
            setStatus("can't paste here — open a folder first")
            return
        }
        let move = forceMove || Self.cutChange == NSPasteboard.general.changeCount
        if move { Self.cutChange = nil }
        let shown = displayPath(dir)
        run({ FileOps.transfer(urls, into: dir, move: move) }) { [weak self] out in
            if let first = out.paths.first { self?.select(path: first) }
            let what = Self.items(out.changes.count, Self.leaf(out.paths.first))
            return out.changes.isEmpty ? "nothing to paste" : "\(move ? "moved" : "pasted") \(what) → \(shown)"
        }
    }

    func newItem(folder: Bool) {
        guard let dir = opsDirectory else {
            setStatus("can't create here — open a folder first")
            return
        }
        commitRename()
        let out = FileOps.create(folder ? "untitled folder" : "untitled.txt", in: dir, folder: folder)
        guard let path = out.paths.first else {
            setStatus("couldn't create — \(out.failed ?? "unknown error")")
            return
        }
        FileDrag.onFileOp?(nil, path)
        if !query.isEmpty {
            query = ""
            searchField.stringValue = ""
        }
        if dir != cwd { cd(dir) } else { reload() }
        guard select(path: path) else { return }
        if let w = window { w.makeFirstResponder(listPane) }
        setStatus("created “\(Self.leaf(path))”")
        beginRename(selection)
    }

    func undoFileOp() {
        guard FileOps.canUndo else {
            setStatus("nothing to undo")
            return
        }
        var what = ""
        run({
            guard let u = FileOps.undo() else { return FileOps.Outcome() }
            what = u.what
            return u.outcome
        }) { [weak self] out in
            if let first = out.paths.first { self?.select(path: first) }
            return out.changes.isEmpty ? "couldn't undo \(what)" : "undid \(what)"
        }
    }

    func toggleHidden() {
        guard !inRecent else { return }
        let keep = rows.indices.contains(selection) ? rows[selection].path : nil
        showHidden.toggle()
        reload()
        if let keep { select(path: keep) }
        setStatus(showHidden ? "showing hidden files" : "hidden files hidden")
    }

    func showEnclosing() {
        guard rows.indices.contains(selection), rows[selection].name != ".." else { return }
        let path = rows[selection].path
        searchField.abortEditing()
        cd((path as NSString).deletingLastPathComponent)
        if !select(path: path), path.contains("/.") {
            showHidden = true
            reload()
            select(path: path)
        }
        if let w = window { w.makeFirstResponder(listPane) }
    }

    func handleShortcut(_ code: UInt16, _ mods: NSEvent.ModifierFlags) -> Bool {
        guard mods.contains(.command), let w = window else { return false }
        let shift = mods.contains(.shift)
        let inList = w.firstResponder === listPane
        if w.firstResponder === previewText, !shift {
            switch code {
            case 0:
                previewText.selectAll(nil)
                setStatus("preview: all selected · ⌘C copies")
                return true
            case 8:
                let sel = previewText.selectedRange()
                let all = previewText.string as NSString
                let text = sel.length > 0 ? all.substring(with: sel) : all as String
                guard !text.isEmpty else { return true }
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(text, forType: .string)
                let lines = text.split(separator: "\n", omittingEmptySubsequences: false).count
                setStatus("copied \(lines) line\(lines == 1 ? "" : "s") of the preview")
                window?.delegate.flatMap { $0 as? PopupWindow }?.showToast(
                    sel.length > 0 ? "Copied selection" : "Copied the whole preview", symbol: "doc.on.clipboard")
                return true
            default: break
            }
        }
        switch code {
        case 33: goBack(); return true
        case 30: goForward(); return true
        case 47 where shift: toggleHidden(); return true
        case 45 where shift: newItem(folder: true); return true
        case 126:
            if canPerform(.enclosing) { showEnclosing() } else { cdParent() }
            return true
        case 125 where inList: openIndex(selection); return true
        case 51 where inList: trashSelection(); return true
        case 2 where inList: duplicateSelection(); return true
        case 0 where inList:
            listPane.selectAll()
            setStatus("\(listPane.selectedRows.count) selected")
            return true
        case 8 where inList: copyFiles(cut: false); return true
        case 7 where inList: copyFiles(cut: true); return true
        case 9 where inList && canPerform(.paste):
            pasteFiles(move: mods.contains(.option))
            return true
        case 6 where inList: undoFileOp(); return true
        default: return false
        }
    }

    private var quickLookPaths: [String] = []
    private var quickLookUp: Bool {
        QLPreviewPanel.sharedPreviewPanelExists() && QLPreviewPanel.shared().isVisible
            && QLPreviewPanel.shared().dataSource === self
    }

    func toggleQuickLook() {
        guard let panel = QLPreviewPanel.shared() else { return }
        if quickLookUp {
            panel.orderOut(nil)
            window?.makeKey()
            return
        }
        let paths = selectedPaths()
        guard !paths.isEmpty else { return }
        quickLookPaths = paths
        panel.dataSource = self
        panel.delegate = self
        panel.reloadData()
        panel.currentPreviewItemIndex = 0
        panel.makeKeyAndOrderFront(nil)
    }

    private func refreshQuickLook() {
        guard quickLookUp else { return }
        quickLookPaths = selectedPaths()
        QLPreviewPanel.shared().reloadData()
    }

    override func acceptsPreviewPanelControl(_ panel: QLPreviewPanel!) -> Bool { true }
    override func beginPreviewPanelControl(_ panel: QLPreviewPanel!) {
        panel.dataSource = self
        panel.delegate = self
    }
    override func endPreviewPanelControl(_ panel: QLPreviewPanel!) {}

    func numberOfPreviewItems(in panel: QLPreviewPanel!) -> Int { quickLookPaths.count }
    func previewPanel(_ panel: QLPreviewPanel!, previewItemAt index: Int) -> QLPreviewItem! {
        guard quickLookPaths.indices.contains(index) else { return nil }
        return URL(fileURLWithPath: quickLookPaths[index]) as NSURL
    }
    func previewPanel(_ panel: QLPreviewPanel!, handle event: NSEvent!) -> Bool {
        guard event.type == .keyDown else { return false }
        switch event.keyCode {
        case 49:
            toggleQuickLook()
            return true
        case 125, 126:
            listPane.moveSelection(event.keyCode == 125 ? 1 : -1)
            return true
        default:
            return false
        }
    }

    @discardableResult
    func copyRowPath(_ i: Int) -> String? {
        guard rows.indices.contains(i) else { return nil }
        let p = rows[i].path
        onCopyPath?(p)
        onStatus?("copied \(p)")
        onCopied?((p as NSString).abbreviatingWithTildeInPath)
        return p
    }
    private func openIndex(_ i: Int) {
        guard rows.indices.contains(i) else { return }
        let e = rows[i]
        if e.isDir {
            cd(e.path)
        } else {
            onOpen?(e.path)
        }
    }

    @discardableResult
    private func jumpToQueryPath() -> Bool {
        let q = searchField.stringValue.trimmingCharacters(in: .whitespaces)
        guard !q.isEmpty, q.contains("/") || q.hasPrefix("~"), !Self.hasGlob(q) else { return false }
        let p = resolvePath(q)
        var isDir: ObjCBool = false
        guard FileManager.default.fileExists(atPath: p, isDirectory: &isDir) else { return false }
        if isDir.boolValue {
            cd(p)
            if let w = window { w.makeFirstResponder(listPane) }
        } else {
            onOpen?(p)
            setStatus("opened \(displayPath(p))")
        }
        return true
    }

    private func terminalDir(for e: Entry) -> String {
        e.isDir ? e.path : (e.path as NSString).deletingLastPathComponent
    }
    private func openTerminal(_ dir: String) {
        onOpenTerminal?(dir)
        setStatus("terminal opened in \(displayPath(dir))")
    }

    private func previewSelection() {
        previewGen += 1
        previewLatest.value = previewGen
        guard rows.indices.contains(selection) else { return showHint(""); }
        let e = rows[selection]
        if e.isDir {
            previewList.rows = listDir(e.path).filter { $0.name != ".." }
            previewList.selection = 0
            layoutPreviewListDocument()
            showFolderList()
            return
        }
        let key = "\(e.path)|\(e.modified.timeIntervalSince1970)|\(e.size)"
        if let hit = Self.previewCache[key] { return applyPreview(hit) }
        let scale = window?.backingScaleFactor ?? 2
        let px = max(800, max(previewImage.bounds.width, previewImage.bounds.height) * scale)
        let gen = previewGen, latest = previewLatest, path = e.path
        Self.previewQueue.async { [weak self] in
            guard latest.value == gen else { return }
            let content = Self.loadPreview(path, maxPixels: px)
            DispatchQueue.main.async {
                Self.cachePreview(key, content)
                guard let self, self.previewGen == gen else { return }
                self.applyPreview(content)
            }
        }
    }
    private func applyPreview(_ content: PreviewContent) {
        switch content {
        case .image(let img):
            previewImage.image = img
            previewImage.path = rows.indices.contains(selection) ? rows[selection].path : nil
            showImage()
        case .text(let text):
            previewText.string = text
            previewText.scrollRangeToVisible(NSRange(location: 0, length: 0))
            showText()
        case .hint(let h):
            showHint(h)
        }
    }
    private static func cachePreview(_ key: String, _ content: PreviewContent) {
        if previewCache[key] == nil { previewCacheOrder.append(key) }
        previewCache[key] = content
        while previewCacheOrder.count > 24 {
            previewCache.removeValue(forKey: previewCacheOrder.removeFirst())
        }
    }
    private static func loadPreview(_ path: String, maxPixels: CGFloat) -> PreviewContent {
        let ext = (path as NSString).pathExtension.lowercased()
        if imageExts.contains(ext) {
            let img = ext == "pdf" ? pdfPreviewImage(path)
                : ext == "gif" ? NSImage(contentsOfFile: path)
                : downsampledImage(path, maxPixels: maxPixels) ?? NSImage(contentsOfFile: path)
            return img.map { .image($0) } ?? .hint("unable to preview")
        }
        if ext == "rtf", let img = rtfPreviewImage(path) {
            return .image(img)
        }
        if ext == "docx", let text = docxText(path), !text.isEmpty {
            return .text(text)
        }
        if let text = textPreview(path) { return .text(text) }
        return .hint("no preview")
    }
    private static func downsampledImage(_ path: String, maxPixels: CGFloat) -> NSImage? {
        guard let src = CGImageSourceCreateWithURL(URL(fileURLWithPath: path) as CFURL,
                                                   [kCGImageSourceShouldCache: false] as CFDictionary)
        else { return nil }
        let opts: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceShouldCacheImmediately: true,
            kCGImageSourceThumbnailMaxPixelSize: Int(maxPixels),
        ]
        guard let cg = CGImageSourceCreateThumbnailAtIndex(src, 0, opts as CFDictionary) else { return nil }
        return NSImage(cgImage: cg, size: NSSize(width: cg.width, height: cg.height))
    }
    private func showHint(_ s: String) {
        previewScroll.isHidden = true
        previewImage.isHidden = true
        previewListScroll.isHidden = true
        previewHint.isHidden = s.isEmpty
        previewHint.stringValue = s
    }
    private func showText() {
        previewHint.isHidden = true
        previewImage.isHidden = true
        previewListScroll.isHidden = true
        previewScroll.isHidden = false
    }
    private func showImage() {
        previewHint.isHidden = true
        previewScroll.isHidden = true
        previewListScroll.isHidden = true
        previewImage.isHidden = false
    }
    private func showFolderList() {
        previewHint.isHidden = true
        previewScroll.isHidden = true
        previewImage.isHidden = true
        previewListScroll.isHidden = false
    }
    private func previewOpen(_ i: Int) {
        guard previewList.rows.indices.contains(i) else { return }
        let e = previewList.rows[i]
        if e.isDir { cd(e.path) } else { onOpen?(e.path) }
    }
    private static func textPreview(_ path: String) -> String? {
        var st = Darwin.stat()
        guard stat(path, &st) == 0 else { return nil }
        let sz = Int(st.st_size)
        if sz > Self.textLimit {
            guard let h = FileHandle(forReadingAtPath: path) else { return nil }
            defer { try? h.close() }
            let data = h.readData(ofLength: Self.textLimit)
            return String(data: data, encoding: .utf8)
        }
        guard let data = FileManager.default.contents(atPath: path), !data.isEmpty else { return nil }
        return String(data: data, encoding: .utf8)
    }

    private static func pdfPreviewImage(_ path: String) -> NSImage? {
        guard let doc = PDFDocument(url: URL(fileURLWithPath: path)),
              let page = doc.page(at: 0) else { return nil }
        let bounds = page.bounds(for: .mediaBox)
        let scale: CGFloat = 2
        let w = max(1, bounds.width * scale)
        let h = max(1, bounds.height * scale)
        guard let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(w),
                                         pixelsHigh: Int(h), bitsPerSample: 8,
                                         samplesPerPixel: 4, hasAlpha: true,
                                         isPlanar: false, colorSpaceName: .deviceRGB,
                                         bytesPerRow: 0, bitsPerPixel: 0) else { return nil }
        rep.size = NSSize(width: w, height: h)
        NSGraphicsContext.saveGraphicsState()
        let ctx = NSGraphicsContext(bitmapImageRep: rep)!
        NSGraphicsContext.current = ctx
        ctx.cgContext.scaleBy(x: scale, y: scale)
        ctx.cgContext.translateBy(x: -bounds.origin.x, y: -bounds.origin.y)
        ctx.cgContext.setFillColor(NSColor.white.cgColor)
        ctx.cgContext.fill(bounds)
        page.draw(with: .mediaBox, to: ctx.cgContext)
        NSGraphicsContext.restoreGraphicsState()
        let img = NSImage(size: NSSize(width: bounds.width, height: bounds.height))
        img.addRepresentation(rep)
        return img
    }

    static func rtfPreviewImage(_ path: String) -> NSImage? {
        guard let data = try? Data(contentsOf: URL(fileURLWithPath: path)),
              let attrs = try? NSAttributedString(data: data,
                                                  options: [.documentType: NSAttributedString.DocumentType.rtf],
                                                  documentAttributes: nil) else { return nil }
        let width: CGFloat = 640
        let drawOpts: NSString.DrawingOptions = [.usesLineFragmentOrigin, .usesFontLeading]
        let pad: CGFloat = 16
        let bounds = attrs.boundingRect(with: NSSize(width: width - pad, height: .greatestFiniteMagnitude),
                                        options: drawOpts, context: nil)
        let h = min(max(bounds.height + pad, 80), 3200)
        let scale: CGFloat = 2
        guard let rep = NSBitmapImageRep(bitmapDataPlanes: nil,
                                         pixelsWide: Int(width * scale),
                                         pixelsHigh: Int(h * scale),
                                         bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true,
                                         isPlanar: false, colorSpaceName: .deviceRGB,
                                         bytesPerRow: 0, bitsPerPixel: 0) else { return nil }
        rep.size = NSSize(width: width, height: h)
        NSGraphicsContext.saveGraphicsState()
        let ctx = NSGraphicsContext(bitmapImageRep: rep)!
        NSGraphicsContext.current = ctx
        ctx.cgContext.scaleBy(x: scale, y: scale)
        ctx.cgContext.setFillColor(NSColor.white.cgColor)
        ctx.cgContext.fill(CGRect(x: 0, y: 0, width: width, height: h))
        attrs.draw(with: NSRect(x: pad / 2, y: pad / 2, width: width - pad, height: h - pad),
                   options: drawOpts)
        NSGraphicsContext.restoreGraphicsState()
        let img = NSImage(size: NSSize(width: width, height: h))
        img.addRepresentation(rep)
        return img
    }

    private static func docxText(_ path: String) -> String? {
        guard let r = try? runProcess("/usr/bin/unzip", ["-p", path, "word/document.xml"]),
              r.code == 0 else { return nil }
        let parser = XMLParser(data: Data(r.out.utf8))
        let ex = DocxTextExtractor()
        parser.delegate = ex
        parser.parse()
        return ex.text.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private func loadFavorites() {
        guard let data = try? Data(contentsOf: favURL) else { return }
        if let arr = try? JSONSerialization.jsonObject(with: data) as? [String] {
            pinnedFavorites = arr
        }
    }
    private func saveFavorites() {
        try? FileManager.default.createDirectory(
            atPath: favURL.deletingLastPathComponent().path, withIntermediateDirectories: true)
        if let data = try? JSONSerialization.data(withJSONObject: pinnedFavorites) {
            try? data.write(to: favURL)
        }
    }
    public var favoriteFolders: [String] { mergedFavorites() }
    public var onFavoritesChanged: (() -> Void)?
    private func mergedFavorites() -> [String] {
        var out: [String] = []
        var seen = Set<String>()
        for raw in staticFavorites + pinnedFavorites {
            for p in Self.favoriteFolders(from: raw) where seen.insert(p).inserted { out.append(p) }
        }
        return out
    }
    static func favoriteFolders(from raw: String) -> [String] {
        func isDir(_ p: String) -> Bool {
            var d: ObjCBool = false
            return FileManager.default.fileExists(atPath: p, isDirectory: &d) && d.boolValue
        }
        let whole = (raw as NSString).expandingTildeInPath
        if isDir(whole) { return [whole] }
        let parts = raw.replacingOccurrences(of: #"\s+(?=[/~])"#, with: "\n", options: .regularExpression)
            .split(separator: "\n").map { ($0.trimmingCharacters(in: .whitespaces) as NSString).expandingTildeInPath }
        return parts.filter(isDir)
    }
    private func toggleStar() {
        guard !inRecent else { return }
        if shownFavorites.contains(cwd) {
            if pinnedFavorites.contains(cwd) {
                pinnedFavorites.removeAll { $0 == cwd }
            } else {
                pinnedFavorites.append(cwd)
            }
        } else {
            pinnedFavorites.append(cwd)
        }
        saveFavorites()
        rebuildPills()
        updateStarTitle()
        onFavoritesChanged?()
        onStatus?(shownFavorites.contains(cwd) ? "★ pinned \(displayPath(cwd))" : "unpinned \(displayPath(cwd))")
    }
    private func updateStarTitle() {
        let on = shownFavorites.contains(cwd)
        starButton.isOn = on
        starButton.title = on ? "Pinned" : "Pin"
        starButton.symbol = on ? "star.fill" : "star"
    }
    private func rebuildPills() {
        for p in favPills { p.removeFromSuperview() }
        favPills = []
        shownFavorites = mergedFavorites()
        if sidebar != nil {
            syncSidebar()
            updateStarTitle()
            needsLayout = true
            return
        }
        for (i, v) in virtualLists.enumerated() {
            let r = ThemeButton(config: config, title: v.title, symbol: v.symbol)
            r.isOn = virtualIndex == i
            r.onClick = { [weak self] in self?.showVirtual(i) }
            addSubview(r)
            favPills.append(r)
        }
        for fav in shownFavorites {
            let p = ThemeButton(config: config, title: displayPath(fav), symbol: "folder")
            p.isOn = fav == cwd && !inRecent
            p.onClick = { [weak self] in
                guard let self, FileManager.default.fileExists(atPath: fav) else { return }
                self.cd(fav)
            }
            addSubview(p)
            favPills.append(p)
        }
        updateStarTitle()
        needsLayout = true
    }

    func controlTextDidBeginEditing(_ obj: Notification) {
        guard (obj.object as AnyObject?) === searchField else { return }
        searchField.currentEditor()?.selectAll(nil)
    }
    func controlTextDidEndEditing(_ obj: Notification) {
        guard (obj.object as AnyObject?) === searchField else { return }
        if query.isEmpty { showCwdInFilter() }
    }

    func controlTextDidChange(_ obj: Notification) {
        guard (obj.object as AnyObject?) === searchField else { return }
        query = searchField.stringValue
        selection = 0
        refilter()
    }
    private func setStatus(_ s: String) {
        statusLine.stringValue = s
        onStatus?(s)
    }
    private func updateStatus() {
        let items = rows.filter { $0.name != ".." }.count
        let count = "\(items) item\(items == 1 ? "" : "s")"
        switch mode {
        case .all where inRecent:
            let vi = virtualIndex ?? 0
            setStatus("\(count) · " + (virtualLists.indices.contains(vi) ? virtualLists[vi].status : "newest first"))
        case .all:
            setStatus("\(count) · sorted by \(sortKey.label.lowercased())")
        case .terminal(let dir):
            setStatus("↵ open a terminal in \(displayPath(dir))")
        case .local:
            setStatus(items == 0 ? "no matches — try a path (~/…) or **/name to search subfolders"
                                 : "\(count) · ↵ open")
        case .dir(let dir, let pat):
            var isDir: ObjCBool = false
            guard FileManager.default.fileExists(atPath: dir, isDirectory: &isDir), isDir.boolValue else {
                setStatus("no such folder: \(displayPath(dir))")
                return
            }
            let exact = (dir as NSString).appendingPathComponent(pat)
            if !pat.isEmpty, FileManager.default.fileExists(atPath: exact, isDirectory: &isDir) {
                setStatus(isDir.boolValue ? "↵ cd \(displayPath(exact)) · ⇥ list it"
                                          : "↵ open \(displayPath(exact))")
            } else {
                setStatus("\(count) in \(displayPath(dir)) · ⇥ complete · ↵ open")
            }
        case .recursive:
            break
        }
    }

    func control(_ control: NSControl, textView: NSTextView,
                 doCommandBy commandSelector: Selector) -> Bool {
        guard control === searchField else { return false }
        switch commandSelector {
        case #selector(NSResponder.insertNewline(_:)):
            if case .terminal(let dir) = parseQuery(searchField.stringValue) {
                openTerminal(dir)
                return true
            }
            if jumpToQueryPath() { return true }
            guard rows.indices.contains(listPane.selection) else {
                setStatus("nothing to open")
                return true
            }
            if let w = window { w.makeFirstResponder(listPane) }
            openIndex(listPane.selection)
            return true
        case #selector(NSResponder.insertTab(_:)):
            guard !query.isEmpty, rows.indices.contains(listPane.selection) else { return true }
            let e = rows[listPane.selection]
            guard e.name != ".." else { return true }
            let completed = displayPath(e.path) + (e.isDir ? "/" : "")
            searchField.stringValue = completed
            query = completed
            selection = 0
            refilter()
            textView.selectedRange = NSRange(location: (completed as NSString).length, length: 0)
            return true
        case #selector(NSResponder.moveUp(_:)):
            if let w = window { w.makeFirstResponder(listPane) }
            listPane.moveSelection(-1)
            return true
        case #selector(NSResponder.moveDown(_:)):
            if let w = window { w.makeFirstResponder(listPane) }
            listPane.moveSelection(1)
            return true
        default:
            return false
        }
    }
}

final class PopupStatusBar: NSView {
    var config: PopupConfig
    var text: String = "" { didSet { needsDisplay = true } }
    var isError = false { didSet { needsDisplay = true } }

    override var isFlipped: Bool { true }

    init(config: PopupConfig) {
        self.config = config
        super.init(frame: .zero)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) not supported") }

    override func draw(_ dirty: NSRect) {
        let c = config.colors
        let r = NSRect(x: 1, y: 1, width: bounds.width - 2, height: bounds.height - 2)
        let p = NSBezierPath(roundedRect: r, xRadius: config.buttonRadius,
                             yRadius: config.buttonRadius)
        (isError ? c.tone(.danger).withAlphaComponent(0.16) : c.crust.withAlphaComponent(0.85)).setFill()
        p.fill()
        let lead = isError ? c.tone(.danger) : c.accentOn
        NSGraphicsContext.current?.saveGraphicsState()
        p.addClip()
        lead.setFill()
        NSRect(x: r.minX, y: r.minY, width: 4, height: r.height).fill()
        NSGraphicsContext.current?.restoreGraphicsState()
        guard !text.isEmpty else { return }
        let para = NSMutableParagraphStyle()
        para.lineBreakMode = .byTruncatingTail
        let attrs: [NSAttributedString.Key: Any] = [
            .font: NSFont.monospacedSystemFont(ofSize: 10.5, weight: .regular),
            .foregroundColor: isError ? c.tone(.danger) : c.text.withAlphaComponent(0.9),
            .paragraphStyle: para,
        ]
        let s = text as NSString
        let lineH = s.size(withAttributes: attrs).height
        s.draw(with: NSRect(x: r.minX + 12, y: r.midY - lineH / 2,
                            width: max(20, r.width - 22), height: lineH),
               options: [.usesLineFragmentOrigin], attributes: attrs)
    }
}

public enum HeaderStyle: String, CaseIterable {
    case quiet, flat, edge, stripe, tinted, glow, aurora

    public var label: String {
        switch self {
        case .quiet: return "Quiet"
        case .flat: return "Flat"
        case .edge: return "Accent Edge"
        case .stripe: return "Accent Stripe"
        case .tinted: return "Tinted"
        case .glow: return "Glow"
        case .aurora: return "Aurora"
        }
    }

    public static var current: HeaderStyle = .flat {
        didSet { if current != oldValue { PopupChrome.redrawAll() } }
    }
}

enum CapsuleStyle {
    static func track(_ r: NSRect, _ c: PopupColors) {
        let p = NSBezierPath(roundedRect: r, xRadius: r.height / 2, yRadius: r.height / 2)
        c.text.withAlphaComponent(c.isLight ? 0.07 : 0.06).setFill()
        p.fill()
    }
    static func chip(_ r: NSRect, _ c: PopupColors, on: Bool, hover: Bool) {
        let chip = NSBezierPath(roundedRect: r, xRadius: r.height / 2, yRadius: r.height / 2)
        if on {
            NSGraphicsContext.saveGraphicsState()
            let sh = NSShadow()
            sh.shadowColor = NSColor.black.withAlphaComponent(c.isLight ? 0.18 : 0.35)
            sh.shadowOffset = NSSize(width: 0, height: -1)
            sh.shadowBlurRadius = 2.5
            sh.set()
            (c.isLight ? NSColor.white
                       : c.surface1.blended(withFraction: hover ? 0.10 : 0.04, of: ButtonStyle.opaque(c.text)) ?? c.surface1)
                .setFill()
            chip.fill()
            NSGraphicsContext.restoreGraphicsState()
            c.text.withAlphaComponent(c.isLight ? 0.08 : 0.10).setStroke()
            chip.lineWidth = 0.5
            chip.stroke()
        } else if hover {
            c.text.withAlphaComponent(0.08).setFill()
            chip.fill()
        }
    }
    static func primaryChip(_ r: NSRect, _ c: PopupColors, hover: Bool, pressed: Bool) {
        let a = c.accentOn
        (pressed ? a.blended(withFraction: 0.18, of: .black) ?? a
                 : hover ? a.blended(withFraction: 0.12, of: .white) ?? a : a).setFill()
        NSBezierPath(roundedRect: r, xRadius: r.height / 2, yRadius: r.height / 2).fill()
    }
}

final class CapsuleButtons: NSView, PopupThemeable {
    struct Item { var title: String; var symbol: String?; var primary: Bool; var action: () -> Void }
    var colors = PopupThemeDefaults.colors { didSet { needsDisplay = true } }
    var items: [Item] { didSet { invalidateIntrinsicContentSize(); needsDisplay = true } }
    private var hover: Int?
    private var pressed: Int?
    private let font = NSFont.systemFont(ofSize: 12.5, weight: .semibold)
    override var isFlipped: Bool { true }
    func applyColors(_ c: PopupColors) { colors = c }

    init(_ items: [Item]) {
        self.items = items
        super.init(frame: .zero)
    }
    required init?(coder: NSCoder) { fatalError() }
    private var widths: [CGFloat] {
        items.map { ceil(($0.title as NSString).size(withAttributes: [.font: font]).width) + ($0.symbol == nil ? 30 : 48) }
    }
    override var intrinsicContentSize: NSSize { NSSize(width: widths.reduce(8, +) + CGFloat(max(0, items.count - 1)) * 2, height: 34) }
    private func rect(_ i: Int) -> NSRect {
        let x = 4 + widths.prefix(i).reduce(0, +) + CGFloat(i) * 2
        return NSRect(x: x, y: 4, width: widths[i], height: bounds.height - 8)
    }
    override func draw(_ dirty: NSRect) {
        let c = colors
        CapsuleStyle.track(bounds.insetBy(dx: 0.5, dy: 0.5), c)
        for (i, it) in items.enumerated() {
            let r = rect(i)
            let fg: NSColor
            if it.primary {
                CapsuleStyle.primaryChip(r, c, hover: hover == i, pressed: pressed == i)
                fg = c.onAccent
            } else {
                CapsuleStyle.chip(r, c, on: false, hover: hover == i || pressed == i)
                fg = hover == i ? c.text : c.text.withAlphaComponent(0.85)
            }
            var x = r.minX + 15
            if let sym = it.symbol {
                ButtonStyle.symbol(sym, in: NSRect(x: x, y: r.minY, width: 14, height: r.height), color: fg, size: 11)
                x += 18
            }
            let a: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: fg]
            let sz = (it.title as NSString).size(withAttributes: a)
            (it.title as NSString).draw(at: NSPoint(x: x, y: r.midY - sz.height / 2), withAttributes: a)
        }
    }
    private func index(_ e: NSEvent) -> Int? {
        let p = convert(e.locationInWindow, from: nil)
        return items.indices.first { rect($0).contains(p) }
    }
    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect: bounds, options: [.mouseMoved, .mouseEnteredAndExited, .activeAlways, .inVisibleRect],
                                       owner: self, userInfo: nil))
    }
    override func mouseMoved(with e: NSEvent) { let i = index(e); if i != hover { hover = i; needsDisplay = true } }
    override func mouseExited(with e: NSEvent) { hover = nil; needsDisplay = true }
    override func mouseDown(with e: NSEvent) { pressed = index(e); needsDisplay = true }
    override func mouseUp(with e: NSEvent) {
        let i = index(e)
        let was = pressed
        pressed = nil
        needsDisplay = true
        if let i, i == was { items[i].action() }
    }
}

final class PopupChrome: NSView {
    var config: PopupConfig
    var headerColorOverride: NSColor? {
        didSet { needsDisplay = true }
    }
    var zoom: CGFloat = 1.0
    var dragHeaderHeight: CGFloat = 0
    var dragAnywhere: Bool = false
    var reservedRect: NSRect = .zero
    var headerTitle: String?
    var headerIcon: NSImage?
    var itemCount: String?
    var footerText: String?
    var meterEnabled = false {
        didSet {
            window?.invalidateCursorRects(for: self)
            needsDisplay = true
        }
    }
    var meterState: Int = 0
    var meterLevel: Float = 0
    var meterElapsed: TimeInterval = 0
    var meterRecordRect: NSRect = .zero
    var meterPauseRect: NSRect = .zero
    var onMeterRecord: (() -> Void)?
    var onMeterPause: (() -> Void)?
    let meterBarHeight: CGFloat = 36
    var copyPathLabel = "copy path"
    var copyConfigLabel = "copy config"
    var extraButtons: [(label: String, id: Int)] = []
    var navIcons: [(image: NSImage, id: Int, tip: String)] = [] {
        didSet { needsDisplay = true }
    }
    var navOn: Int? { didSet { if navOn != oldValue { needsDisplay = true } } }
    struct WorkspaceCell {
        let key: String
        let icons: [NSImage]
        let extra: String?
        let unread: String
        let focused: Bool
    }
    static let workspaceBase = 1000
    static var workspaceCells: [WorkspaceCell] = []
    private let wsCellH: CGFloat = 24
    private var wsRight: CGFloat = 0
    private func wsCellRects() -> [NSRect] {
        guard !navIcons.isEmpty, dragHeaderHeight > 0, wsRight > 0 else { return [] }
        let widths = Self.workspaceCells.map { c -> CGFloat in
            var w: CGFloat = 10 + 9 + 6 + CGFloat(c.icons.count) * 19 + 4
            if c.extra != nil { w += 18 }
            if !c.unread.isEmpty { w += 22 }
            return w
        }
        let minX = navRect(navIcons.count - 1).maxX + 24
        var first = 0
        func total(_ f: Int) -> CGFloat { widths[f...].reduce(0, +) + CGFloat(max(0, widths.count - f - 1)) * 2 }
        while first < widths.count, wsRight - 3 - total(first) < minX { first += 1 }
        guard first < widths.count else { return [] }
        var x = wsRight - 3 - total(first)
        let y = (dragHeaderHeight - wsCellH) / 2
        var out = [NSRect](repeating: .zero, count: first)
        for w in widths[first...] {
            out.append(NSRect(x: x, y: y, width: w, height: wsCellH))
            x += w + 2
        }
        return out
    }
    private func drawWorkspaceCells() {
        let c = config.colors
        let rects = wsCellRects()
        guard let firstShown = rects.firstIndex(where: { $0 != .zero }), let last = rects.last else { return }
        let first = rects[firstShown]
        CapsuleStyle.track(NSRect(x: first.minX - 3, y: first.minY - 3,
                                  width: last.maxX - first.minX + 6, height: first.height + 6), c)
        for (i, cell) in Self.workspaceCells.enumerated() where i >= firstShown && i < rects.count {
            let r = rects[i]
            let id = Self.workspaceBase + i
            CapsuleStyle.chip(r, c, on: cell.focused, hover: hoveredSegment == id)
            let ka: [NSAttributedString.Key: Any] = [
                .font: NSFont.systemFont(ofSize: 11, weight: .bold),
                .foregroundColor: cell.focused ? c.text : c.dim,
            ]
            let ks = (cell.key as NSString).size(withAttributes: ka)
            (cell.key as NSString).draw(at: NSPoint(x: r.minX + 10, y: r.midY - ks.height / 2), withAttributes: ka)
            var x = r.minX + 10 + 9 + 6
            for img in cell.icons {
                popupDrawImage(img, in: NSRect(x: x, y: r.midY - 8, width: 16, height: 16),
                               fraction: cell.focused || hoveredSegment == id ? 1 : 0.8)
                x += 19
            }
            if let extra = cell.extra {
                let ea: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 10), .foregroundColor: c.dim]
                let es = (extra as NSString).size(withAttributes: ea)
                (extra as NSString).draw(at: NSPoint(x: x, y: r.midY - es.height / 2), withAttributes: ea)
                x += 18
            }
            if !cell.unread.isEmpty {
                let ba: [NSAttributedString.Key: Any] = [
                    .font: NSFont.systemFont(ofSize: 9.5, weight: .semibold), .foregroundColor: NSColor.white,
                ]
                let bs = (cell.unread as NSString).size(withAttributes: ba)
                let bw = max(15, bs.width + 9)
                let br = NSRect(x: x, y: r.midY - 7.5, width: bw, height: 15)
                c.tone(.danger).setFill()
                NSBezierPath(roundedRect: br, xRadius: 7.5, yRadius: 7.5).fill()
                (cell.unread as NSString).draw(at: NSPoint(x: br.midX - bs.width / 2, y: br.midY - bs.height / 2),
                                               withAttributes: ba)
            }
            extraButtonRects[id] = r
        }
    }
    private let navSize: CGFloat = 24
    private let navW: CGFloat = 32
    private var navTipRects: [NSRect] = []
    private var navTipOwners: [NSString] = []
    func navRect(_ i: Int) -> NSRect {
        let x0 = headerIcon != nil ? iconButtonRect.maxX + 6
            : config.headerCloseButton ? closeButtonRect.maxX + 6 : 6
        return NSRect(x: x0 + 3 + CGFloat(i) * (navW + 2), y: (dragHeaderHeight - navSize) / 2,
                      width: navW, height: navSize)
    }
    var headerOrder: [Int]?
    var extraButtonRects: [Int: NSRect] = [:]
    var activeButtonIDs: Set<Int> = []
    var copyButtonRect: NSRect = .zero
    var configButtonRect: NSRect = .zero
    var copyRowsButtonRect: NSRect = .zero
    var copyRowsLabel: String?
    private var resize = PopupBackdrop.Resize()
    private var feedback: Int = 0
    private var feedbackTimer: DispatchWorkItem?
    private var hoveredSegment: Int?
    var iconButtonRect: NSRect {
        NSRect(x: config.headerCloseButton ? 32 : 6, y: (dragHeaderHeight - 22) / 2, width: 40, height: 22)
    }
    var closeButtonRect: NSRect {
        config.headerCloseButton && dragHeaderHeight > 0
            ? NSRect(x: 6, y: (dragHeaderHeight - 22) / 2, width: 22, height: 22) : .zero
    }
    var leftInset: CGFloat {
        if !navIcons.isEmpty, dragHeaderHeight > 0 { return navRect(navIcons.count - 1).maxX + 10 }
        if headerIcon != nil { return iconButtonRect.maxX + 8 }
        return config.headerCloseButton ? closeButtonRect.maxX + 8 : 10
    }
    var iconHovered = false { didSet { if iconHovered != oldValue { needsDisplay = true } } }
    var closeHovered = false { didSet { if closeHovered != oldValue { needsDisplay = true } } }
    var iconMenuOpen = false { didSet { if iconMenuOpen != oldValue { needsDisplay = true } } }
    private var headerSegRects: [(Int, NSRect)] = []
    private var trackingArea: NSTrackingArea?

    override var isFlipped: Bool { true }

    override func resetCursorRects() {
        super.resetCursorRects()
        if meterEnabled {
            addCursorRect(NSRect(x: 0, y: bounds.height - meterBarHeight,
                                 width: bounds.width, height: meterBarHeight),
                          cursor: .arrow)
        }
        if dragHeaderHeight > 0 {
            addCursorRect(NSRect(x: 0, y: 0, width: bounds.width,
                                 height: dragHeaderHeight), cursor: .arrow)
        }
    }

    init(config: PopupConfig) {
        self.config = config
        super.init(frame: .zero)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) not supported") }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let t = trackingArea { removeTrackingArea(t) }
        let t = NSTrackingArea(rect: bounds,
                               options: [.mouseMoved, .mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect],
                               owner: self, userInfo: nil)
        addTrackingArea(t)
        trackingArea = t
    }

    override func mouseEntered(with event: NSEvent) {}
    override func mouseExited(with event: NSEvent) {
        iconHovered = false
        closeHovered = false
        if hoveredSegment != nil {
            hoveredSegment = nil
            needsDisplay = true
        }
    }

    override func mouseMoved(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        var hovered: Int?
        if dragHeaderHeight > 0, p.y <= dragHeaderHeight {
            for (fb, rect) in headerSegRects where rect.contains(p) {
                hovered = fb
                break
            }
            for (i, n) in navIcons.enumerated() where navRect(i).contains(p) {
                hovered = n.id
            }
            for (i, r) in wsCellRects().enumerated() where r != .zero && r.contains(p) {
                hovered = Self.workspaceBase + i
            }
        }
        iconHovered = headerIcon != nil && dragHeaderHeight > 0 && iconButtonRect.contains(p)
        closeHovered = closeButtonRect.contains(p)
        if hovered != hoveredSegment {
            hoveredSegment = hovered
            needsDisplay = true
        }
    }

    private var customResize: Bool {
        config.enableResize && !(window?.styleMask.contains(.resizable) ?? false)
    }

    private func resizeEdges(at p: NSPoint) -> PopupBackdrop.Edge {
        var e = PopupBackdrop.Edge.at(p, in: bounds.size)
        if dragHeaderHeight > 0 {
            e.remove(.top)
        }
        return e
    }

    override func hitTest(_ point: NSPoint) -> NSView? {
        let e = resizeEdges(at: point)
        if customResize && !e.isEmpty { return self }
        if meterEnabled && point.y >= bounds.height - meterBarHeight { return self }
        if dragHeaderHeight > 0 && point.y <= dragHeaderHeight { return self }
        if dragAnywhere && !reservedRect.contains(point) { return self }
        return nil
    }

    override func mouseDown(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        if meterEnabled {
            if meterState != 3, meterRecordRect.contains(p) {
                onMeterRecord?()
                return
            }
            if meterState == 1 || meterState == 2, meterPauseRect.contains(p) {
                onMeterPause?()
                return
            }
        }
        if customResize, let win = window, !resizeEdges(at: p).isEmpty {
            resize.begin(resizeEdges(at: p), in: win)
        } else {
            window?.performDrag(with: event)
        }
    }

    override func mouseDragged(with event: NSEvent) {
        if let win = window { resize.drag(win) }
    }

    override func mouseUp(with event: NSEvent) {
        if resize.end() { window?.invalidateShadow() }
        super.mouseUp(with: event)
    }

    private func headerButtonFont(_ label: String) -> NSFont {
        let scalars = label.unicodeScalars
        let pua = { (v: UInt32) in
            (0xE000...0xF8FF).contains(v)
                || (0xF0000...0xFFFFD).contains(v)
                || (0x100000...0x10FFFD).contains(v)
        }
        let needsNerd = scalars.contains { pua($0.value) }
        if needsNerd, let f = NSFont(name: config.terminalFont, size: config.buttonFontSize) {
            if scalars.allSatisfy({ pua($0.value) }) {
                return NSFont(name: config.terminalFont, size: config.buttonFontSize + 3.5) ?? f
            }
            return f
        }
        return NSFont.systemFont(ofSize: config.buttonFontSize, weight: .semibold)
    }

    private func headerSegs() -> [(text: String, fb: Int, w: CGFloat)] {
        var labels: [(String, Int)] = []
        if !copyPathLabel.isEmpty { labels.append((copyPathLabel, 1)) }
        if !copyConfigLabel.isEmpty { labels.append((copyConfigLabel, 2)) }
        if let cr = copyRowsLabel {
            labels.append((cr, 3))
        }
        for b in extraButtons {
            labels.append((b.label, b.id))
        }
        if let order = headerOrder {
            labels.sort { a, b in
                let ia = order.firstIndex(of: a.1) ?? Int.max
                let ib = order.firstIndex(of: b.1) ?? Int.max
                return ia < ib
            }
        }
        var segs: [(text: String, fb: Int, w: CGFloat)] = []
        for (label, fb) in labels {
            let text = (feedback == fb ? "✓ " : "") + label
            let attrs: [NSAttributedString.Key: Any] = [.font: headerButtonFont(text)]
            let w = (text as NSString).size(withAttributes: attrs).width + 30
            segs.append((text, fb, w))
        }
        return segs
    }

    func neededWidth() -> CGFloat {
        var w: CGFloat = 34 + CGFloat(navIcons.count) * (navW + 2) + 6 + (navIcons.isEmpty ? 0 : 10)
        let metaAttrs: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: 10),
        ]
        for t in [itemCount, footerText].compactMap({ $0 }) {
            w += (t as NSString).size(withAttributes: metaAttrs).width + 12
        }
        if let title = headerTitle {
            let sz = (title as NSString).size(withAttributes: [
                .font: config.rowFont(13.5, weight: .bold),
            ])
            w += sz.width + 48
        }
        for seg in headerSegs() { w += seg.w + 1 }
        return w + 20
    }

    static func redrawAll() {
        func walk(_ v: NSView) {
            if let c = v as? PopupChrome { c.needsDisplay = true }
            v.subviews.forEach(walk)
        }
        guard let app = NSApp else { return }
        for w in app.windows { if let v = w.contentView { walk(v) } }
    }

    private func drawHeaderBackground(_ header: NSRect) {
        let base = headerColorOverride ?? config.headerColor ?? config.colors.background
        let alpha = (base.usingColorSpace(.sRGB) ?? base).alphaComponent
        let accent = config.colors.accent, accent2 = config.colors.palette.accent2
        func mix(_ c: NSColor, _ t: CGFloat) -> NSColor {
            let b = base.withAlphaComponent(1)
            return (b.blended(withFraction: t, of: c.withAlphaComponent(1)) ?? b).withAlphaComponent(alpha)
        }
        func bottomLine(_ c: NSColor, _ width: CGFloat) {
            c.setFill()
            NSRect(x: 0, y: header.maxY - width, width: header.width, height: width).fill()
        }
        switch HeaderStyle.current {
        case .quiet:
            config.colors.background.setFill(); header.fill()
            bottomLine(config.colors.hairline, 1)
        case .flat:
            base.setFill(); header.fill()
            bottomLine(config.colors.hairline, 1)
        case .edge:
            base.setFill(); header.fill()
            bottomLine(accent.withAlphaComponent(0.9), 2)
        case .stripe:
            base.setFill(); header.fill()
            bottomLine(config.colors.hairline, 1)
            NSGradient(colors: [accent, accent2])?
                .draw(in: NSRect(x: 0, y: 0, width: header.width, height: 3), angle: 0)
        case .tinted:
            mix(accent, 0.20).setFill(); header.fill()
            bottomLine(accent.withAlphaComponent(0.35), 1)
        case .glow:
            NSGradient(starting: mix(accent, 0.40), ending: base)?.draw(in: header, angle: 90)
            bottomLine(accent.withAlphaComponent(0.55), 1)
        case .aurora:
            NSGradient(colors: [mix(accent, 0.36), mix(accent2, 0.26), base],
                       atLocations: [0, 0.45, 1], colorSpace: .sRGB)?.draw(in: header, angle: 0)
            bottomLine(accent2.withAlphaComponent(0.35), 1)
        }
    }

    override func draw(_ dirtyRect: NSRect) {
        guard dragHeaderHeight > 0 else { return }
        let header = NSRect(x: 0, y: 0, width: bounds.width, height: dragHeaderHeight)
        drawHeaderBackground(header)
        let segs = headerSegs()
        var meta = ""
        for t in [itemCount, footerText].compactMap({ $0 }) {
            meta += meta.isEmpty ? t : "   " + t
        }
        let metaAttrs: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: 9),
            .foregroundColor: config.colors.dim,
        ]
        let metaWidth = (meta as NSString).size(withAttributes: metaAttrs).width
        let stretch = config.stretchHeaderButtons && !segs.isEmpty
        let leftContent: CGFloat = stretch
            ? leftInset + (meta.isEmpty ? 0 : metaWidth + 8)
            : 0
        let naturalBarW = segs.map { $0.w }.reduce(0, +)
            + CGFloat(max(0, segs.count - 1))
        var buttonsWidth: CGFloat = 10 + naturalBarW
        if stretch {
            buttonsWidth = max(naturalBarW, bounds.width - 10 - leftContent)
        }
        if let title = headerTitle {
            let attrs: [NSAttributedString.Key: Any] = [
                .font: config.rowFont(13.5, weight: .bold),
                .foregroundColor: config.colors.text,
            ]
            let s = title as NSString
            let sz = s.size(withAttributes: attrs)
            let padX: CGFloat = 12
            let padY: CGFloat = 4
            let center = max(sz.width / 2 + padX,
                             (bounds.width - buttonsWidth) / 2)
            let pill = NSRect(x: center - sz.width / 2 - padX,
                              y: 4,
                              width: sz.width + padX * 2,
                              height: sz.height + padY * 2)
            if config.titlePill {
                let path = NSBezierPath(roundedRect: pill, xRadius: 7, yRadius: 7)
                config.colors.highlight.withAlphaComponent(0.35).setFill()
                path.fill()
            }
            s.draw(at: NSPoint(x: pill.midX - sz.width / 2,
                               y: pill.midY - sz.height / 2),
                   withAttributes: attrs)
        }
        drawCloseGlyph()
        if let icon = headerIcon { drawIconMenuButton(icon) }
        extraButtonRects = [:]
        if !navIcons.isEmpty { drawNavIcons() }
        if !navIcons.isEmpty {
            wsRight = stretch ? bounds.width - 10 : bounds.width - buttonsWidth - 10
            drawWorkspaceCells()
        }
        let barH: CGFloat = 20
        let barW = stretch ? max(naturalBarW, bounds.width - 10 - leftContent)
                           : naturalBarW
        let barRect = NSRect(x: stretch ? leftContent : bounds.width - 10 - barW,
                             y: (dragHeaderHeight - barH) / 2,
                             width: barW, height: barH)
        if !segs.isEmpty {
            ButtonStyle.draw(barRect, .idle, config.colors, radius: config.buttonRadius)
        }
        let perSegExtra = stretch ? max(0, barW - naturalBarW) / CGFloat(segs.count) : 0
        headerSegRects = []
        copyButtonRect = .zero
        configButtonRect = .zero
        var sx = barRect.minX
        for (i, seg) in segs.enumerated() {
            let segRect = NSRect(x: sx, y: barRect.minY,
                                 width: seg.w + perSegExtra, height: barH)
            let active = feedback == seg.fb
            let persistentOn = seg.fb >= 10 && activeButtonIDs.contains(seg.fb)
            let hovered = hoveredSegment == seg.fb
            headerSegRects.append((seg.fb, segRect))
            let st: ButtonState = active ? .pressed
                : persistentOn ? (hovered ? .onHover : .on)
                : hovered ? .hover : .idle
            if i > 0, st == .idle, !(feedback == segs[i - 1].fb
                    || (segs[i - 1].fb >= 10 && activeButtonIDs.contains(segs[i - 1].fb))
                    || hoveredSegment == segs[i - 1].fb) {
                config.colors.text.withAlphaComponent(0.10).setStroke()
                let d = NSBezierPath()
                d.lineWidth = 1
                d.move(to: NSPoint(x: segRect.minX, y: barRect.minY + 5))
                d.line(to: NSPoint(x: segRect.minX, y: barRect.maxY - 5))
                d.stroke()
            }
            if st != .idle {
                ButtonStyle.draw(segRect.insetBy(dx: 2, dy: 2), st, config.colors,
                                 radius: config.buttonRadius - 2)
            }
            if seg.fb == 1 { copyButtonRect = segRect }
            if seg.fb == 2 { configButtonRect = segRect }
            if seg.fb == 3 { copyRowsButtonRect = segRect }
            if seg.fb >= 10 { extraButtonRects[seg.fb] = segRect }
            let attrs: [NSAttributedString.Key: Any] = [
                .font: headerButtonFont(seg.text),
                .foregroundColor: ButtonStyle.text(st, config.colors),
            ]
            let s = seg.text as NSString
            let sz = s.size(withAttributes: attrs)
            s.draw(at: NSPoint(x: segRect.midX - sz.width / 2,
                               y: segRect.midY - sz.height / 2),
                   withAttributes: attrs)
            sx += segRect.width + 1
        }
        if !meta.isEmpty {
            let x0: CGFloat = leftInset
            let strip = wsCellRects().first(where: { $0 != .zero }).map { wsRight - $0.minX + 3 + 8 } ?? 0
            let maxW = max(60, bounds.width - buttonsWidth - x0 - 10 - strip)
            var text = meta
            if (text as NSString).size(withAttributes: metaAttrs).width > maxW {
                while (text as NSString).size(withAttributes: metaAttrs).width > maxW {
                    text.removeLast()
                }
                text += "…"
            }
            let sz = (text as NSString).size(withAttributes: metaAttrs)
            (text as NSString).draw(at: NSPoint(x: x0, y: (dragHeaderHeight - sz.height) / 2),
                                    withAttributes: metaAttrs)
        }
        if meterEnabled { drawMeterBar() }
    }

    private func drawCloseGlyph() {
        let closeRect = closeButtonRect
        guard !closeRect.isEmpty else { return }
        let dot = closeRect.insetBy(dx: 3, dy: 3)
        let xColor: NSColor
        if closeHovered {
            let danger = config.colors.tone(.danger)
            danger.setFill()
            NSBezierPath(ovalIn: dot).fill()
            xColor = ButtonStyle.readable(on: danger, preferred: config.colors.crust)
        } else {
            ButtonStyle.draw(dot, .idle, config.colors, radius: dot.height / 2, flat: true)
            xColor = config.colors.dim
        }
        let r: CGFloat = 3.2
        let x = NSBezierPath()
        x.move(to: NSPoint(x: dot.midX - r, y: dot.midY - r))
        x.line(to: NSPoint(x: dot.midX + r, y: dot.midY + r))
        x.move(to: NSPoint(x: dot.midX + r, y: dot.midY - r))
        x.line(to: NSPoint(x: dot.midX - r, y: dot.midY + r))
        x.lineWidth = 1.6
        x.lineCapStyle = .round
        xColor.setStroke()
        x.stroke()
    }

    private func drawIconMenuButton(_ icon: NSImage) {
        let isz: CGFloat = 16
        let badge = iconButtonRect
        let st: ButtonState = iconMenuOpen ? .on : iconHovered ? .hover : .idle
        ButtonStyle.draw(badge, st, config.colors, radius: config.buttonRadius, flat: true)
        popupDrawImage(icon, in: NSRect(x: badge.minX + 6,
                                        y: badge.midY - isz / 2,
                                        width: isz, height: isz))
        ButtonStyle.chevron(in: NSRect(x: badge.maxX - 13, y: badge.midY - 4, width: 8, height: 8),
                            color: ButtonStyle.text(st, config.colors))
    }

    private func drawNavIcons() {
        let c = config.colors
        let first = navRect(0), last = navRect(navIcons.count - 1)
        let well = NSRect(x: first.minX - 3, y: first.minY - 3,
                          width: last.maxX - first.minX + 6, height: first.height + 6)
        CapsuleStyle.track(well, c)
        var tips: [NSRect] = []
        for (i, n) in navIcons.enumerated() {
            let r = navRect(i)
            let on = navOn == n.id, hov = hoveredSegment == n.id
            CapsuleStyle.chip(r, c, on: on, hover: hov)
            let isz: CGFloat = 16
            popupDrawImage(n.image, in: NSRect(x: r.midX - isz / 2, y: r.midY - isz / 2,
                                               width: isz, height: isz),
                           fraction: on || hov ? 1 : 0.7)
            extraButtonRects[n.id] = r
            tips.append(r)
        }
        if tips != navTipRects {
            navTipRects = tips
            removeAllToolTips()
            navTipOwners = navIcons.map { $0.tip as NSString }
            for (i, r) in tips.enumerated() { addToolTip(r, owner: navTipOwners[i], userData: nil) }
        }
    }

    private func drawMeterBar() {
        let strip = NSRect(x: 0, y: bounds.height - meterBarHeight,
                           width: bounds.width, height: meterBarHeight)
        config.colors.background.withAlphaComponent(1).setFill()
        strip.fill()
        NSColor.systemRed.withAlphaComponent(0.16).setFill()
        strip.fill()
        config.colors.dim.withAlphaComponent(0.35).setStroke()
        let hair = NSBezierPath()
        hair.lineWidth = 1
        hair.move(to: NSPoint(x: 0, y: strip.minY + 0.5))
        hair.line(to: NSPoint(x: bounds.width, y: strip.minY + 0.5))
        hair.stroke()
        let t = Date().timeIntervalSinceReferenceDate
        if meterState == 3 {
            let attrs: [NSAttributedString.Key: Any] = [
                .font: NSFont.systemFont(ofSize: 11, weight: .medium),
                .foregroundColor: config.colors.dim,
            ]
            let s = "transcribing…" as NSString
            let sz = s.size(withAttributes: attrs)
            s.draw(at: NSPoint(x: (bounds.width - sz.width) / 2,
                               y: strip.midY - sz.height / 2), withAttributes: attrs)
        } else {
            let active = meterState == 1 || meterState == 2
            meterRecordRect = NSRect(x: 12, y: strip.midY - 13, width: 26, height: 26)
            let mid = NSPoint(x: meterRecordRect.midX, y: meterRecordRect.midY)
            if active {
                NSColor.systemRed.setFill()
                NSBezierPath(roundedRect: NSRect(x: mid.x - 7, y: mid.y - 7,
                                                 width: 14, height: 14),
                             xRadius: 3, yRadius: 3).fill()
            } else {
                let pulse = 0.65 + 0.35 * sin(t * 4)
                NSColor.systemRed.withAlphaComponent(CGFloat(pulse)).setStroke()
                let ring = NSBezierPath(ovalIn: NSRect(x: mid.x - 10, y: mid.y - 10,
                                                       width: 20, height: 20))
                ring.lineWidth = 2
                ring.stroke()
                NSColor.systemRed.setFill()
                NSBezierPath(ovalIn: NSRect(x: mid.x - 6, y: mid.y - 6,
                                            width: 12, height: 12)).fill()
            }
            if active {
                meterPauseRect = NSRect(x: 46, y: strip.midY - 11, width: 30, height: 22)
                NSColor.systemRed.withAlphaComponent(0.35).setFill()
                NSBezierPath(roundedRect: meterPauseRect,
                             xRadius: config.buttonRadius - 1,
                             yRadius: config.buttonRadius - 1).fill()
                if meterState == 1 {
                    NSColor.white.withAlphaComponent(0.9).setFill()
                    NSRect(x: meterPauseRect.midX - 7, y: meterPauseRect.midY - 5,
                           width: 4, height: 10).fill()
                    NSRect(x: meterPauseRect.midX + 3, y: meterPauseRect.midY - 5,
                           width: 4, height: 10).fill()
                } else {
                    NSColor.white.withAlphaComponent(0.9).setFill()
                    let tri = NSBezierPath()
                    tri.move(to: NSPoint(x: meterPauseRect.midX - 4, y: meterPauseRect.midY - 5))
                    tri.line(to: NSPoint(x: meterPauseRect.midX - 4, y: meterPauseRect.midY + 5))
                    tri.line(to: NSPoint(x: meterPauseRect.midX + 6, y: meterPauseRect.midY))
                    tri.close()
                    tri.fill()
                }
            }
            let lvl = CGFloat(min(1, max(0, meterLevel)))
            var bx: CGFloat = 88
            for i in 0..<18 {
                let env = 0.35 + 0.65 * abs(sin(t * 4 + Double(i) * 0.55))
                let h = max(2, lvl * 16 * env)
                NSColor.systemRed.withAlphaComponent(0.9).setFill()
                NSBezierPath(roundedRect: NSRect(x: bx, y: strip.midY - h / 2,
                                                 width: 4, height: h),
                             xRadius: 1.5, yRadius: 1.5).fill()
                bx += 7
            }
            let attrs: [NSAttributedString.Key: Any] = [
                .font: NSFont.monospacedDigitSystemFont(ofSize: 12, weight: .semibold),
                .foregroundColor: config.colors.text.withAlphaComponent(0.9),
            ]
            let s = String(format: "%d:%02d", Int(meterElapsed) / 60,
                           Int(meterElapsed) % 60) as NSString
            let sz = s.size(withAttributes: attrs)
            s.draw(at: NSPoint(x: bounds.width - sz.width - 14,
                               y: strip.midY - sz.height / 2), withAttributes: attrs)
        }
    }

    func showCopiedFeedback(_ which: Int = 1) {
        feedback = which
        needsDisplay = true
        feedbackTimer?.cancel()
        let t = DispatchWorkItem { [weak self] in
            self?.feedback = 0
            self?.needsDisplay = true
        }
        feedbackTimer = t
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.2, execute: t)
    }
}

final class TerminalAutoRestart: NSObject,
                                 LocalProcessTerminalViewDelegate {
    nonisolated(unsafe) var onTerminated: (() -> Void)?
    nonisolated override init() { super.init() }
    nonisolated func sizeChanged(source: LocalProcessTerminalView,
                                 newCols: Int, newRows: Int) {}
    nonisolated func setTerminalTitle(source: LocalProcessTerminalView,
                                      title: String) {}
    nonisolated func hostCurrentDirectoryUpdate(source: TerminalView,
                                                directory: String?) {}
    nonisolated func processFailedToStart(source: TerminalView,
                                          error: LocalProcessError) {}
    nonisolated func processTerminated(source: TerminalView,
                                       exitCode: Int32?) { onTerminated?() }
}

final class VimImageOverlay: NSView {
    struct Item: Equatable { let path: String; let row: Int; let rows: Int }
    var cell = NSSize(width: 8, height: 16) { didSet { if cell != oldValue { relayout() } } }
    var textRows = 0 { didSet { if textRows != oldValue { relayout() } } }
    var items: [Item] = [] { didSet { if items != oldValue { relayout() } } }
    private var views: [NSView] = []
    private var cache: [String: (mtime: Date, image: NSImage)] = [:]

    var onOpen: ((String) -> Void)?
    var onMenu: ((String, NSEvent) -> Void)?
    private var hits: [(rect: NSRect, path: String)] = []

    override var isFlipped: Bool { true }
    override func hitTest(_ point: NSPoint) -> NSView? {
        let p = convert(point, from: superview)
        return hits.contains { $0.rect.contains(p) } ? self : nil
    }
    private func path(at event: NSEvent) -> String? {
        let p = convert(event.locationInWindow, from: nil)
        return hits.first { $0.rect.contains(p) }?.path
    }
    override func mouseDown(with event: NSEvent) {
        if let p = path(at: event) { onOpen?(p) }
    }
    override func rightMouseDown(with event: NSEvent) {
        if let p = path(at: event) { onMenu?(p, event) }
    }
    override var acceptsFirstResponder: Bool { false }

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.masksToBounds = true
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) not supported") }

    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        relayout()
    }

    private func image(_ path: String) -> NSImage? {
        let mt = (try? FileManager.default.attributesOfItem(atPath: path)[.modificationDate]) as? Date
            ?? .distantPast
        if let c = cache[path], c.mtime == mt { return c.image }
        guard let img = NSImage(contentsOfFile: path) else { return nil }
        cache[path] = (mt, img)
        return img
    }

    private func relayout() {
        views.forEach { $0.removeFromSuperview() }
        views = []
        hits = []
        let limit = CGFloat(textRows) * cell.height
        for it in items {
            guard let img = image(it.path), img.size.width > 0, img.size.height > 0 else { continue }
            let top = CGFloat(it.row) * cell.height + 2
            guard top < limit else { continue }
            let boxH = CGFloat(it.rows) * cell.height - 4
            let boxW = max(40, bounds.width - 16)
            let scale = min(1, boxH / img.size.height, boxW / img.size.width)
            let size = NSSize(width: floor(img.size.width * scale), height: floor(img.size.height * scale))
            let iv = NSImageView(frame: NSRect(x: 2, y: top, width: size.width, height: size.height))
            iv.image = img
            iv.imageScaling = .scaleProportionallyUpOrDown
            iv.wantsLayer = true
            iv.layer?.cornerRadius = 6
            iv.layer?.masksToBounds = true
            if iv.frame.maxY > limit {
                let visible = limit - top
                guard visible > 4 else { continue }
                let clip = NSView(frame: NSRect(x: 2, y: top, width: size.width, height: visible))
                clip.wantsLayer = true
                clip.layer?.masksToBounds = true
                iv.frame.origin = .zero
                clip.addSubview(iv)
                addSubview(clip)
                views.append(clip)
                hits.append((clip.frame, it.path))
                continue
            }
            addSubview(iv)
            views.append(iv)
            hits.append((iv.frame, it.path))
        }
    }
}

func focusLeft(_ window: NSWindow) -> Bool {
    if window.isKeyWindow || window.attachedSheet != nil { return false }
    if let k = NSApp.keyWindow, k !== window, k.isVisible, !(k.delegate is PopupWindow) { return false }
    return true
}

public final class PopupWindow: NSObject, NSTextFieldDelegate, NSWindowDelegate {
    public var config: PopupConfig

    public var onShow: (() -> Void)?
    public var onFilter: ((String) -> [PopupRow])?
    public var onAccept: ((PopupRow) -> Void)?
    public var onRowClick: ((Int) -> Void)?
    public var onRowDoubleClick: ((Int) -> Void)?
    public var onEscape: (() -> Void)?
    public var onHide: ((Bool) -> Void)?
    public var onHideVoiceStop: (() -> Void)?
    public var onVimExit: (() -> Void)?
    public var vimLaunchArgs: (() -> [String])?
    public var onFontSizeStep: ((Int) -> Void)?
    public var onCloseWindow: (() -> Void)?
    public var editorText: String = ""
    public var onEditorCommit: ((String) -> Void)?
    public var onEditorClose: ((String) -> Void)?
    public var onEditorTextChange: (() -> Void)? {
        didSet { wireEditorTextChange() }
    }
    public func setStatus(_ text: String?, isError: Bool) {
        guard let bar = statusBar else { return }
        let visible = !(text?.isEmpty ?? true)
        bar.text = text ?? ""
        bar.isError = isError
        bar.isHidden = !visible
        layoutEditorScroll()
    }
    public var onDrawRow: ((NSRect, PopupRow, Bool) -> Void)? {
        didSet { rowView.onDrawRow = onDrawRow }
    }

    public private(set) var rows: [PopupRow] = []
    public var selection = 0 {
        didSet {
            rowView.selection = selection
            rowView.needsDisplay = true
            scrollSelectionIntoView()
            onSelectionChanged?(selection)
        }
    }
    public var onSelectionChanged: ((Int) -> Void)?
    public var onInspectorOpen: (() -> Void)?
    public var inspectorContent: PopupInspectorContent? { didSet { inspectorView?.content = inspectorContent } }
    private var inspectorView: PopupInspectorView?
    private var inspectorKey: String { "inspectorShown." + config.name }
    public var inspectorShown: Bool {
        guard config.inspectorWidth > 0, !config.editMode else { return false }
        return (UserDefaults.standard.object(forKey: inspectorKey) as? Bool) ?? true
    }
    private var listRight: CGFloat { inspectorShown ? config.inspectorWidth * zoom : 0 }
    public func toggleInspector() {
        guard config.inspectorWidth > 0, !config.editMode else { return }
        UserDefaults.standard.set(!inspectorShown, forKey: inspectorKey)
        layoutForZoom()
        layoutSearchField()
        layoutScrollDocument()
        onSelectionChanged?(selection)
    }
    private func layoutInspector() {
        guard let backdrop = panel.contentView else { return }
        guard inspectorShown else { inspectorView?.isHidden = true; return }
        let v = inspectorView ?? {
            let v = PopupInspectorView(colors: config.colors, zoom: { [weak self] in self?.zoom ?? 1 })
            v.onOpen = { [weak self] in self?.onInspectorOpen?() }
            v.content = inspectorContent
            backdrop.addSubview(v)
            inspectorView = v
            DispatchQueue.main.async { [weak self] in
                guard let self else { return }
                self.onSelectionChanged?(self.selection)
            }
            return v
        }()
        v.isHidden = listOverlay != nil
        let top = (config.dragHeader ? config.headerHeight * zoom : 0) + topAccessoryHeight
        let w = min(config.inspectorWidth * zoom, backdrop.bounds.width * 0.6)
        let f = NSRect(x: backdrop.bounds.width - w, y: top, width: w, height: max(0, backdrop.bounds.height - top))
        if v.frame != f { v.frame = f }
        if let chrome, v.superview === backdrop, backdrop.subviews.last !== chrome {
            backdrop.addSubview(v, positioned: .below, relativeTo: chrome)
        }
    }

    public var selectedIndices: Set<Int> {
        get { rowView.selected }
        set {
            rowView.selected = newValue.filter { rows.indices.contains($0) }
            rowView.needsDisplay = true
            updateCopyRowsLabel()
        }
    }

    public var onCopyRows: (([PopupRow]) -> String)?

    public var onCommandK: (() -> Void)?
    public var onCycleView: ((Int) -> Void)?
    public var onCommandF: (() -> Void)?

    public var actionRows: [PopupRow] {
        let idx = [selection] + rowView.selected.sorted().filter { $0 != selection }
        return idx.filter { rows.indices.contains($0) }.map { rows[$0] }
    }

    public var headerButtons: [(String, Int)] = [] {
        didSet {
            chrome?.extraButtons = headerButtons
            chrome?.needsDisplay = true
        }
    }
    public var headerOrder: [Int]? {
        didSet {
            chrome?.headerOrder = headerOrder
            chrome?.needsDisplay = true
        }
    }
    public var onHeaderButton: ((Int) -> Void)?
    public static var transientEscape: (() -> Void)?
    private var topAccessory: NSView?
    private var topAccessoryHeight: CGFloat = 0
    public var onAccessoryEscape: (() -> Void)?
    public func setTopAccessory(_ v: NSView?, height: CGFloat = 0) {
        let hadFocus = topAccessoryHasFocus
        if let old = topAccessory, old !== v { old.removeFromSuperview() }
        topAccessory = v
        topAccessoryHeight = v == nil ? 0 : ceil(height)
        if let v, let backdrop = panel.contentView {
            if v.superview !== backdrop {
                if let chrome { backdrop.addSubview(v, positioned: .below, relativeTo: chrome) }
                else { backdrop.addSubview(v) }
            }
            v.autoresizingMask = [.width]
            v.frame = NSRect(x: 0, y: config.dragHeader ? config.headerHeight * zoom : 0,
                             width: backdrop.bounds.width, height: topAccessoryHeight)
        }
        layoutForZoom()
        layoutSearchField()
        if v == nil, hadFocus || panel.firstResponder === panel {
            panel.makeFirstResponder(primaryEditor ?? field)
        }
    }
    public var hasTopAccessory: Bool { topAccessory != nil }

    private var listBar: NSView?
    private var listBarHeight: CGFloat = 0
    public private(set) var listOverlay: NSView?
    public func setListBar(_ v: NSView?, height: CGFloat = 0) {
        if let old = listBar, old !== v { old.removeFromSuperview() }
        listBar = v
        listBarHeight = v == nil ? 0 : ceil(height)
        if let v, let backdrop = panel.contentView, v.superview !== backdrop {
            if let chrome { backdrop.addSubview(v, positioned: .below, relativeTo: chrome) } else { backdrop.addSubview(v) }
        }
        layoutForZoom()
        layoutSearchField()
        layoutScrollDocument()
    }
    public func setListOverlay(_ v: NSView?) {
        if let old = listOverlay, old !== v { old.removeFromSuperview() }
        listOverlay = v
        if textZoomKey != nil { (v as? PageZoomable)?.pageZoom = textZoom }
        if let v, let backdrop = panel.contentView, v.superview !== backdrop {
            if let chrome { backdrop.addSubview(v, positioned: .below, relativeTo: chrome) } else { backdrop.addSubview(v) }
        }
        rowScroll?.isHidden = v != nil
        tableHeader?.isHidden = v != nil
        inspectorView?.isHidden = v != nil || !inspectorShown
        layoutListExtras()
    }
    private func layoutListExtras() {
        guard !config.editMode, let backdrop = panel.contentView else { return }
        let top = (config.dragHeader ? config.headerHeight * zoom : 0) + topAccessoryHeight
        let right = listOverlay == nil ? listRight : 0
        listBar?.frame = NSRect(x: listLeft, y: top, width: backdrop.bounds.width - listLeft - right, height: listBarHeight)
        if let ov = listOverlay {
            let y = chromeBottom
            ov.frame = NSRect(x: listLeft, y: y, width: backdrop.bounds.width - listLeft,
                              height: max(0, backdrop.bounds.height - y))
        }
    }
    public var onRowsChanged: (() -> Void)?
    public var onTestAction: ((String) -> Void)?
    public var testExtra: (() -> [String: Any])?
    public var topAccessoryHasFocus: Bool {
        guard let acc = topAccessory, let fr = panel.firstResponder else { return false }
        if let v = fr as? NSView, v.isDescendant(of: acc) { return true }
        if let t = fr as? NSText, let d = t.delegate as? NSView, d.isDescendant(of: acc) { return true }
        return false
    }

    public var navIcons: [(image: NSImage, id: Int, tip: String)] = [] {
        didSet { chrome?.navIcons = navIcons }
    }
    public var navOn: Int? {
        didSet { chrome?.navOn = navOn }
    }

    private var headerButtonOn: Set<Int> = []
    public func setHeaderButtonOn(_ id: Int, _ on: Bool) {
        if on { headerButtonOn.insert(id) } else { headerButtonOn.remove(id) }
        chrome?.activeButtonIDs = headerButtonOn
        chrome?.needsDisplay = true
    }

    public var editorReadOnly = false

    public func toggleRowSelection(_ index: Int) {
        guard rows.indices.contains(index) else { return }
        var s = rowView.selected
        if s.contains(index) {
            s.remove(index)
        } else {
            s.insert(index)
        }
        rowView.selected = s
        rowView.needsDisplay = true
        updateCopyRowsLabel()
    }

    private func updateCopyRowsLabel() {
        guard config.selectableRows, config.copyRowsButton else {
            chrome?.copyRowsLabel = nil
            return
        }
        let n = rowView.selected.count
        chrome?.copyRowsLabel = n == 0 ? "copy selected" : "copy \(n)"
    }

    func performCopyRows() {
        guard let onCopyRows else { return }
        let idx = rowView.selected.isEmpty
            ? Array(rows.indices)
            : rowView.selected.sorted()
        let picked = idx.filter { rows.indices.contains($0) }.map { rows[$0] }
        guard !picked.isEmpty else { return }
        let pb = NSPasteboard.general
        pb.clearContents()
        pb.setString(onCopyRows(picked), forType: .string)
    }

    public private(set) var isShown = false
    public private(set) var isShowingMenu = false
    public var hostHandlesFocusLoss = false
    private var focusLossGen = 0

    public var nativeWindow: NSWindow { panel }

    private var panel: NSWindow
    private let homeWindow: NSWindow
    private let field: NSTextField
    private let rowView: PopupRowView
    private var editorView: NSTextView?
    private var tintView: NSView?
    private var terminalDrawer: LocalProcessTerminalView?
    private let terminalInset: CGFloat = 4
    private var findField: NSTextField?
    private var findCountLabel: NSTextField?
    private var findMatches: [NSRange] = []
    private var findIndex = 0
    private var terminalRestartTimer: Timer?
    public private(set) var terminalShown = true
    private(set) var fileBrowser: PopupFileBrowser?
    public private(set) var fileBrowserShown = false
    private var fileBrowserDrawerMode = false

    enum FocusedPane { case editor, browser, terminal }
    private var focusedPane: FocusedPane?
    private var editorFocusBorder: NSView?
    private var browserFocusBorder: NSView?
    private var terminalFocusBorder: NSView?
    private let focusBorderWidth: CGFloat = 3

    private var drawerInsetNow: CGFloat = 0
    private var currentTerminalHeight: CGFloat = 0
    private var currentBrowserHeight: CGFloat = 0
    private lazy var preferredTerminalHeight: CGFloat = config.terminalHeight
    private lazy var preferredBrowserHeight: CGFloat = config.fileBrowserHeight
    private let minEditorH: CGFloat = 80
    private let minTerminalH: CGFloat = 40
    private let minBrowserH: CGFloat = 50
    private var editorScroll: NSScrollView?
    private var tabsBar: PopupTabsBar?
    private var filterBar: PopupFilterBar?
    private var windowCloseTarget: ClosureTarget?
    private var terminalRestarter: TerminalAutoRestart?
    private var vimView: LocalProcessTerminalView?
    private var proseView: ProseView?
    private var pageOverlay: NSView?
    public func setPageOverlay(_ v: NSView?) {
        if pageOverlay !== v { pageOverlay?.removeFromSuperview() }
        pageOverlay = v
        if textZoomKey != nil { (v as? PageZoomable)?.pageZoom = textZoom }
        guard let v, let backdrop = panel.contentView else { return }
        if v.superview == nil {
            if let chrome { backdrop.addSubview(v, positioned: .below, relativeTo: chrome) } else { backdrop.addSubview(v) }
        }
        layoutEditorScroll()
    }
    private var proseSwitch: ProseModeSwitch?
    public private(set) var proseShown = false
    public var proseFont = "Literata, ui-serif, \"New York\", Georgia, serif"
    public var proseFontSize: CGFloat = 19
    public var proseWidth: CGFloat = 900
    public var proseProvider: (() -> ProseSource?)? {
        didSet { installProseSwitch() }
    }
    private var vimRestarter: TerminalAutoRestart?
    private var vimShuttingDown = false
    private var vimPaneActive = true
    private var vimImageOverlay: VimImageOverlay?
    private var vimImageWatch: DispatchSourceFileSystemObject?
    private var escStreak = 0
    private var lastEsc = Date.distantPast
    private var rowScroll: NSScrollView?
    private var tableHeader: PopupTableHeaderView?
    public var tableSort: (column: Int, ascending: Bool)? {
        didSet {
            tableHeader?.sortColumn = tableSort?.column
            tableHeader?.sortAscending = tableSort?.ascending ?? true
        }
    }
    public var onTableSort: ((Int) -> Void)?
    public var onTableFit: (() -> Void)? {
        didSet {
            tableHeader?.onFit = onTableFit == nil ? nil : { [weak self] in self?.onTableFit?() }
            if let h = tableHeader { h.window?.invalidateCursorRects(for: h) }
        }
    }
    public var onTableHeaderMenu: (() -> [NSMenuItem])? {
        didSet { tableHeader?.extraMenu = { [weak self] in self?.onTableHeaderMenu?() ?? [] } }
    }
    public var onTableFilter: ((Int, NSView, NSRect) -> Void)?
    public var tableFilterActive: Set<Int> = [] {
        didSet { tableHeader?.activeFilters = tableFilterActive }
    }
    public var onToggleStar: ((Int) -> Void)?
    public var onTableColumnsReordered: ((Int, Int) -> Void)?
    public var onTableColumnsResized: (([CGFloat], Bool) -> Void)?
    private var chrome: PopupChrome?
    private var statusBar: PopupStatusBar?
    private let statusBarHeight: CGFloat = 26
    private var chromeBottom: CGFloat = 0
    private var monitors: [Any] = []
    private var focusRetries = 0

    public var chromeHeaderTitle: String? {
        didSet { chrome?.headerTitle = chromeHeaderTitle }
    }
    public var headerIcon: NSImage? {
        didSet { chrome?.headerIcon = headerIcon }
    }
    public var copyPathButtonLabel: String? {
        didSet {
            guard let v = copyPathButtonLabel else { return }
            chrome?.copyPathLabel = v
            chrome?.needsDisplay = true
        }
    }
    public var copyConfigButtonLabel: String? {
        didSet {
            guard let v = copyConfigButtonLabel else { return }
            chrome?.copyConfigLabel = v
            chrome?.needsDisplay = true
        }
    }

    public var itemCount: String? {
        didSet {
            chrome?.itemCount = itemCount
            chrome?.needsDisplay = true
        }
    }

    public var meterEnabled = false {
        didSet {
            guard oldValue != meterEnabled else { return }
            chrome?.meterEnabled = meterEnabled
            chrome?.needsDisplay = true
            if panel.frame.height > 0, let barH = chrome?.meterBarHeight {
                var f = panel.frame
                if meterEnabled {
                    f.origin.y -= barH
                    f.size.height += barH
                } else {
                    f.origin.y += barH
                    f.size.height -= barH
                }
                if f.size.height >= 120 {
                    panel.setFrame(clampToScreen(f), display: true)
                }
            }
            layoutEditorScroll()
            layoutTerminal()
            layoutFileBrowser()
        }
    }
    public var recordingState: Int = 0 {
        didSet {
            chrome?.meterState = recordingState
            chrome?.needsDisplay = true
        }
    }
    public var recordingLevel: Float = 0 {
        didSet {
            chrome?.meterLevel = recordingLevel
            chrome?.needsDisplay = true
        }
    }
    public var recordingElapsed: TimeInterval = 0 {
        didSet {
            chrome?.meterElapsed = recordingElapsed
            chrome?.needsDisplay = true
        }
    }
    public var onMeterRecord: (() -> Void)?
    public var onMeterPause: (() -> Void)?

    public var onChromeHeaderClick: (() -> Void)?
    public var onChromeConfigClick: (() -> Void)?
    public var onChromeIconClick: (() -> Void)?

    public var tabTitles: [String] = [] {
        didSet {
            tabsBar?.titles = tabTitles
            tabsBar?.needsDisplay = true
            relayoutTabs()
        }
    }
    public var tabRowIcon: ((Int) -> String?)?
    public var tabRowTitle: ((Int) -> String?)?
    public var quietOKTabBadges: Bool {
        get { tabsBar?.quietOKBadges ?? false }
        set { tabsBar?.quietOKBadges = newValue; tabsBar?.needsDisplay = true }
    }
    public func setSidebarStatus(_ text: String?, tone: PopupTone = .success) {
        tabsBar?.statusLine = text.map { ($0, tone) }
    }
    public func setSidebarPinned(_ ids: [String], title: String, icon: String,
                                 selected: String? = nil,
                                 label: @escaping (String) -> String,
                                 tip: @escaping (String) -> String,
                                 menu: ((String) -> NSMenu?)? = nil,
                                 section: ((String) -> String)? = nil,
                                 iconFor: ((String) -> String)? = nil,
                                 meta: ((String) -> String?)? = nil,
                                 maxShown: Int = 8,
                                 onClick: @escaping (String) -> Void) {
        guard let bar = tabsBar, bar.vertical else { return }
        bar.pinnedTitle = title
        bar.pinnedSection = section
        bar.pinnedIconFor = iconFor
        bar.pinnedMeta = meta
        bar.pinnedIcon = icon
        bar.pinnedLabel = label
        bar.pinnedTip = tip
        bar.pinnedMenu = menu
        bar.onPinned = onClick
        bar.maxPinnedShown = maxShown
        bar.pinned = ids
        bar.pinnedSelected = selected
    }
    public func selectSidebarPin(_ id: String) {
        let cb = onTabChange
        onTabChange = nil
        selectedTab = -1
        onTabChange = cb
        tabsBar?.pinnedSelected = id
    }
    public func clearSidebarPin() { tabsBar?.pinnedSelected = nil }
    public struct SidebarJumpItem { public let section: String, title: String, icon: String?, row: Int }
    public var onSidebarJump: (() -> Void)?
    public func sidebarJumpItems() -> [SidebarJumpItem] {
        guard let bar = tabsBar, bar.vertical else { return [] }
        return bar.jumpItems()
    }
    public func sidebarJump(_ row: Int) { tabsBar?.activate(row: row) }
    public var tabPathTip: ((Int) -> String?)? {
        didSet { tabsBar?.pathTip = tabPathTip }
    }
    public var tabBadges: [PopupTabBadge?] = [] {
        didSet {
            tabsBar?.badges = tabBadges
            relayoutTabs()
        }
    }
    public var selectedTab = 0 {
        didSet {
            guard oldValue != selectedTab else { return }
            tabsBar?.selected = selectedTab
            tabsBar?.needsDisplay = true
            onTabChange?(selectedTab)
            if proseShown { DispatchQueue.main.asyncAfter(deadline: .now() + 0.2) { [weak self] in self?.refreshProse() } }
        }
    }
    public var tabFooterText: String? {
        didSet {
            chrome?.footerText = tabFooterText
            chrome?.needsDisplay = true
        }
    }
    public var onTabChange: ((Int) -> Void)?
    public var onTabClick: ((Int) -> Void)?
    public var onAddTab: (() -> Void)?
    public var onCloseTab: ((Int) -> Void)?
    public var onTabCopyPath: ((Int) -> Void)?
    public var onOpenExternalPath: ((String) -> Void)?
    public var openNotePaths: (() -> [String])?
    public var onOpenPathPrompt: (() -> Void)?
    public var onNewNote: ((_ template: Bool) -> Void)?
    public var onCopyFilePath: (() -> Void)?
    public var onTerminalOpenInNotes: ((String) -> Void)?
    public var onTerminalOpenDefault: ((String) -> Void)?
    public var onTerminalRevealInFinder: ((String) -> Void)?
    public var onFileBrowserOpenInNotes: ((String) -> Void)?

    public var filterLabels: [String] = [] {
        didSet { filterBar?.labels = filterLabels; filterBar?.needsDisplay = true }
    }
    public var filterValues: [[String]] = [] {
        didSet { filterBar?.values = filterValues; filterBar?.needsDisplay = true }
    }
    public var filterValueLabels: [[String]] = [] {
        didSet { filterBar?.valueLabels = filterValueLabels; filterBar?.needsDisplay = true }
    }
    public var onFilterOpen: ((Int, NSView, NSRect) -> Void)? {
        didSet {
            filterBar?.onOpen = onFilterOpen.map { cb in
                { [weak self] dim, r in
                    guard let bar = self?.filterBar else { return }
                    cb(dim, bar, r)
                }
            }
        }
    }
    public var filterSummaries: [String] = [] {
        didSet { filterBar?.summaries = filterSummaries; filterBar?.needsDisplay = true; growWidthToContent() }
    }
    public var filterActive: Set<Int> = [] {
        didSet { filterBar?.active = filterActive; filterBar?.needsDisplay = true }
    }
    public var filterSelections: [Int] = [] {
        didSet {
            guard oldValue != filterSelections else { return }
            filterBar?.selections = filterSelections
            filterBar?.needsDisplay = true
            growWidthToContent()
            onFilterChange?(filterSelections)
        }
    }
    public var onFilterChange: (([Int]) -> Void)?

    public var currentQuery: String { field.stringValue }

    public var zoom: CGFloat = 1.0 {
        didSet {
            guard oldValue != zoom else { return }
            config.zoom = zoom
            rowView.zoom = zoom * textZoom
            chrome?.zoom = zoom
            tabsBar?.zoom = zoom
            filterBar?.zoom = zoom
            field.font = config.rowFont(config.inputFontSize * zoom)
            if let tv = editorView {
                tv.font = editorFont(config.fontName, zoom, size: config.editorFontSize)
            }
            applyVimFont()
            refreshVimImageRows()
            if let chrome {
                chrome.dragHeaderHeight = config.headerHeight * zoom
                chrome.needsDisplay = true
            }
            if let base = panel as? HeaderClickWindow, base.headerClickBand > 0 {
                base.headerClickBand = config.headerHeight * zoom
            }
            rowView.needsDisplay = true
            tabsBar?.needsDisplay = true
            filterBar?.needsDisplay = true
            field.needsDisplay = true
            layoutForZoom()
        }
    }

    public var textZoomKey: String? {
        didSet {
            guard let k = textZoomKey, k != oldValue else { return }
            let z = CGFloat(UserDefaults.standard.double(forKey: k))
            setTextZoom(z > 0 ? z : 1, save: false)
        }
    }
    public private(set) var textZoom: CGFloat = 1
    public func setTextZoom(_ z: CGFloat, save: Bool = true) {
        let z = min(3, max(0.6, (z * 100).rounded() / 100))
        if z != textZoom {
            textZoom = z
            rowView.zoom = zoom * z
            if !fileBrowserDrawerMode { fileBrowser?.textZoom = z }
            layoutScrollDocument()
            rowView.needsDisplay = true
        }
        for case let v as PageZoomable in [listOverlay, pageOverlay] { v.pageZoom = z }
        if save, let k = textZoomKey { UserDefaults.standard.set(Double(z), forKey: k) }
    }

    private func layoutForZoom() {
        let z = zoom
        guard let backdrop = panel.contentView else { return }
        if config.editMode {
            if let bar = tabsBar, !sidebarTabs {
                bar.frame = NSRect(x: 0, y: config.headerHeight * z + 2,
                                   width: backdrop.bounds.width,
                                   height: config.tabBarHeight * z)
            }
            layoutEditorScroll()
            layoutTerminal()
            layoutFileBrowser()
        } else {
            let headerOffset = ((config.dragHeader) ? config.headerHeight * z + 4 : 0) + topAccessoryHeight + listBarHeight
            let fieldFrame = NSRect(x: listLeft + config.padding + 10,
                                    y: headerOffset + config.padding + 2,
                                    width: config.width - listLeft - listRight - 2 * (config.padding + 10),
                                    height: 24 * z)
            field.frame = fieldFrame
            var cb = fieldFrame.maxY + 4
            if let bar = filterBar {
                bar.frame = NSRect(x: listLeft, y: cb, width: backdrop.bounds.width - listLeft - listRight,
                                   height: config.filterBarHeight * z)
                cb += config.filterBarHeight * z + 2
            }
            if let bar = tabsBar, !sidebarTabs {
                bar.frame = NSRect(x: 0, y: cb, width: backdrop.bounds.width,
                                   height: config.tabBarHeight * z)
                cb += config.tabBarHeight * z + 2
            }
            if config.scrollableRows {
                chromeBottom = cb
                layoutScrollDocument()
            } else {
                rowView.topInset = cb
            }
        }
        relayoutTabs()
    }

    public init(config: PopupConfig) {
        self.config = config
        let height = config.padding * 2 + config.headerHeight * zoom + config.rowHeight * zoom
        panel = Self.makePanel(config, height: height)
        homeWindow = panel
        rowView = PopupRowView(config: config)
        field = Self.makeSearchField(config, zoom: zoom)
        super.init()

        let backdrop = makeBackdrop(height: height)
        if config.editMode { buildEditor(in: backdrop) } else { buildList(in: backdrop) }
        if config.editMode || config.enableDrag { buildChrome(in: backdrop) }
        panel.contentView = backdrop
        applyThemeAppearance()

        wireRows()
        if config.selectableRows { updateCopyRowsLabel() }
        wireCloseButton()
        panel.delegate = self
        (panel as? EscapableWindow)?.onEscape = { [weak self] in
            self?.handleEscape()
        }
        if !config.editMode {
            field.delegate = self
        }
        wireHeaderClicks()
        panel.orderOut(nil)
    }

    deinit { terminalRestartTimer?.invalidate() }

    private static func makePanel(_ config: PopupConfig, height: CGFloat) -> NSWindow {
        let panel: NSWindow
        let wantsTitlebar = (config.editMode || config.enableDrag) && !config.toolPanel
        if wantsTitlebar {
            var mask: NSWindow.StyleMask = [.titled, .closable, .fullSizeContentView]
            if config.enableResize { mask.insert(.resizable) }
            panel = PopupPlainWindow(
                contentRect: NSRect(x: 0, y: 0, width: config.width, height: height),
                styleMask: mask,
                backing: .buffered, defer: false)
            panel.contentMinSize = NSSize(width: 320, height: 220)
            (panel as? PopupPlainWindow)?.cornerRadius = config.cornerRadius
            panel.titlebarAppearsTransparent = true
            panel.titleVisibility = .hidden
            for type: NSWindow.ButtonType in [.miniaturizeButton, .zoomButton] {
                panel.standardWindowButton(type)?.isHidden = true
            }
            if !config.showCloseButton {
                panel.standardWindowButton(.closeButton)?.isHidden = true
            }
        } else {
            panel = PopupPanel(
                contentRect: NSRect(x: 0, y: 0, width: config.width, height: height),
                styleMask: [.borderless, .nonactivatingPanel],
                backing: .buffered, defer: false)
            if config.toolPanel { panel.hidesOnDeactivate = false }
        }
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = config.hasShadow
        panel.level = config.floating ? .floating : .normal
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        panel.isReleasedWhenClosed = false
        panel.animationBehavior = .none
        panel.title = config.name
        panel.acceptsMouseMovedEvents = true
        return panel
    }

    private static func makeSearchField(_ config: PopupConfig, zoom: CGFloat) -> NSTextField {
        let field = NSTextField(frame: NSRect(x: config.padding + 10, y: config.padding + 2,
                                              width: config.width - 2 * (config.padding + 10),
                                              height: 22))
        field.isBezeled = false
        field.drawsBackground = false
        field.isEditable = config.enableSearch
        field.isSelectable = config.enableSearch
        field.font = config.rowFont(config.inputFontSize * zoom)
        field.textColor = config.colors.text
        field.alignment = .left
        field.focusRingType = .none
        if config.showSearchBar {
            let fieldCell = PopupSearchFieldCell()
            fieldCell.hInset = 8
            field.cell = fieldCell
            field.isEditable = config.enableSearch
            field.isSelectable = config.enableSearch
            field.focusRingType = .none
            field.wantsLayer = true
            field.layer?.backgroundColor = ButtonStyle.inputFill(config.colors).cgColor
            field.layer?.cornerRadius = 6
            field.layer?.borderWidth = 1
            field.layer?.borderColor = ButtonStyle.inputStroke(config.colors).cgColor
            field.placeholderAttributedString = NSAttributedString(
                string: config.searchPlaceholder,
                attributes: [
                    .font: config.rowFont(config.inputFontSize * zoom),
                    .foregroundColor: config.colors.dim,
                ])
        }
        return field
    }

    private func makeBackdrop(height: CGFloat) -> PopupBackdrop {
        let backdrop = PopupBackdrop(config: config,
                                     frame: NSRect(x: 0, y: 0, width: config.width, height: height))
        backdrop.autoresizingMask = [.width, .height]
        backdrop.wantsLayer = true
        backdrop.layer?.cornerRadius = config.cornerRadius
        backdrop.layer?.masksToBounds = true
        backdrop.layer?.borderWidth = 1
        backdrop.layer?.borderColor = config.colors.border.cgColor

        if config.tintAlpha > 0 {
            let fx = NSVisualEffectView(frame: backdrop.bounds)
            fx.material = config.material
            fx.blendingMode = .behindWindow
            fx.state = .active
            fx.autoresizingMask = [.width, .height]
            backdrop.addSubview(fx)
        }

        let tint = NSView(frame: backdrop.bounds)
        tint.wantsLayer = true
        tint.layer?.backgroundColor =
            config.colors.background.withAlphaComponent(config.tintAlpha).cgColor
        tint.autoresizingMask = [.width, .height]
        backdrop.addSubview(tint)
        tintView = tint

        rowView.frame = backdrop.bounds
        rowView.autoresizingMask = [.width, .height]
        return backdrop
    }

    private func makeTabsBar(frame: NSRect) -> PopupTabsBar {
        let bar = PopupTabsBar(config: config)
        bar.frame = frame
        bar.autoresizingMask = [.width]
        bar.onSelect = { [weak self] index in
            self?.selectedTab = index
        }
        bar.onClick = { [weak self] index in
            self?.onTabClick?(index)
        }
        bar.onAddTab = { [weak self] in
            self?.onAddTab?()
        }
        bar.onCloseTab = { [weak self] index in
            self?.onCloseTab?(index)
        }
        bar.onCopyPath = { [weak self] index in
            self?.onTabCopyPath?(index)
            self?.toastPasteboardPath()
        }
        bar.pathTip = { [weak self] i in self?.tabPathTip?(i) }
        bar.onWidthChange = { [weak self] w, done in self?.setSidebarWidth(w, done: done) }
        bar.rowIcon = { [weak self] i in self?.tabRowIcon?(i) }
        bar.rowTitleFor = { [weak self] i in self?.tabRowTitle?(i) }
        bar.sectionTitle = config.tabsSidebarTitle
        bar.collapseKey = config.name
        bar.onCollapse = { [weak self] _ in self?.sidebarRailChanged() }
        return bar
    }

    private func makeFocusBorder(around frame: NSRect, radius: CGFloat,
                                 resize: NSView.AutoresizingMask) -> NSView {
        let b = NSView(frame: frame)
        b.autoresizingMask = resize
        b.wantsLayer = true
        b.layer?.borderWidth = focusBorderWidth
        b.layer?.borderColor = ButtonStyle.focusStroke(config.colors).cgColor
        b.layer?.cornerRadius = radius
        b.isHidden = true
        return b
    }

    private func buildEditor(in backdrop: NSView) {
        let topY = config.headerHeight * zoom + editorTabStripHeight
        let scroll = NSScrollView(frame: NSRect(x: 0, y: topY,
                                                width: backdrop.bounds.width,
                                                height: backdrop.bounds.height - topY))
        scroll.autoresizingMask = [.width]
        scroll.hasVerticalScroller = true
        scroll.hasHorizontalScroller = false
        scroll.autohidesScrollers = true
        scroll.drawsBackground = false
        scroll.borderType = .noBorder
        let tv = PopupTextView(frame: scroll.bounds)
        tv.isRichText = false
        tv.isEditable = true
        tv.isSelectable = true
        tv.allowsUndo = true
        tv.font = editorFont(config.fontName, zoom, size: config.editorFontSize)
        tv.textColor = config.colors.text
        tv.selectedTextAttributes = ButtonStyle.selection(config.colors)
        tv.backgroundColor = .clear
        tv.drawsBackground = false
        tv.textContainerInset = NSSize(width: 10, height: 10)
        tv.autoresizingMask = [.width]
        if config.markdownImages {
            tv.isHorizontallyResizable = true
            tv.maxSize = NSSize(width: 4096,
                                height: CGFloat.greatestFiniteMagnitude)
            tv.textContainer?.widthTracksTextView = false
            tv.textContainer?.containerSize = NSSize(
                width: max(120, scroll.bounds.width - 24),
                height: CGFloat.greatestFiniteMagnitude)
        }
        tv.onOpenFileAtPath = { [weak self] in
            self?.onOpenPathPrompt?()
        }
        tv.onCopyFilePath = { [weak self] in
            self?.onCopyFilePath?()
            self?.toastPasteboardPath()
        }
        tv.onCopiedImagePath = { [weak self] shown in self?.toastCopiedPath(shown) }
        tv.onOpenImage = { [weak self] path in
            FilePopup.show(path: path, over: self?.panel)
        }
        tv.onPasteImage = { [weak self] img in
            guard let self, self.config.markdownImages,
                  let rel = self.imageSaver?(img) else { return }
            self.insertImageAttachment(rel: rel, image: img)
        }
        tv.absolutePathAt = { [weak self] idx in
            guard let self, let tv = self.editorView,
                  let storage = tv.textStorage, idx < storage.length else { return nil }
            let attrs = storage.attributes(at: idx, effectiveRange: nil)
            guard let att = attrs[.attachment] as? NSTextAttachment,
                  let rel = self.attachmentPaths[att] else { return nil }
            return (self.imageBaseDir as NSString).appendingPathComponent(rel)
        }
        scroll.documentView = tv
        backdrop.addSubview(scroll)
        editorView = tv
        editorScroll = scroll
        let efb = makeFocusBorder(around: scroll.frame, radius: 6, resize: [.width, .height])
        backdrop.addSubview(efb)
        editorFocusBorder = efb
        if config.vimEditorExecutable != nil { buildVimPane(over: scroll, below: efb, in: backdrop) }
        if config.tabs {
            let bar = makeTabsBar(frame: NSRect(x: 0, y: config.headerHeight * zoom + 2,
                                                width: backdrop.bounds.width, height: config.tabBarHeight * zoom))
            if config.tabsSidebarWidth > 0 {
                bar.vertical = true
                bar.autoresizingMask = []
                bar.frame = NSRect(x: 0, y: config.headerHeight * zoom,
                                   width: config.tabsSidebarWidth * zoom,
                                   height: max(0, backdrop.bounds.height - config.headerHeight * zoom))
            }
            backdrop.addSubview(bar)
            tabsBar = bar
        }
        buildFindBar(in: backdrop)
        let sb = PopupStatusBar(config: config)
        sb.frame = NSRect(x: 6, y: backdrop.bounds.height - statusBarHeight - 4,
                          width: max(0, backdrop.bounds.width - 12),
                          height: statusBarHeight)
        sb.autoresizingMask = [.width]
        sb.isHidden = true
        backdrop.addSubview(sb)
        statusBar = sb
        if config.terminal {
            buildTerminalDrawer(in: backdrop)
        } else {
            terminalShown = false
        }
    }

    private func buildVimPane(over scroll: NSScrollView, below focusBorder: NSView, in backdrop: NSView) {
        let vv = LocalProcessTerminalView(frame: scroll.frame)
        vv.font = PopupWindow.vimFont(config)
        vv.nativeBackgroundColor = .clear
        vv.nativeForegroundColor = config.colors.text
        vv.wantsLayer = true
        vv.layer?.backgroundColor = NSColor.clear.cgColor
        let vr = TerminalAutoRestart()
        vr.onTerminated = { [weak self] in
            DispatchQueue.main.async {
                guard let self, !self.vimShuttingDown else { return }
                self.onVimExit?()
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.15) { [weak self] in
                    guard let self, !self.vimShuttingDown else { return }
                    self.startVimIfNeeded()
                    if self.isShown, self.vimPaneActive, let vv = self.vimView {
                        self.panel.makeFirstResponder(vv)
                        self.focusedPane = .editor
                        self.updateFocusIndicator()
                    }
                }
            }
        }
        vv.processDelegate = vr
        vimRestarter = vr
        backdrop.addSubview(vv, positioned: .below, relativeTo: focusBorder)
        vimView = vv
        scroll.isHidden = true
        if config.vimImageFile != nil {
            let ov = VimImageOverlay(frame: vv.frame)
            ov.onOpen = { [weak self] path in
                FilePopup.show(path: path, over: self?.panel)
            }
            ov.onMenu = { [weak self, weak ov] path, event in
                guard let self, let ov else { return }
                NSMenu.popUpContextMenu(self.imageMenu(path), with: event, for: ov)
            }
            backdrop.addSubview(ov, positioned: .above, relativeTo: vv)
            vimImageOverlay = ov
        }
    }

    private func buildFindBar(in backdrop: NSView) {
        let ff = NSTextField()
        let ffCell = PopupSearchFieldCell()
        ffCell.hInset = 8
        ff.cell = ffCell
        ff.isEditable = true
        ff.isSelectable = true
        ff.focusRingType = .none
        ff.wantsLayer = true
        ff.layer?.backgroundColor = ButtonStyle.inputFill(config.colors).cgColor
        ff.layer?.cornerRadius = 6
        ff.layer?.borderWidth = 1
        ff.layer?.borderColor = ButtonStyle.focusStroke(config.colors).withAlphaComponent(0.6).cgColor
        ff.font = config.rowFont(config.inputFontSize * zoom)
        ff.textColor = config.colors.text
        ff.placeholderAttributedString = NSAttributedString(
            string: "find in note…",
            attributes: [.font: config.rowFont(config.inputFontSize * zoom),
                         .foregroundColor: config.colors.dim])
        ff.isHidden = true
        ff.delegate = self
        backdrop.addSubview(ff)
        findField = ff
        let fc = NSTextField(labelWithString: "")
        fc.font = config.rowFont(config.inputFontSize * zoom)
        fc.textColor = config.colors.dim
        fc.isHidden = true
        backdrop.addSubview(fc)
        findCountLabel = fc
    }

    private func buildTerminalDrawer(in backdrop: NSView) {
        drawerInsetNow = config.terminalHeight
        let term = LocalProcessTerminalView(frame: NSRect(
            x: terminalInset,
            y: backdrop.bounds.height - config.terminalHeight,
            width: max(0, backdrop.bounds.width - 2 * terminalInset),
            height: config.terminalHeight))
        term.autoresizingMask = [.width]
        if let tf = NSFont(name: config.terminalFont, size: config.terminalFontSize) {
            term.font = tf
        }
        term.wantsLayer = true
        term.layer?.cornerRadius = 8
        term.layer?.masksToBounds = true
        term.nativeBackgroundColor = config.terminalBackground
        term.nativeForegroundColor = config.terminalForeground ?? config.colors.text
        backdrop.addSubview(term)
        terminalDrawer = term
        term.installColors(PopupWindow.ansiPalette(config.colors))
        currentTerminalHeight = config.terminalHeight
        let tfb = makeFocusBorder(around: term.frame, radius: 8, resize: [.width])
        backdrop.addSubview(tfb)
        terminalFocusBorder = tfb
        let shell = config.shell, shellArgs = config.shellArgs, dir = config.terminalDir
        let restarter = TerminalAutoRestart()
        restarter.onTerminated = { [weak term] in
            term?.startProcess(executable: shell, args: shellArgs, currentDirectory: dir)
        }
        term.processDelegate = restarter
        terminalRestarter = restarter
        term.startProcess(executable: shell, args: shellArgs, currentDirectory: dir)
        let poll = Timer(timeInterval: 1.5, repeats: true) { [weak term] _ in
            guard let term, let p = term.process else { return }
            if !p.running, !p.windingDown {
                term.startProcess(executable: shell, args: shellArgs, currentDirectory: dir)
            }
        }
        RunLoop.main.add(poll, forMode: .common)
        terminalRestartTimer = poll
        term.menu = terminalMenu(for: term)
        if !config.terminalStartsOpen {
            terminalShown = false
            drawerInsetNow = 0
        }
    }

    private func terminalMenu(for term: LocalProcessTerminalView) -> NSMenu {
        let menu = NSMenu(title: "Terminal")
        func shellItem(_ t: String, _ sel: Selector, _ key: String) -> NSMenuItem {
            let i = NSMenuItem(title: t, action: sel, keyEquivalent: key)
            i.target = term
            return i
        }
        let selection: () -> String? = { [weak term] in
            guard let term, term.selectedRange().length > 0 else { return nil }
            term.copy(NSNull())
            let s = NSPasteboard.general.string(forType: .string)?
                .trimmingCharacters(in: .whitespacesAndNewlines)
            return s?.isEmpty == false ? s : nil
        }
        func pathItem(_ t: String, _ hook: @escaping (PopupWindow) -> ((String) -> Void)?) -> NSMenuItem {
            menuItem(t) { [weak self] in
                guard let self, let path = selection() else { return }
                hook(self)?(path)
            }
        }
        let copy = menuItem("Copy") { [weak term] in term?.copy(NSNull()) }
        copy.keyEquivalent = "c"
        menu.addItem(shellItem("Paste", #selector(LocalProcessTerminalView.paste(_:)), "v"))
        menu.addItem(copy)
        menu.addItem(.separator())
        menu.addItem(pathItem("Open in Notes") { $0.onTerminalOpenInNotes })
        menu.addItem(pathItem("Open in Default App") { $0.onTerminalOpenDefault })
        menu.addItem(pathItem("Reveal in Finder") { $0.onTerminalRevealInFinder })
        menu.addItem(.separator())
        menu.addItem(shellItem("Select All", #selector(LocalProcessTerminalView.selectAll(_:)), "a"))
        return menu
    }

    private func buildList(in backdrop: NSView) {
        let fieldH: CGFloat = 24 * zoom
        let headerOffset = config.dragHeader ? config.headerHeight * zoom + 4 : 0
        let fieldFrame = NSRect(x: config.padding + 10, y: headerOffset + config.padding + 2,
                                width: (config.width - 2 * (config.padding + 10))
                                    * config.searchWidthFraction,
                                height: fieldH)
        func addListBars(to host: NSView, from y: CGFloat) -> CGFloat {
            var bottom = y
            if config.filters {
                let bar = PopupFilterBar(config: config)
                bar.frame = NSRect(x: 0, y: bottom, width: config.width,
                                   height: config.filterBarHeight * zoom)
                bar.autoresizingMask = [.width]
                bar.onSelect = { [weak self] dim, vi in
                    guard let self, self.filterSelections.indices.contains(dim) else { return }
                    var sels = self.filterSelections
                    sels[dim] = vi
                    self.filterSelections = sels
                }
                host.addSubview(bar)
                filterBar = bar
                bottom += config.filterBarHeight * zoom + 2
            }
            if config.tabs && config.tabsSidebarWidth > 0 {
                let bar = makeTabsBar(frame: NSRect(x: 0, y: 0, width: config.tabsSidebarWidth * zoom, height: 100))
                bar.vertical = true
                bar.autoresizingMask = []
                host.addSubview(bar)
                tabsBar = bar
            } else if config.tabs {
                let bar = makeTabsBar(frame: NSRect(x: 0, y: bottom, width: config.width,
                                                    height: config.tabBarHeight * zoom))
                host.addSubview(bar)
                tabsBar = bar
                bottom += config.tabBarHeight * zoom + 2
            }
            return bottom
        }
        field.frame = fieldFrame
        field.autoresizingMask = [.width]
        if config.scrollableRows {
            backdrop.addSubview(field)
            let chromeBottom = addListBars(to: backdrop, from: fieldFrame.maxY + 4)
            self.chromeBottom = chromeBottom
            let scroll = NSScrollView(frame: NSRect(x: 0, y: chromeBottom,
                                                    width: backdrop.bounds.width,
                                                    height: max(40, backdrop.bounds.height - chromeBottom)))
            scroll.autoresizingMask = [.width, .height]
            scroll.hasVerticalScroller = true
            scroll.autohidesScrollers = true
            scroll.drawsBackground = false
            scroll.borderType = .noBorder
            rowView.topInset = 4
            rowView.bottomInset = 16
            if !config.tableColumns.isEmpty {
                let header = PopupTableHeaderView(config: config)
                header.frame = NSRect(x: 0, y: 0, width: backdrop.bounds.width,
                                      height: config.tableHeaderHeight * zoom)
                header.autoresizingMask = [.width]
                tableHeader = header
                rowView.topInset = config.tableHeaderHeight * zoom + 2
                wireTableHeader(header)
            }
            scroll.documentView = rowView
            if let header = tableHeader {
                scroll.addFloatingSubview(header, for: .vertical)
            }
            backdrop.addSubview(scroll)
            rowScroll = scroll
        } else {
            rowView.addSubview(field)
            rowView.topInset = addListBars(to: rowView, from: fieldFrame.maxY + 4)
            backdrop.addSubview(rowView)
        }
    }

    private func wireRows() {
        if config.clickToSelect {
            rowView.onRowClick = { [weak self] index in
                guard let self, self.rows.indices.contains(index) else { return }
                self.selection = index
                self.onRowClick?(index)
            }
            rowView.onRowDoubleClick = { [weak self] index in
                guard let self, self.rows.indices.contains(index) else { return }
                self.selection = index
                self.onRowDoubleClick?(index)
            }
        }
        if config.selectableRows {
            rowView.onToggleSelect = { [weak self] index in
                self?.toggleRowSelection(index)
            }
        }
        rowView.onToggleStar = { [weak self] i in self?.onToggleStar?(i) }
    }

    private func wireTableHeader(_ header: PopupTableHeaderView) {
        header.onSort = { [weak self] i in self?.onTableSort?(i) }
        header.extraMenu = { [weak self] in self?.onTableHeaderMenu?() ?? [] }
        header.onFilter = { [weak self] i, r in
            guard let self, let h = self.tableHeader else { return }
            self.onTableFilter?(i, h, r)
        }
        header.onReorder = { [weak self] from, to in
            guard let self else { return }
            var cols = self.config.tableColumns
            guard cols.indices.contains(from), cols.indices.contains(to) else { return }
            cols.insert(cols.remove(at: from), at: to)
            self.setTableColumns(cols)
            self.onTableColumnsReordered?(from, to)
        }
        header.onResize = { [weak self] pcts, final in
            guard let self else { return }
            var cols = self.config.tableColumns
            for i in cols.indices where i < pcts.count { cols[i].width = pcts[i] }
            self.setTableColumns(cols)
            self.onTableColumnsResized?(pcts, final)
        }
    }

    private func buildChrome(in backdrop: NSView) {
        let chrome = PopupChrome(config: config)
        if !(panel is PopupBaseWindow) && !config.toolPanel { chrome.config.headerCloseButton = false }
        chrome.frame = backdrop.bounds
        chrome.autoresizingMask = [.width, .height]
        if config.editMode || config.dragHeader {
            chrome.dragHeaderHeight = config.headerHeight * zoom
            chrome.headerTitle = chromeHeaderTitle
        } else if config.scrollableRows {
            chrome.dragAnywhere = false
        } else {
            chrome.dragAnywhere = true
            let top = config.padding + 2
            let bottom = top + 24 * zoom + 4 + (config.tabs ? config.tabBarHeight * zoom + 2 : 0)
            chrome.reservedRect = NSRect(x: 0, y: top, width: config.width,
                                         height: bottom - top)
        }
        chrome.onMeterRecord = { [weak self] in self?.onMeterRecord?() }
        chrome.onMeterPause = { [weak self] in self?.onMeterPause?() }
        backdrop.addSubview(chrome)
        self.chrome = chrome
    }

    private func wireCloseButton() {
        guard config.showCloseButton, let closeBtn = panel.standardWindowButton(.closeButton) else { return }
        let target = ClosureTarget { [weak self] in
            if let onCloseWindow = self?.onCloseWindow {
                onCloseWindow()
            } else {
                self?.hide(restore: true)
            }
        }
        closeBtn.target = target
        closeBtn.action = #selector(ClosureTarget.run)
        windowCloseTarget = target
    }

    private func wireHeaderClicks() {
        guard config.editMode || config.dragHeader, let base = panel as? HeaderClickWindow else { return }
        base.headerClickBand = config.headerHeight * zoom
        base.onHeaderClick = { [weak self] point in
            guard let self, let chrome = self.chrome else { return }
            let p = NSPoint(x: point.x, y: self.panel.frame.height - point.y)
            if chrome.closeButtonRect.insetBy(dx: -2, dy: -2).contains(p) {
                if let onCloseWindow = self.onCloseWindow { onCloseWindow() } else { self.handleEscape() }
            } else if let hit = chrome.extraButtonRects.first(where: { $0.value.contains(p) }) {
                self.onHeaderButton?(hit.key)
                chrome.showCopiedFeedback(hit.key)
            } else if chrome.copyRowsLabel != nil, chrome.copyRowsButtonRect.contains(p) {
                self.performCopyRows()
                chrome.showCopiedFeedback(3)
            } else if chrome.configButtonRect.contains(p) {
                self.onChromeConfigClick?()
                chrome.showCopiedFeedback(2)
            } else if chrome.copyButtonRect.contains(p) {
                self.onChromeHeaderClick?()
                chrome.showCopiedFeedback(1)
            } else if chrome.headerIcon != nil, chrome.iconButtonRect.insetBy(dx: -4, dy: -4).contains(p) {
                self.onChromeIconClick?()
            }
        }
    }

    public func start() {
        if config.enableToggle {
            startToggleServer()
        }
    }

    public func show() {
        if !quietShow, Self.builtForHost?(self) == true { quietShow = true }
        presentList()
    }
    public static var builtForHost: ((PopupWindow) -> Bool)?

    public var initialFrame: NSRect?
    public var quietShow = false
    public var initialQuery: String?

    private func beginShown() {
        let quiet = quietShow
        quietShow = false
        if !quiet {
            isShown = true
            installMonitors()
        }
        focusRetries = 0
    }

    public var onPark: (() -> Void)?
    public var onUnpark: (() -> Void)?
    public func park(stopVoice: Bool = false) {
        guard isShown else { slotDetach(); return }
        isShown = false
        removeMonitors()
        if config.editMode, editorView != nil {
            onEditorClose?(currentEditorText)
        }
        if stopVoice { onHideVoiceStop?() }
        onPark?()
        slotDetach()
        panel.orderOut(nil)
    }

    func slotAttach(to host: SlotHostWindow) {
        guard panel === homeWindow else { return }
        host.take(self, chromeOf: homeWindow)
        host.delegate = self
        SlotHostWindow.moveContent(from: homeWindow, to: host)
        panel = host
    }
    func slotDetach() {
        guard panel !== homeWindow, let host = panel as? SlotHostWindow else { return }
        host.give(chromeTo: homeWindow)
        homeWindow.setFrame(host.frame, display: false)
        SlotHostWindow.moveContent(from: host, to: homeWindow)
        if host.delegate === self { host.delegate = nil }
        panel = homeWindow
        host.guestLeft(self)
    }
    public func unpark(frame: NSRect?) {
        if let f = frame { setBaseFrame(f) }
        if isShown {
            NSApp.activate(ignoringOtherApps: true)
            panel.makeKeyAndOrderFront(nil)
        } else {
            showPersistent()
            NSApp.activate(ignoringOtherApps: true)
        }
        if config.editMode { layoutForZoom() } else {
            layoutSearchField()
            layoutScrollDocument()
            relayoutTabs()
        }
        onUnpark?()
    }

    private func presentList() {
        onShow?()
        if config.editMode {
            if let attr = editorAttributed {
                editorView?.textStorage?.setAttributedString(attr)
            } else {
                editorView?.string = editorText
            }
            editorView?.isEditable = !editorReadOnly
            wireEditorTextChange()
            if let f = initialFrame {
                setBaseFrame(f)
                initialFrame = nil
            } else {
                let h = min(config.height, maxPanelHeight())
                let origin = centeredOrigin(width: config.width, height: h)
                panel.setContentSize(NSSize(width: config.width, height: h))
                panel.setFrameOrigin(origin)
            }
            layoutForZoom()
            beginShown()
            startVimIfNeeded()
            if let ed = primaryEditor {
                panel.makeFirstResponder(ed)
                focusedPane = .editor
            }
            updateFocusIndicator()
            takeFocus()
            return
        }
        let q = initialQuery ?? ""
        initialQuery = nil
        let initial = onFilter?(q) ?? []
        setRows(initial)
        field.stringValue = q
        rowView.highlightQuery = q

        let contentH = rowView.contentHeight()
        let height: CGFloat
        if config.scrollableRows {
            height = min(maxPanelHeight(), config.height > 0 ? config.height : max(200, contentH))
        } else {
            height = min(contentH, maxPanelHeight())
        }
        let origin = centeredOrigin(width: config.width, height: height)
        panel.setContentSize(NSSize(width: config.width, height: height))
        panel.setFrameOrigin(origin)
        panel.setFrame(clampToScreen(panel.frame), display: true)
        if let f = initialFrame {
            panel.setFrame(f, display: true)
            initialFrame = nil
        }
        let docW = panel.contentView?.bounds.width ?? config.width
        if config.scrollableRows {
            rowView.frame = NSRect(x: 0, y: 0, width: docW, height: contentH)
        } else {
            rowView.frame = NSRect(x: 0, y: 0, width: docW, height: height)
        }
        rowView.sizingRowCount = rows.count
        rowView.needsDisplay = true

        beginShown()
        relayoutTabs()
        layoutSearchField()
        growWidthToContent()
        takeFocus()
        if !q.isEmpty {
            field.currentEditor()?.selectedRange = NSRange(location: (q as NSString).length, length: 0)
        }
    }

    public func hide(restore: Bool) {
        guard isShown else { return }
        isShown = false
        let front = NSWorkspace.shared.frontmostApplication?.bundleIdentifier ?? "?"
        let caller = Thread.callStackSymbols.dropFirst().prefix(4)
            .map { $0.split(separator: " ", omittingEmptySubsequences: true).dropFirst(3).prefix(1).joined() }
            .joined(separator: " < ")
        appendToFile(debugLogPath, "ws: hide '\(config.name)' restore=\(restore) front=\(front) via \(caller)\n")
        removeMonitors()
        slotDetach()
        panel.orderOut(nil)
        if config.editMode, editorView != nil {
            onEditorClose?(currentEditorText)
        }
        onHideVoiceStop?()
        onHide?(restore)
    }

    public func showPersistent() {
        guard !isShown else { return }
        onShow?()
        isShown = true
        installMonitors()
        focusRetries = 0
        relayoutTabs()
        layoutEditorScroll()
        startVimIfNeeded()
        panel.makeKeyAndOrderFront(nil)
        takeFocus()
    }

    public func releaseHooks() {
        onShow = nil
        onFilter = nil
        onTableFilter = nil
        onToggleStar = nil
        onTableColumnsReordered = nil
        onPark = nil
        onUnpark = nil
        onFilterOpen = nil
        onAccept = nil
        onRowClick = nil
        onRowDoubleClick = nil
        onEscape = nil
        onHide = nil
        onHideVoiceStop = nil
        onEditorCommit = nil
        onEditorClose = nil
        onDrawRow = nil
        onCopyRows = nil
        onCommandK = nil
        onCommandF = nil
        onSelectionChanged = nil
        onInspectorOpen = nil
        closeActionPicker()
        onHeaderButton = nil
        onMeterRecord = nil
        onMeterPause = nil
        onChromeHeaderClick = nil
        onChromeConfigClick = nil
        onTabChange = nil
        onTabClick = nil
        onAddTab = nil
        onNewNote = nil
        onFilterChange = nil
    }

    public func toggle() {
        if isShown {
            hide(restore: true)
        } else {
            show()
        }
    }

    public func showHeaderMenu(_ menu: NSMenu) {
        isShowingMenu = true
        chrome?.iconMenuOpen = true
        defer { chrome?.iconMenuOpen = false }
        let pt = NSPoint(x: (chrome?.iconButtonRect.minX ?? 6) + 4, y: panel.frame.height - config.headerHeight * zoom - 4)
        let screenPt = panel.convertPoint(toScreen: pt)
        menu.popUp(positioning: nil, at: screenPt, in: nil)
        isShowingMenu = false
    }

    public func resetToDefaultSize() {
        zoom = 1
        var h = config.height
        if config.editMode {
            let launchDrawer = config.fileBrowserDefault && fileBrowser != nil ? config.fileBrowserHeight
                : (config.terminal && config.terminalStartsOpen ? config.terminalHeight : 0)
            h += (terminalShown ? preferredTerminalHeight : 0)
                + (fileBrowserShown ? preferredBrowserHeight : 0) - launchDrawer
            currentTerminalHeight = preferredTerminalHeight
            currentBrowserHeight = preferredBrowserHeight
            drawerInsetNow = (terminalShown ? preferredTerminalHeight : 0)
                + (fileBrowserShown ? preferredBrowserHeight : 0)
        }
        h = min(h, maxPanelHeight())
        panel.setContentSize(NSSize(width: config.width, height: h))
        panel.setFrameOrigin(centeredOrigin(width: config.width, height: h))
        if config.editMode {
            layoutForZoom()
        }
    }

    public func resetToDefaultColors() {
        let base = PopupConfig(name: "")
        setThemeColor(base.fileBrowserBackground, for: .browser)
        setThemeColor(base.terminalBackground, for: .terminal)
        let notepadDefault = base.colors.background.withAlphaComponent(base.tintAlpha)
        setThemeColor(notepadDefault, for: .notepad)
        config.headerColor = nil
        chrome?.headerColorOverride = config.colors.background
        panel.contentView?.needsDisplay = true
    }

    public func setRows(_ newRows: [PopupRow], resetScroll: Bool = true) {
        rows = newRows
        defer { onRowsChanged?() }
        if selection >= rows.count {
            selection = max(0, rows.count - 1)
        }
        if config.selectableRows {
            rowView.selected = rowView.selected.filter { rows.indices.contains($0) }
            updateCopyRowsLabel()
        }
        rowView.rows = rows
        rowView.selection = selection
        rowView.needsDisplay = true
        if config.scrollableRows {
            layoutScrollDocument()
            rowView.sizingRowCount = rows.count
            if resetScroll {
                scrollRowsToTop()
            }
            return
        }
        if config.dynamicHeight, isShown {
            let height = min(rowView.contentHeight(), maxPanelHeight())
            panel.setContentSize(NSSize(width: config.width, height: height))
            rowView.frame = NSRect(x: 0, y: 0, width: config.width, height: height)
        }
    }

    private func scrollSelectionIntoView() {
        guard config.scrollableRows, isShown,
              let scroll = rowScroll, selection >= 0, selection < rows.count,
              let rv = rowScroll?.documentView as? PopupRowView else { return }
        let r = rv.rect(for: selection)
        let vis = scroll.documentVisibleRect
        let inset: CGFloat = 8
        let targetY: CGFloat
        if r.minY < vis.minY {
            targetY = r.minY - inset
        } else if r.maxY > vis.maxY {
            targetY = r.maxY - vis.height + inset
        } else {
            return
        }
        let maxTarget = max(0, (scroll.documentView?.frame.height ?? 0) - vis.height)
        let clamped = min(max(0, targetY), maxTarget)
        guard abs(clamped - vis.minY) > 2 else { return }
        if abs(clamped - vis.minY) > 48 {
            scroll.contentView.animator().scroll(to: NSPoint(x: vis.minX, y: clamped))
        } else {
            scroll.contentView.scroll(to: NSPoint(x: vis.minX, y: clamped))
            scroll.reflectScrolledClipView(scroll.contentView)
        }
    }

    private func scrollRowsToTop() {
        guard let scroll = rowScroll else { return }
        scroll.contentView.scroll(to: .zero)
        scroll.reflectScrolledClipView(scroll.contentView)
    }

    public func clearInput() {
        field.stringValue = ""
        rowView.highlightQuery = ""
    }

    public var currentEditorText: String {
        config.markdownImages ? editorMarkdown : (editorView?.string ?? editorText)
    }

    public var imageSaver: ((NSImage) -> String?)?
    public var imageBaseDir: String = ""
    public var attachmentPaths: [NSTextAttachment: String] = [:]

    private var editorAttributed: NSAttributedString?

    public func setEditorText(_ s: String) {
        editorAttributed = nil
        editorText = s
        editorView?.string = s
        restyleEditor()
    }

    private func wireEditorTextChange() {
        (editorView as? PopupTextView)?.onTextChange = onEditorTextChange
    }

    private func restyleEditor() {
        guard let tv = editorView, let storage = tv.textStorage else { return }
        let font = editorFont(config.fontName, zoom, size: config.editorFontSize)
        let attrs: [NSAttributedString.Key: Any] = [
            .font: font, .foregroundColor: config.colors.text,
        ]
        storage.beginEditing()
        storage.addAttributes(attrs, range: NSRange(0..<storage.length))
        storage.endEditing()
        tv.typingAttributes = attrs
    }

    public var editorSelection: NSRange {
        editorView?.selectedRange() ?? NSRange(location: (editorText as NSString).length, length: 0)
    }

    @discardableResult
    public func replaceRange(_ range: NSRange, with s: String, caretBack: Int = 0) -> Int {
        guard let tv = editorView, let storage = tv.textStorage else { return 0 }
        let loc = max(0, min(range.location, storage.length))
        let len = max(0, min(range.length, storage.length - loc))
        storage.replaceCharacters(in: NSRange(location: loc, length: len), with: s)
        editorText = tv.string
        restyleEditor()
        let n = (s as NSString).length
        let caret = NSRange(location: loc + max(0, n - caretBack), length: 0)
        tv.setSelectedRange(caret)
        tv.scrollRangeToVisible(caret)
        return n
    }

    private func makeAttachment(_ img: NSImage, rel: String) -> NSTextAttachment {
        let att = NSTextAttachment()
        att.image = img
        let maxW = editorScroll?.bounds.width ?? config.width
        var size = img.size
        if size.width > maxW {
            size = NSSize(width: maxW, height: size.height * maxW / size.width)
        }
        att.bounds = NSRect(origin: .zero, size: size)
        attachmentPaths[att] = rel
        return att
    }

    public func insertImageAttachment(rel: String, image: NSImage) {
        guard let tv = editorView else { return }
        let att = makeAttachment(image, rel: rel)
        let str = NSMutableAttributedString(attachment: att)
        str.append(NSAttributedString(string: "\n"))
        tv.textStorage?.replaceCharacters(in: tv.selectedRange, with: str)
        editorText = tv.string
        editorAttributed = tv.attributedString()
        restyleEditor()
        syncEditorDocWidth()
        scrollEditorToEnd()
    }

    public func setEditorMarkdown(_ s: String, baseDir: String) {
        guard config.markdownImages, let tv = editorView else {
            setEditorText(s)
            return
        }
        let plainAttrs: [NSAttributedString.Key: Any] = [
            .font: tv.font ?? NSFont.monospacedSystemFont(ofSize: 13, weight: .regular),
            .foregroundColor: tv.textColor ?? NSColor.white,
        ]
        let storage = NSMutableAttributedString()
        let ns = s as NSString
        let rx = try! NSRegularExpression(pattern: "!\\[[^\\]]*\\]\\(([^)]+)\\)")
        var loc = 0
        for m in rx.matches(in: s, range: NSRange(0..<ns.length)) {
            if m.range.location > loc {
                storage.append(NSAttributedString(
                    string: ns.substring(with: NSRange(location: loc,
                                                       length: m.range.location - loc)),
                    attributes: plainAttrs))
            }
            let rel = ns.substring(with: m.range(at: 1))
            let path = (baseDir as NSString).appendingPathComponent(rel)
            if let img = NSImage(contentsOfFile: path) {
                storage.append(NSAttributedString(attachment:
                    makeAttachment(img, rel: rel)))
                storage.append(NSAttributedString(string: "\n", attributes: plainAttrs))
            } else {
                storage.append(NSAttributedString(string: ns.substring(with: m.range),
                                                  attributes: plainAttrs))
            }
            loc = m.range.location + m.range.length
        }
        if loc < ns.length {
            storage.append(NSAttributedString(string: ns.substring(from: loc),
                                              attributes: plainAttrs))
        }
        editorAttributed = storage
        editorText = storage.string
        tv.textStorage?.setAttributedString(storage)
        restyleEditor()
        syncEditorDocWidth()
    }

    @discardableResult
    public func setEditorFilePreview(_ path: String) -> Bool {
        guard let tv = editorView else { return false }
        let plainAttrs: [NSAttributedString.Key: Any] = [
            .font: editorFont(config.fontName, zoom, size: config.editorFontSize),
            .foregroundColor: config.colors.text,
        ]
        let name = URL(fileURLWithPath: path).lastPathComponent
        let storage = NSMutableAttributedString()
        let ext = (path as NSString).pathExtension.lowercased()
        if ext == "pdf", let doc = PDFDocument(url: URL(fileURLWithPath: path)) {
            guard doc.pageCount > 0 else { return false }
            for i in 0..<doc.pageCount {
                guard let page = doc.page(at: i) else { continue }
                let thumb = page.thumbnail(of: NSSize(width: 1400, height: 1400), for: .mediaBox)
                storage.append(NSAttributedString(attachment: makeAttachment(thumb, rel: "\(name)#\(i)")))
                storage.append(NSAttributedString(string: "\n", attributes: plainAttrs))
            }
        } else if ext == "rtf", let img = PopupFileBrowser.rtfPreviewImage(path) {
            storage.append(NSAttributedString(attachment: makeAttachment(img, rel: name)))
            storage.append(NSAttributedString(string: "\n", attributes: plainAttrs))
        } else if let img = NSImage(contentsOfFile: path) {
            storage.append(NSAttributedString(attachment: makeAttachment(img, rel: name)))
            storage.append(NSAttributedString(string: "\n", attributes: plainAttrs))
        } else {
            return false
        }
        editorAttributed = storage
        editorText = storage.string
        tv.textStorage?.setAttributedString(storage)
        restyleEditor()
        syncEditorDocWidth()
        editorReadOnly = true
        return true
    }

    public var editorMarkdown: String {
        guard config.markdownImages, let tv = editorView,
              let storage = tv.textStorage else {
            return editorView?.string ?? editorText
        }
        var out = ""
        storage.enumerateAttributes(in: NSRange(0..<storage.length)) { attrs, range, _ in
            if let att = attrs[.attachment] as? NSTextAttachment,
               let rel = self.attachmentPaths[att] {
                out += "![](\(rel))\n"
            } else {
                out += (storage.string as NSString).substring(with: range)
            }
        }
        return out
    }

    private func syncEditorDocWidth() {
        guard config.markdownImages, let tv = editorView,
              let scroll = editorScroll else { return }
        var widest: CGFloat = 0
        for att in attachmentPaths.keys {
            widest = max(widest, att.bounds.width)
        }
        let want = max(scroll.bounds.width, widest + 24)
        if abs(tv.frame.width - want) > 1 {
            tv.frame.size.width = want
        }
    }

    public func setEditorAttributedText(_ s: NSAttributedString) {
        editorAttributed = s
        editorText = s.string
        editorView?.textStorage?.setAttributedString(s)
    }

    public func scrollEditorToEnd(ifAtBottom: Bool = false) {
        guard let tv = editorView, let scroll = editorScroll,
              let doc = scroll.documentView else { return }
        if ifAtBottom {
            let visible = scroll.documentVisibleRect
            let docH = doc.frame.height
            guard docH - visible.maxY < 80 else { return }
        }
        tv.layoutManager?.ensureLayout(for: tv.textContainer!)
        let docH = doc.frame.height
        let y = max(0, docH - scroll.contentView.bounds.height)
        scroll.contentView.scroll(to: NSPoint(x: 0, y: y))
        scroll.reflectScrolledClipView(scroll.contentView)
    }

    public func setEditorANSI(_ s: String) {
        let font = editorFont(config.fontName, config.zoom, size: config.editorFontSize)
        setEditorAttributedText(parseANSI(s, baseFont: font, defaultColor: config.colors.text))
    }

    public func setEditorSyntaxHighlighted(_ text: String) {
        let font = editorFont(config.fontName, zoom, size: config.editorFontSize)
        setEditorAttributedText(popupHighlightSyntax(text, font: font,
                                                     colors: config.colors))
        if let tv = editorView {
            tv.typingAttributes = [
                .font: font, .foregroundColor: config.colors.text,
            ]
        }
    }

    private func takeFocus() {
        guard isShown else { return }
        if config.toolPanel {
            panel.orderFrontRegardless()
            panel.makeKey()
        } else {
            panel.makeKeyAndOrderFront(nil)
        }
        if !panel.isKeyWindow, !NSApp.isActive, !config.toolPanel {
            NSApp.activate(ignoringOtherApps: true)
        }
        if !paneHoldsFocus() {
            if let ed = primaryEditor {
                panel.makeFirstResponder(ed)
                focusedPane = .editor
            } else if let fb = fileBrowser, !fileBrowserDrawerMode {
                panel.makeFirstResponder(fb.listView)
            } else {
                panel.makeFirstResponder(field)
            }
        }
        if !panel.isKeyWindow, focusRetries < 10 {
            focusRetries += 1
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.15) { [weak self] in
                self?.takeFocus()
            }
        }
    }

    private var activeObserver: NSObjectProtocol?
    private var responderObserver: NSObjectProtocol?

    private func installMonitors() {
        if activeObserver == nil, !config.toolPanel {
            activeObserver = NotificationCenter.default.addObserver(
                forName: NSApplication.didBecomeActiveNotification, object: nil,
                queue: .main) { [weak self] _ in
                guard let self, self.isShown, !self.isShowingMenu else { return }
                self.panel.makeKeyAndOrderFront(nil)
                if let term = self.terminalDrawer, self.terminalShown,
                   self.terminalFocused(term) {
                    return
                }
                if self.paneHoldsFocus() { return }
                if let tv = self.primaryEditor {
                    self.panel.makeFirstResponder(tv)
                } else {
                    self.panel.makeFirstResponder(self.field)
                }
            }
        }
        if let m = NSEvent.addLocalMonitorForEvents(matching: .keyDown, handler: {
            [weak self] event in
            guard let self, self.isShown, self.panel.isKeyWindow
                || self.panel.attachedSheet != nil else { return event }
            if self.panel.attachedSheet == nil, let ic = PopupWindow.keyInterceptor, ic(event, self.panel) { return nil }
            if event.keyCode == 53, let dismiss = PopupWindow.transientEscape {
                dismiss()
                return nil
            }
            if self.topAccessory != nil {
                if event.keyCode == 53, let esc = self.onAccessoryEscape {
                    esc()
                    return nil
                }
                let m = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
                if self.topAccessoryHasFocus, !m.contains(.command),
                   !(m.contains(.control) && event.keyCode == 48) {
                    return event
                }
            }
            if self.handleKey(event.keyCode, event.modifierFlags) {
                return nil
            }
            return event
        }) {
            monitors.append(m)
        }
        if config.sticky {
        } else if config.dismissOnClickOff,
                  let m = NSEvent.addGlobalMonitorForEvents(matching: .leftMouseDown, handler: {
            [weak self] event in
            guard let self, self.isShown else { return }
            let p = NSEvent.mouseLocation
            if !self.panel.frame.contains(p) {
                self.hide(restore: false)
            }
        }) {
            monitors.append(m)
        }
        responderObserver = NotificationCenter.default.addObserver(
            forName: NSWindow.didBecomeKeyNotification, object: panel,
            queue: .main) { [weak self] _ in
            self?.updateFocusedPane()
        }
        if let m = NSEvent.addLocalMonitorForEvents(matching: .leftMouseDown, handler: {
            [weak self] event in
            guard let self, self.isShown, self.panel.isKeyWindow else { return event }
            DispatchQueue.main.async { self.updateFocusedPane() }
            return event
        }) {
            monitors.append(m)
        }
    }

    private func updateFocusedPane() {
        let fr = panel.firstResponder
        var inVim = false
        if let vv = vimView, vimPaneActive, fr === vv || (fr as? NSView)?.isDescendant(of: vv) == true {
            inVim = true
        }
        var inEditor = false
        if let ed = editorView, fr === ed || (fr as? NSTextView)?.isDescendant(of: ed) == true {
            inEditor = true
        }
        var inBrowser = false
        if fileBrowserShown, let fb = fileBrowser {
            let lv = fb.listView
            if fr === lv || (fr as? NSView)?.isDescendant(of: lv) == true {
                inBrowser = true
            }
        }
        var inTerminal = false
        if terminalShown, let term = terminalDrawer,
           fr === term || (fr as? NSView)?.isDescendant(of: term) == true {
            inTerminal = true
        }
        if inVim || inEditor {
            focusedPane = .editor
        } else if inBrowser {
            focusedPane = .browser
        } else if inTerminal {
            focusedPane = .terminal
        }
        updateFocusIndicator()
    }

    private func removeMonitors() {
        for m in monitors {
            NSEvent.removeMonitor(m)
        }
        monitors = []
        if let o = activeObserver {
            NotificationCenter.default.removeObserver(o)
            activeObserver = nil
        }
        if let o = responderObserver {
            NotificationCenter.default.removeObserver(o)
            responderObserver = nil
        }
    }

    private func activeTextEditor() -> NSTextView? {
        func editor(_ responder: NSResponder?) -> NSTextView? {
            if let tv = responder as? NSTextView { return tv }
            if let f = responder as? NSTextField, let ed = f.currentEditor() as? NSTextView { return ed }
            return nil
        }
        if let sheet = panel.attachedSheet {
            if let ed = editor(sheet.firstResponder) { return ed }
        }
        return editor(panel.firstResponder)
    }

    public var onKeyPreview: ((UInt16, NSEvent.ModifierFlags) -> Bool)?
    public static var keyInterceptor: ((NSEvent, NSWindow) -> Bool)?
    public var searchFieldFrame: NSRect { field.frame }
    public func focusSearchField() { panel.makeFirstResponder(field) }
    public var currentSearchText: String { field.stringValue }

    private func handleKey(_ code: UInt16, _ mods: NSEvent.ModifierFlags) -> Bool {
        if proseShown, let pv = proseView, !pv.isHidden, panel.attachedSheet == nil,
           let r = pv.searchKey(code: code, mods: mods) {
            if r, code != 53, panel.firstResponder !== pv.web { panel.makeFirstResponder(pv.web) }
            return r
        }
        if let r = overlayKey(code, mods) { return r }
        if let r = windowSizeKey(code, mods) { return r }
        if mods.contains(.command) || mods.contains(.control),
           let r = modifiedKey(code, mods) { return r }
        if proseShown, let pv = proseView, !pv.isHidden,
           mods.intersection([.command, .control, .option, .shift]).isEmpty,
           code == 38 || code == 40, !(panel.firstResponder is NSText) {
            pv.scrollBy(code == 38 ? 60 : -60)
            return true
        }
        if !config.editMode, jumpLeaderKey(code, mods) { return true }
        return config.editMode ? editorKey(code, mods) : listKey(code, mods)
    }

    private var jumpLeader = 0
    private var jumpLeaderAt = Date.distantPast
    private func jumpLeaderKey(_ code: UInt16, _ mods: NSEvent.ModifierFlags) -> Bool {
        guard let hook = onSidebarJump, panel.attachedSheet == nil, actionPicker == nil,
              shortcutsSheet == nil, !topAccessoryHasFocus,
              mods.intersection([.command, .control, .option, .shift]).isEmpty else { jumpLeader = 0; return false }
        if jumpLeader > 0, Date().timeIntervalSince(jumpLeaderAt) > 1.2 { jumpLeader = 0 }
        let typing = !((panel.firstResponder as? NSText)?.string.isEmpty ?? true)
        switch (jumpLeader, code) {
        case (0, 49) where !typing:
            jumpLeader = 1; jumpLeaderAt = Date(); return true
        case (1, 1):
            jumpLeader = 2; return true
        case (2, 3):
            jumpLeader = 0; hook(); return true
        default:
            jumpLeader = 0; return false
        }
    }

    private func modifiedKey(_ code: UInt16, _ mods: NSEvent.ModifierFlags) -> Bool? {
        if panel.attachedSheet != nil { return sheetEditKey(code, mods) }
        if let r = viewCycleKey(code, mods) { return r }
        if code == 42, mods.contains(.command), !mods.contains(.control), !mods.contains(.shift),
           PopupTabsBar.toggleRail(in: panel) { return true }
        if code == 35, mods.contains(.command), mods.contains(.shift), proseProvider != nil {
            setProse(!proseShown)
            return true
        }
        if code == 31, mods.contains(.command), mods.contains(.shift), proseProvider != nil {
            popOutProse()
            return true
        }
        if code == 35, proseShown, mods.intersection([.command, .control, .option, .shift]) == .command {
            proseView?.exportPDF()
            return true
        }
        if code == 45, mods.intersection([.command, .control, .option, .shift]) == .command,
           let onNewNote {
            onNewNote(false)
            return true
        }
        if code == 45, mods.intersection([.command, .control, .option, .shift]) == .control,
           let onNewNote, focusedTerm() == nil,
           !(fileBrowser.map(browserHasFocus) ?? false),
           focusedVim() == nil || vimEval("mode() . (get(g:, 'ws_picking', 0) ? 'p' : '')")?.trimmingCharacters(in: .whitespacesAndNewlines) == "n" {
            onNewNote(true)
            return true
        }
        if (code == 38 || code == 40), mods.contains(.control), mods.contains(.shift),
           !mods.contains(.command), !mods.contains(.option), focusedVim() != nil {
            vimRemote(code == 38 ? "<Plug>(VM-Add-Cursor-Down)" : "<Plug>(VM-Add-Cursor-Up)")
            return true
        }
        if let r = paneResizeKey(code, mods) { return r }
        if let vv = focusedVim() { return vimPaneKey(vv, code, mods) }
        if let term = focusedTerm() { return terminalKey(term, code, mods) }
        if let r = hostShortcutKey(code, mods) { return r }
        if let r = fileBrowserKey(code, mods) { return r }
        return editKey(code, mods)
    }

    private func overlayKey(_ code: UInt16, _ mods: NSEvent.ModifierFlags) -> Bool? {
        if let hook = onKeyPreview, hook(code, mods) { return true }
        if proseShown, code == 53, mods.intersection([.command, .control, .option]).isEmpty {
            setProse(false)
            return true
        }
        if actionPicker != nil { return actionPickerKey(code, mods) }
        if shortcutsSheet != nil { return shortcutsKey(code, mods) }
        if code == 44, mods.contains(.command), panel.attachedSheet == nil, let hook = onShowShortcuts {
            hook()
            return true
        }
        return nil
    }

    private func windowSizeKey(_ code: UInt16, _ mods: NSEvent.ModifierFlags) -> Bool? {
        if code != 53 { escStreak = 0 }
        let cmd = mods.contains(.command)
        let plus = code == 24 || code == 69, minus = code == 27 || code == 78
        if cmd, (plus || minus), panel.attachedSheet == nil {
            let sign = plus ? 1 : -1
            if mods.contains(.option) {
                resizeBy(CGFloat(80 * sign))
            } else if proseShown, let pv = proseView {
                pv.zoom(by: plus ? 1.1 : 1 / 1.1); pv.saveZoom()
            } else if let step = onFontSizeStep {
                step(sign)
            } else if textZoomKey != nil {
                setTextZoom(textZoom * (plus ? 1.1 : 1 / 1.1))
            } else {
                resizeBy(CGFloat(80 * sign))
            }
            return true
        }
        if cmd, code == 29, !mods.contains(.option), proseShown, let pv = proseView, panel.attachedSheet == nil {
            pv.resetZoom(); pv.saveZoom()
            return true
        }
        if cmd, code == 29, !mods.contains(.option), onFontSizeStep == nil, textZoomKey != nil, panel.attachedSheet == nil {
            setTextZoom(1)
            return true
        }
        return nil
    }

    private func sheetEditKey(_ code: UInt16, _ mods: NSEvent.ModifierFlags) -> Bool {
        let cmd = mods.contains(.command)
        let ctrl = mods.contains(.control)
        if let ed = activeTextEditor() {
            switch code {
            case 9 where cmd || ctrl: ed.paste(nil); return true
            case 0 where cmd: ed.selectAll(nil); return true
            case 8 where cmd: ed.copy(nil); return true
            case 7 where cmd: ed.cut(nil); return true
            case 6 where cmd: ed.undoManager?.undo(); return true
            default: return false
            }
        }
        return false
    }

    private func viewCycleKey(_ code: UInt16, _ mods: NSEvent.ModifierFlags) -> Bool? {
        let cmd = mods.contains(.command)
        let ctrl = mods.contains(.control)
        if ctrl && !cmd && code == 48, let hook = onCycleView, panel.attachedSheet == nil {
            hook(mods.contains(.shift) ? -1 : 1)
            return true
        }
        if ctrl && !cmd && code == 48, config.tabs, tabTitles.count > 1 {
            let n = tabTitles.count
            selectedTab = (selectedTab + (mods.contains(.shift) ? -1 : 1) + n) % n
            return true
        }
        return nil
    }

    private func paneResizeKey(_ code: UInt16, _ mods: NSEvent.ModifierFlags) -> Bool? {
        let ctrl = mods.contains(.control)
        if ctrl && mods.contains(.shift) && !mods.contains(.command) && !mods.contains(.option), panel.attachedSheet == nil {
            let step: CGFloat = 20
            switch code {
            case 4:
                var f = panel.frame
                f.size.width = max(120, f.width - step)
                panel.setFrame(clampToScreen(f), display: true)
                return true
            case 37:
                var f = panel.frame
                f.size.width = min(maxPanelWidth(), f.width + step)
                panel.setFrame(clampToScreen(f), display: true)
                return true
            case 40:
                if terminalShown && focusedPane == .terminal {
                    currentTerminalHeight = min(600, currentTerminalHeight + step)
                    preferredTerminalHeight = currentTerminalHeight
                } else if fileBrowserShown && focusedPane == .browser {
                    currentBrowserHeight = min(600, currentBrowserHeight + step)
                    preferredBrowserHeight = currentBrowserHeight
                } else {
                    var f = panel.frame
                    f.size.height = min(maxPanelHeight(), f.height + step)
                    panel.setFrame(clampToScreen(f), display: true)
                    return true
                }
                syncDrawerLayout()
                updateFocusIndicator()
                return true
            case 38:
                if terminalShown && focusedPane == .terminal {
                    currentTerminalHeight = max(minTerminalH, currentTerminalHeight - step)
                    preferredTerminalHeight = currentTerminalHeight
                } else if fileBrowserShown && focusedPane == .browser {
                    currentBrowserHeight = max(minBrowserH, currentBrowserHeight - step)
                    preferredBrowserHeight = currentBrowserHeight
                } else {
                    var f = panel.frame
                    f.size.height = max(140, f.height - step)
                    panel.setFrame(clampToScreen(f), display: true)
                    return true
                }
                syncDrawerLayout()
                updateFocusIndicator()
                return true
            default: break
            }
        }
        return nil
    }

    private func vimPaneKey(_ vv: LocalProcessTerminalView, _ code: UInt16,
                            _ mods: NSEvent.ModifierFlags) -> Bool {
        let cmd = mods.contains(.command)
        let ctrl = mods.contains(.control)
        switch code {
        case 8 where cmd || ctrl:
            if vimCopySelection(cut: false) { return true }
            if cmd, vv.selectedRange().length > 0 { vv.copy(self); return true }
            return cmd
        case 9 where cmd || ctrl:
            vimPaste(); return true
        case 7 where cmd:
            _ = vimCopySelection(cut: true); return true
        case 0 where cmd:
            vimRemote("<C-\\><C-N>ggVG"); return true
        case 6 where cmd:
            vimRemote("<C-\\><C-N>u"); return true
        case 1 where cmd:
            vimCommand("silent! wall")
            onEditorCommit?(currentEditorText)
            return true
        case 3 where cmd:
            vimRemote("<C-\\><C-N>/"); return true
        case 13 where cmd:
            if let onCloseWindow { onCloseWindow() } else { handleEscape() }
            return true
        case 31 where cmd:
            onOpenPathPrompt?(); return true
        default:
            return false
        }
    }

    private func terminalKey(_ term: LocalProcessTerminalView, _ code: UInt16,
                             _ mods: NSEvent.ModifierFlags) -> Bool {
        let cmd = mods.contains(.command)
        let ctrl = mods.contains(.control)
        switch code {
        case 8 where cmd: term.copy(self); return true
        case 9 where cmd || ctrl: term.paste(self); return true
        default: return false
        }
    }

    private func hostShortcutKey(_ code: UInt16, _ mods: NSEvent.ModifierFlags) -> Bool? {
        let cmd = mods.contains(.command)
        if cmd && code == 40, let hook = onCommandK,
           !(fileBrowser.map { browserActive() && browserHasFocus($0) } ?? false) {
            hook()
            return true
        }
        if cmd && code == 34, config.inspectorWidth > 0, !config.editMode {
            toggleInspector()
            return true
        }
        if cmd && code == 3, !config.editMode, let hook = onCommandF {
            hook()
            return true
        }
        if cmd && code == 37, let fb = fileBrowser, browserActive() {
            panel.makeFirstResponder(fb.searchView)
            fb.searchView.currentEditor()?.selectAll(nil)
            return true
        }
        return nil
    }

    private func fileBrowserKey(_ code: UInt16, _ mods: NSEvent.ModifierFlags) -> Bool? {
        let cmd = mods.contains(.command)
        let ctrl = mods.contains(.control)
        guard let fb = fileBrowser, browserActive(), browserHasFocus(fb) else { return nil }
        if let ed = fb.renameEditor {
            switch code {
            case 0: ed.selectAll(nil); return true
            case 8: ed.copy(nil); return true
            case 9: ed.paste(nil); return true
            case 7: ed.cut(nil); return true
            case 6: ed.undoManager?.undo(); return true
            default: return false
            }
        }
        if fb.handleShortcut(code, mods) { return true }
        switch code {
        case 15 where cmd:
            fb.beginRename()
            return true
        case 45 where ctrl, 35 where ctrl:
            fb.listView.moveSelection(code == 45 ? 1 : -1)
            return true
        case 40 where cmd:
            fb.copyRowPath(fb.listView.selection)
            return true
        case 0:
            fb.searchView.selectText(nil)
            return true
        case 8:
            if let ed = fb.searchView.currentEditor() {
                ed.copy(nil)
            } else {
                fb.copyRowPath(fb.listView.selection)
            }
            return true
        case 9:
            if let ed = fb.searchView.currentEditor() {
                ed.paste(nil)
            } else if panel.makeFirstResponder(fb.searchView),
                      let ed = fb.searchView.currentEditor() {
                ed.paste(nil)
            }
            return true
        case 7:
            fb.searchView.currentEditor()?.cut(nil)
            return true
        case 6:
            fb.searchView.currentEditor()?.undoManager?.undo()
            return true
        default:
            break
        }
        return nil
    }

    private func editKey(_ code: UInt16, _ mods: NSEvent.ModifierFlags) -> Bool? {
        switch code {
        case 0:
            if let ff = findField, !ff.isHidden, panel.firstResponder === ff || ff.currentEditor() != nil {
                ff.selectText(nil)
                ff.currentEditor()?.selectAll(nil)
            } else if let tv = editorView {
                tv.selectAll(nil)
            } else {
                field.selectText(nil)
            }
            return true
        case 8:
            if let ff = findField, !ff.isHidden, let ed = ff.currentEditor() {
                ed.copy(nil)
            } else if let tv = editorView {
                tv.copy(nil)
            } else if let ed = field.currentEditor() {
                ed.copy(nil)
            }
            return true
        case 9:
            if let ff = findField, !ff.isHidden, let ed = ff.currentEditor() {
                ed.paste(nil)
            } else if let tv = editorView {
                tv.paste(nil)
            } else if let ed = field.currentEditor() {
                ed.paste(nil)
            }
            return true
        case 7:
            if let ff = findField, !ff.isHidden, let ed = ff.currentEditor() {
                ed.cut(nil)
            } else if let tv = editorView {
                tv.cut(nil)
            } else if let ed = field.currentEditor() {
                ed.cut(nil)
            }
            return true
        case 6:
            if let ff = findField, !ff.isHidden, let ed = ff.currentEditor() {
                ed.undoManager?.undo()
            } else if let tv = editorView {
                tv.undoManager?.undo()
            } else if let ed = field.currentEditor() {
                ed.undoManager?.undo()
            }
            return true
        case 3:
            if config.editMode {
                toggleFindBar()
                return true
            }
            return false
        default:
            break
        }
        return nil
    }

    private func editorKey(_ code: UInt16, _ mods: NSEvent.ModifierFlags) -> Bool {
        if let term = terminalDrawer, terminalShown, terminalFocused(term) {
            if code == 53, escStreakCloses() {
                handleEscape()
                return true
            }
            return false
        }
        if let vv = focusedVim() {
            if code == 53, mods.intersection([.command, .control, .option]).isEmpty {
                if escStreakCloses(),
                   vimEval("mode()")?.trimmingCharacters(in: .whitespacesAndNewlines) == "n" {
                    handleEscape()
                    return true
                }
                vv.send(txt: "\u{1b}")
                return true
            }
            return false
        }
        if findBarShown {
            if code == 53 {
                closeFindBar()
                return true
            }
            if code == 36 {
                findStep(mods.contains(.shift) ? -1 : 1)
                return true
            }
            return false
        }
        if code == 53 {
            if escStreakCloses() { handleEscape() }
            return true
        }
        if code == 1, mods.contains(.command), editorView != nil {
            onEditorCommit?(currentEditorText)
            return true
        }
        if code == 31, mods.contains(.command) {
            onOpenPathPrompt?()
            return true
        }
        return false
    }

    private func listKey(_ code: UInt16, _ mods: NSEvent.ModifierFlags) -> Bool {
        if config.selectableRows, mods.contains(.control), code == 49 {
            toggleRowSelection(selection)
            return true
        }
        if config.enableNavigation {
            let ctrl = mods.contains(.control)
            switch (code, ctrl) {
            case (125, _): moveSelection(1); return true
            case (126, _): moveSelection(-1); return true
            case (48, _): moveSelection(mods.contains(.shift) ? -1 : 1); return true
            case (45, true): moveSelection(1); return true
            case (35, true): moveSelection(-1); return true
            case (36, _), (38, true): acceptSelection(); return true
            case (115, false), (119, false), (116, false), (121, false):
                guard !rows.isEmpty else { return true }
                let page = max(1, Int((rowScroll?.contentSize.height ?? 300) / max(1, config.rowHeight * zoom * textZoom)) - 1)
                let to = code == 115 ? 0 : code == 119 ? rows.count - 1
                    : selection + (code == 116 ? -page : page)
                selection = min(max(0, to), rows.count - 1)
                return true
            default: break
            }
        }
        if config.enableEscape && code == 53 {
            if escStreakCloses() { handleEscape() }
            return true
        }
        return false
    }

    private var actionPicker: NSView?
    private var actionItems: [(title: String, detail: String)] = []
    private var actionIndex = 0
    private var actionPick: ((Int) -> Void)?
    private var actionTitle = ""

    public func showActionPicker(title: String, items: [(title: String, detail: String)],
                                 onPick: @escaping (Int) -> Void) {
        guard !items.isEmpty else { return }
        actionTitle = title
        actionItems = items
        actionIndex = 0
        actionPick = onPick
        escStreak = 0
        renderActionPicker()
    }

    public func closeActionPicker() {
        actionPicker?.removeFromSuperview()
        actionPicker = nil
        actionPick = nil
        escStreak = 0
    }

    private func actionPickerKey(_ code: UInt16, _ mods: NSEvent.ModifierFlags) -> Bool {
        let ctrl = mods.contains(.control)
        switch (code, ctrl) {
        case (125, _), (45, true): actionIndex = (actionIndex + 1) % actionItems.count
        case (126, _), (35, true): actionIndex = (actionIndex - 1 + actionItems.count) % actionItems.count
        case (48, _): actionIndex = (actionIndex + (mods.contains(.shift) ? -1 : 1) + actionItems.count)
            % actionItems.count
        case (36, _), (76, _), (38, true):
            let pick = actionPick, i = actionIndex
            closeActionPicker()
            pick?(i)
            return true
        case (53, _):
            closeActionPicker()
            return true
        default:
            if let n = [18: 1, 19: 2, 20: 3, 21: 4, 23: 5, 22: 6, 26: 7, 28: 8, 25: 9][Int(code)],
               n <= actionItems.count {
                let pick = actionPick
                closeActionPicker()
                pick?(n - 1)
            }
            return true
        }
        renderActionPicker()
        return true
    }

    private final class PickerView: NSView {
        var rowRects: [NSRect] = []
        var onClick: ((Int) -> Void)?
        override var isFlipped: Bool { true }
        override func mouseDown(with e: NSEvent) {
            let p = convert(e.locationInWindow, from: nil)
            if let i = rowRects.firstIndex(where: { $0.contains(p) }) { onClick?(i) }
        }
    }

    private func renderActionPicker() {
        guard let root = panel.contentView else { return }
        actionPicker?.removeFromSuperview()
        let c = config.colors
        let z = zoom
        let rowH = 34 * z, pad = 8 * z, titleH = 26 * z
        let w = min(420 * z, root.bounds.width - 40)
        let h = titleH + CGFloat(actionItems.count) * rowH + pad * 2
        let v = PickerView(frame: NSRect(x: (root.bounds.width - w) / 2,
                                         y: root.isFlipped ? max(40, root.bounds.height * 0.28)
                                                           : root.bounds.height * 0.72 - h,
                                         width: w, height: h))
        v.autoresizingMask = [.minXMargin, .maxXMargin]
        v.wantsLayer = true
        let fill = (c.background.usingColorSpace(.sRGB) ?? c.background)
            .blended(withFraction: 0.25, of: .black) ?? c.background
        v.layer?.backgroundColor = fill.withAlphaComponent(0.97).cgColor
        v.layer?.cornerRadius = 10 * z
        v.layer?.borderColor = c.text.withAlphaComponent(0.15).cgColor
        v.layer?.borderWidth = 1
        v.layer?.shadowColor = NSColor.black.cgColor
        v.layer?.shadowOpacity = 0.35
        v.layer?.shadowRadius = 14
        let t = NSTextField(labelWithString: actionTitle + "   ↑↓ / ⌃N ⌃P · ↩ · esc")
        t.font = .systemFont(ofSize: 11 * z, weight: .medium)
        t.textColor = c.dim
        t.frame = NSRect(x: pad + 6 * z, y: pad, width: w - pad * 2, height: titleH - 6 * z)
        v.addSubview(t)
        var rects: [NSRect] = []
        for (i, item) in actionItems.enumerated() {
            let r = NSRect(x: pad, y: pad + titleH + CGFloat(i) * rowH, width: w - pad * 2, height: rowH)
            rects.append(r)
            if i == actionIndex {
                let hl = NSView(frame: r)
                hl.wantsLayer = true
                hl.layer?.backgroundColor = c.highlight.cgColor
                hl.layer?.cornerRadius = 6 * z
                v.addSubview(hl)
            }
            let l = NSTextField(labelWithString: "\(i + 1)  \(item.title)")
            l.font = .systemFont(ofSize: 13 * z, weight: .semibold)
            l.textColor = c.text
            l.frame = NSRect(x: r.minX + 10 * z, y: r.minY + 3 * z, width: r.width - 20 * z, height: 16 * z)
            v.addSubview(l)
            let d = NSTextField(labelWithString: item.detail)
            d.font = .systemFont(ofSize: 11 * z)
            d.textColor = c.dim
            d.lineBreakMode = .byTruncatingTail
            d.frame = NSRect(x: r.minX + 26 * z, y: r.minY + 18 * z, width: r.width - 36 * z, height: 14 * z)
            v.addSubview(d)
        }
        v.rowRects = rects
        v.onClick = { [weak self] i in
            guard let self else { return }
            let pick = self.actionPick
            self.closeActionPicker()
            pick?(i)
        }
        root.addSubview(v, positioned: .above, relativeTo: nil)
        actionPicker = v
    }

    public typealias ShortcutGroup = ShortcutRows
    public var onShowShortcuts: (() -> Void)?
    private var shortcutsSheet: ShortcutsOverlay?

    public func showShortcuts(_ groups: [ShortcutGroup]) {
        guard let root = panel.contentView, !groups.isEmpty else { return }
        closeShortcuts()
        closeActionPicker()
        let o = ShortcutsOverlay(groups: groups, colors: config.colors, zoom: zoom, in: root)
        o.onClose = { [weak self] in self?.closeShortcuts() }
        shortcutsSheet = o
        escStreak = 0
    }

    public func closeShortcuts() {
        shortcutsSheet?.removeFromSuperview()
        shortcutsSheet = nil
        escStreak = 0
    }

    private func shortcutsKey(_ code: UInt16, _ mods: NSEvent.ModifierFlags) -> Bool {
        shortcutsSheet?.handleKey(code, mods) ?? false
    }

    private weak var toastView: NSView?
    func toastCopiedPath(_ shown: String) {
        guard !config.copyToast.isEmpty else { return }
        showToast(config.copyToast.replacingOccurrences(of: "{}", with: shown), symbol: "checkmark",
                  centered: true, boxed: true, hold: 3.0)
    }
    func toastPasteboardPath() {
        guard let s = NSPasteboard.general.string(forType: .string), !s.isEmpty else { return }
        let n = s.split(separator: "\n").count
        toastCopiedPath(n > 1 ? "\(n) items" : (s as NSString).abbreviatingWithTildeInPath)
    }
    func showToast(_ text: String, symbol: String? = nil, centered: Bool = false,
                   boxed: Bool = false, hold: TimeInterval = 1.4) {
        guard let root = panel.contentView else { return }
        toastView?.removeFromSuperview()
        let pill = makeToastPill(text, symbol: symbol, colors: config.colors, zoom: zoom,
                                 maxWidth: root.bounds.width - 32, boxed: boxed)
        let h = pill.frame.height, w = pill.frame.width
        let inset = centered ? max(0, (root.bounds.height - h) / 2) : 30 * zoom
        let flipped = root.isFlipped
        pill.frame = NSRect(x: (root.bounds.width - w) / 2,
                            y: flipped ? root.bounds.height - h - inset : inset,
                            width: w, height: h)
        pill.autoresizingMask = [.minXMargin, .maxXMargin, flipped ? .minYMargin : .maxYMargin]
        root.addSubview(pill, positioned: .above, relativeTo: nil)
        toastView = pill
        animateToastPill(pill, rise: (flipped ? 6 : -6) * zoom, hold: hold, fade: boxed ? 0.7 : 0.25)
    }

    private func escStreakCloses() -> Bool {
        let now = Date()
        escStreak = now.timeIntervalSince(lastEsc) < 0.6 ? escStreak + 1 : 1
        lastEsc = now
        let n = config.escCloseCount
        guard n > 0, escStreak >= n else { return false }
        escStreak = 0
        return true
    }

    private func moveSelection(_ delta: Int) {
        guard config.enableNavigation, rows.count > 0 else { return }
        if config.wrapNavigation {
            selection = (selection + delta + rows.count) % rows.count
        } else {
            selection = min(max(0, selection + delta), rows.count - 1)
        }
    }

    private func resizeBy(_ delta: CGFloat) {
        rowView.stretchToFill = true
        var f = panel.frame
        let oldW = f.width
        f.size.width = max(120, f.width + delta)
        f.size.height = max(140, f.height + delta)
        panel.setFrame(clampToScreen(f), display: true)
        if oldW > 0 {
            zoom = min(3, max(0.6, zoom * panel.frame.width / oldW))
        }
        rowView.sizingRowCount = rows.count
        layoutScrollDocument()
        rowView.needsDisplay = true
    }

    private func acceptSelection() {
        guard !rows.isEmpty else {
            hide(restore: true)
            return
        }
        onAccept?(rows[selection])
    }

    private func handleEscape() {
        if let onEscape {
            onEscape()
        } else {
            hide(restore: true)
        }
    }

    public func controlTextDidBeginEditing(_ obj: Notification) {
        field.currentEditor()?.focusRingType = .none
        findField?.currentEditor()?.focusRingType = .none
    }

    public func controlTextDidChange(_ obj: Notification) {
        if let ff = findField, obj.object as AnyObject? === ff {
            applyFindQuery()
            return
        }
        guard config.enableSearch else { return }
        let q = field.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        rowView.highlightQuery = field.stringValue
        let newRows = onFilter?(q) ?? []
        setRows(newRows)
    }

    public func windowDidResignKey(_ notification: Notification) {
        editorFocusBorder?.isHidden = true
        browserFocusBorder?.isHidden = true
        terminalFocusBorder?.isHidden = true
        focusLossGen += 1
        guard isShown && !config.sticky && settings.hideOnFocusLoss && !hostHandlesFocusLoss else { return }
        let gen = focusLossGen
        DispatchQueue.main.asyncAfter(deadline: .now() + settings.focusLossDelay) { [weak self] in
            guard let self, gen == self.focusLossGen, self.isShown, !self.config.sticky,
                  settings.hideOnFocusLoss, !self.hostHandlesFocusLoss, !self.isShowingMenu,
                  focusLeft(self.panel) else { return }
            self.hide(restore: false)
        }
    }

    public func windowDidBecomeKey(_ notification: Notification) {
        focusLossGen += 1
    }

    public func windowDidResize(_ notification: Notification) {
        if isShown, !panel.inLiveResize {
            let clamped = clampToScreen(panel.frame)
            if clamped != panel.frame {
                panel.setFrame(clamped, display: true)
                return
            }
        }
        fitDrawersToWindow()
        drawerInsetNow = min(drawerInsetNow, drawerInsetTotal())
        rowView.sizingRowCount = rows.count
        layoutScrollDocument()
        relayoutTabs()
        layoutEditorScroll()
        layoutSearchField()
        layoutFindBar()
        layoutTerminal()
        layoutFileBrowser()
        updateFocusIndicator()
        rowView.needsDisplay = true
        chrome?.needsDisplay = true
    }

    private func fitDrawersToWindow() {
        guard config.editMode, let backdrop = panel.contentView else { return }
        let topY = config.headerHeight * zoom + editorTabStripHeight + findBarHeight()
        let meter = (chrome?.meterEnabled ?? false) ? chrome!.meterBarHeight + 4 : 0
        let status = !(statusBar?.isHidden ?? true) ? statusBarHeight + 4 : 0
        let avail = backdrop.bounds.height - topY - meter - status - 4
        var term = terminalShown ? preferredTerminalHeight : 0
        var browser = fileBrowserShown ? preferredBrowserHeight : 0
        var deficit = minEditorH - (avail - term - browser)
        if deficit > 0, terminalShown {
            let take = min(deficit, max(0, term - minTerminalH)); term -= take; deficit -= take
        }
        if deficit > 0, fileBrowserShown {
            let take = min(deficit, max(0, browser - minBrowserH)); browser -= take
        }
        if terminalShown { currentTerminalHeight = term }
        if fileBrowserShown { currentBrowserHeight = browser }
    }

    public func windowDidEndLiveResize(_ notification: Notification) {
        rowView.stretchToFill = true
        rowView.sizingRowCount = rows.count
        rowView.needsDisplay = true
    }

    private func scrollDocumentHeight() -> CGFloat {
        let content = rowView.contentHeight()
        guard rowView.stretchToFill, let scroll = rowScroll else { return content }
        return max(content, scroll.bounds.height) + rowView.bottomInset
    }

    private func layoutScrollDocument() {
        guard config.scrollableRows, let scroll = rowScroll else { return }
        let w = scroll.bounds.width > 0 ? scroll.bounds.width : config.width
        if let backdrop = panel.contentView {
            let cb = chromeBottom > 0 ? chromeBottom : 0
            let h = max(40, backdrop.bounds.height - cb)
            let x = listLeft, sw = max(80, backdrop.bounds.width - listLeft - listRight)
            if scroll.frame.origin.y != cb || abs(scroll.frame.height - h) > 0.5
                || scroll.frame.origin.x != x || abs(scroll.frame.width - sw) > 0.5 {
                scroll.frame = NSRect(x: x, y: cb, width: sw, height: h)
            }
            layoutListSidebar()
            layoutInspector()
            if sidebarTabs, let fb = filterBar {
                fb.frame.origin.x = listLeft
                fb.frame.size.width = sw
            }
            layoutListExtras()
        }
        rowView.frame.size.width = w
        if let header = tableHeader {
            header.zoom = zoom
            header.frame = NSRect(x: 0, y: 0, width: w, height: config.tableHeaderHeight * zoom)
            rowView.topInset = config.tableHeaderHeight * zoom + 2
            header.needsDisplay = true
        }
        rowView.frame.size.height = scrollDocumentHeight()
    }

    @discardableResult
    public func fitTableColumns(sample: Int = 400) -> [CGFloat]? {
        let cols = config.tableColumns
        guard !cols.isEmpty else { return nil }
        let font = config.rowFont(config.rowFontSize * zoom)
        let bold = NSFontManager.shared.convert(font, toHaveTrait: .boldFontMask)
        let hfont = NSFontManager.shared.convert(config.rowFont(config.rowFontSize * zoom * 0.86),
                                                 toHaveTrait: .boldFontMask)
        let rowsToMeasure = rows.filter { !$0.loadMore && !$0.groupHeader }.prefix(sample)
        var need: [CGFloat] = cols.enumerated().map { i, col in
            var w = (col.title.uppercased() + " ↑" as NSString)
                .size(withAttributes: [.font: hfont, .kern: 0.6]).width + 12
            if col.filterable { w += 22 }
            for r in rowsToMeasure {
                guard let t = r.cellText(col.field), !t.isEmpty else { continue }
                let flat = t.replacingOccurrences(of: "\n", with: " ")
                let f = i == 0 ? bold : font
                w = max(w, (flat as NSString).size(withAttributes: [.font: f]).width + 11 + 11 * zoom)
            }
            return ceil(w)
        }
        let screenW = (panel.screen ?? NSScreen.main)?.visibleFrame.width ?? 1400
        let lead = config.rowLeadInset, trail = config.padding + 10 + 16
        let cap = screenW - 40 - lead - trail
        need = need.map { min($0, max(160, cap * 0.45)) }
        var total = need.reduce(0, +)
        if total > cap {
            var over = total - cap
            while over > 0.5 {
                let maxW = need.max() ?? 0
                let idx = need.indices.filter { need[$0] >= maxW - 0.5 }
                let next = need.filter { $0 < maxW - 0.5 }.max() ?? 40
                let step = min(over / CGFloat(idx.count), maxW - max(next, 40))
                guard step > 0.5 else { break }
                for i in idx { need[i] -= step }
                over -= step * CGFloat(idx.count)
            }
            total = need.reduce(0, +)
        }
        let width = min(screenW - 40, lead + total + trail)
        var f = panel.frame
        if let vis = (panel.screen ?? NSScreen.main)?.visibleFrame {
            f.origin.x = max(vis.minX + 20, min(f.origin.x - (width - f.width) / 2, vis.maxX - width - 20))
        }
        f.size.width = width
        panel.setFrame(f, display: true, animate: false)
        layoutSearchField()
        layoutScrollDocument()
        relayoutTabs()
        let usable = max(1, rowView.bounds.width - lead - (config.padding + 10))
        let pcts = need.map { (($0 / usable * 100) * 10).rounded() / 10 }
        var newCols = cols
        for i in newCols.indices { newCols[i].width = pcts[i] }
        setTableColumns(newCols)
        return pcts
    }

    public func setTableColumns(_ cols: [PopupTableColumn]) {
        config.tableColumns = cols
        rowView.config.tableColumns = cols
        rowView.invalidateHeightCache()
        rowView.needsDisplay = true
        if let header = tableHeader {
            header.config.tableColumns = cols
            header.needsDisplay = true
            header.window?.invalidateCursorRects(for: header)
        }
    }

    private func relayoutTabs() {
        guard let bar = tabsBar else { return }
        let w = bar.bounds.width > 0 ? bar.bounds.width : config.width
        let h = bar.heightNeeded(forWidth: w)
        if abs(bar.frame.height - h) > 0.5 {
            let oldH = bar.frame.height
            bar.frame.size.height = h
            bar.needsDisplay = true
            if config.editMode {
                layoutEditorScroll()
            } else if config.scrollableRows, let scroll = rowScroll,
                      let backdrop = panel.contentView {
                let base = self.chromeBottom - (oldH + 2)
                let cb = base + h + 2
                self.chromeBottom = cb
                scroll.frame = NSRect(x: 0, y: cb, width: backdrop.bounds.width,
                                      height: max(40, backdrop.bounds.height - cb))
                layoutScrollDocument()
            }
        }
    }

    public func toggleTerminalDrawer() {
        setTerminalDrawer(!terminalShown)
    }

    public func setTerminalDrawer(_ show: Bool) {
        guard let drawer = terminalDrawer else { return }
        if show, let p = drawer.process, !p.running, !p.windingDown {
            drawer.startProcess(executable: config.shell, args: config.shellArgs)
        }
        if show != terminalShown {
            terminalShown = show
            if show { currentTerminalHeight = preferredTerminalHeight }
            syncDrawerLayout()
        }
        if terminalShown {
            panel.makeFirstResponder(drawer)
            focusedPane = .terminal
        } else if let ed = primaryEditor {
            panel.makeFirstResponder(ed)
            focusedPane = .editor
        }
        updateFocusIndicator()
    }

    private var primaryEditor: NSView? {
        if let vv = vimView, vimPaneActive { return vv }
        return editorView
    }
    private func focusedVim() -> LocalProcessTerminalView? {
        guard let vv = vimView, vimPaneActive else { return nil }
        let fr = panel.firstResponder
        if fr === vv { return vv }
        if let v = fr as? NSView, v.isDescendant(of: vv) { return vv }
        return nil
    }

    private func paneHoldsFocus() -> Bool {
        guard let v = panel.firstResponder as? NSView else { return false }
        if let vv = vimView, vimPaneActive, v === vv || v.isDescendant(of: vv) { return true }
        if let ed = editorView, !(editorScroll?.isHidden ?? true),
           v === ed || v.isDescendant(of: ed) { return true }
        if let term = terminalDrawer, terminalShown, v === term || v.isDescendant(of: term) { return true }
        if let fb = fileBrowser, browserActive(), v.isDescendant(of: fb) { return true }
        if let ff = findField, !ff.isHidden, v === ff || ff.currentEditor() === v { return true }
        return false
    }

    private func applyVimFont() {
        guard let vv = vimView else { return }
        vv.font = PopupWindow.vimFont(config)
        let size = vv.frame.size
        vv.setFrameSize(NSSize(width: size.width + 1, height: size.height))
        vv.setFrameSize(size)
    }

    static func vimFont(_ c: PopupConfig) -> NSFont {
        let size = c.editorFontSize * c.zoom
        if let n = c.fontName {
            for name in [n, n + " Mono"] {
                if let f = NSFont(name: name, size: size), f.isFixedPitch { return f }
            }
        }
        if let f = NSFont(name: c.terminalFont, size: size) { return f }
        return NSFont.monospacedSystemFont(ofSize: size, weight: .regular)
    }

    static func vimEnvironment() -> [String] {
        var env = ProcessInfo.processInfo.environment
        let need = ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin",
                    "/usr/sbin", "/sbin"]
        var path = (env["PATH"] ?? "").split(separator: ":").map(String.init)
        for d in need where !path.contains(d) { path.append(d) }
        env["PATH"] = path.joined(separator: ":")
        env["TERM"] = "xterm-256color"
        env["COLORTERM"] = "truecolor"
        if (env["LANG"] ?? "").isEmpty { env["LANG"] = "en_US.UTF-8" }
        env.removeValue(forKey: "NVIM")
        env.removeValue(forKey: "NVIM_LISTEN_ADDRESS")
        return env.map { "\($0.key)=\($0.value)" }
    }

    public var vimRunning: Bool { vimView?.process?.running ?? false }

    public func setVimPaneActive(_ active: Bool) {
        guard let vv = vimView else { return }
        vimPaneActive = active
        vv.isHidden = !active
        vimImageOverlay?.isHidden = !active
        editorScroll?.isHidden = active
        if isShown, panel.firstResponder === vv || !active {
            if let ed = primaryEditor { panel.makeFirstResponder(ed) }
        }
        updateFocusedPane()
    }

    func startVimIfNeeded() {
        guard let vv = vimView, let exec = config.vimEditorExecutable,
              !vimShuttingDown else { return }
        if let p = vv.process, p.running { return }
        if let sock = config.vimEditorSocket {
            try? FileManager.default.removeItem(atPath: sock)
            try? FileManager.default.createDirectory(
                atPath: (sock as NSString).deletingLastPathComponent,
                withIntermediateDirectories: true)
        }
        startVimImageWatch()
        var args = vimLaunchArgs?() ?? config.vimEditorArgs
        if config.vimImageFile != nil {
            let ch = Int(PopupWindow.cellSize(PopupWindow.vimFont(config)).height)
            args.insert(contentsOf: ["--cmd", "let g:ws_cell_h=\(ch)"], at: 0)
        }
        vv.startProcess(executable: exec, args: args,
                        environment: PopupWindow.vimEnvironment(),
                        currentDirectory: config.terminalDir)
    }

    public func shutdownVim() {
        guard vimView != nil else { return }
        vimShuttingDown = true
        if vimEval("execute('silent! wall')") == nil {
            vimRemote("<C-\\><C-N>:silent! wall<CR>")
        }
        vimRemote("<C-\\><C-N>:qa!<CR>")
    }

    private var vimRPC: NvimRPC?
    @discardableResult
    private func vimClient(_ flag: String, _ arg: String) -> String? {
        guard config.vimEditorExecutable != nil,
              let sock = config.vimEditorSocket,
              vimRunning else { return nil }
        if vimRPC?.path != sock { vimRPC = NvimRPC(path: sock) }
        guard let rpc = vimRPC else { return nil }
        return flag == "--remote-send" ? (rpc.input(arg) ? "" : nil) : rpc.eval(arg)
    }

    public func vimRemote(_ keys: String) {
        if vimClient("--remote-send", keys) != nil { return }
        guard let vv = vimView, vimRunning else { return }
        let raw = keys
            .replacingOccurrences(of: "<C-\\><C-N>", with: "\u{1c}\u{0e}")
            .replacingOccurrences(of: "<CR>", with: "\r")
            .replacingOccurrences(of: "<Esc>", with: "\u{1b}")
        vv.send(txt: raw)
    }

    public func vimEval(_ expr: String) -> String? {
        vimClient("--remote-expr", expr)
    }

    public func vimCommand(_ ex: String) {
        let quoted = "'" + ex.replacingOccurrences(of: "'", with: "''") + "'"
        if vimEval("execute(\(quoted))") != nil { return }
        vimRemote("<C-\\><C-N>:\(ex) | echo ''<CR>")
    }

    public static func vimString(_ s: String) -> String {
        "'" + s.replacingOccurrences(of: "'", with: "''") + "'"
    }

    public func vimFlush() {
        vimCommand("silent! wall")
    }

    public func vimOpen(_ path: String) {
        let lit = PopupWindow.vimString(path)
        let ex = "silent! wall | stopinsert | execute 'edit ' .. fnameescape(\(lit)) | redraw!"
        if vimEval("execute(\(PopupWindow.vimString(ex)))") != nil { return }
        let esc = path.replacingOccurrences(of: " ", with: "\\ ")
        vimRemote("<C-\\><C-N>:silent! wall | edit \(esc)<CR>")
    }

    @discardableResult
    public func vimAppend(_ text: String, to path: String) -> Bool {
        let lines = text.components(separatedBy: "\n").map { PopupWindow.vimString($0) }
        let list = "[" + lines.joined(separator: ",") + "]"
        let buf = "bufnr(\(PopupWindow.vimString(path)))"
        let expr = "\(buf) > 0 ? [appendbufline(\(buf), '$', \(list)), execute('silent! wall')][0] : -1"
        guard let r = vimEval(expr) else { return false }
        return r.trimmingCharacters(in: .whitespacesAndNewlines) == "0"
    }

    private static func vimLua(_ lines: [String]) -> String {
        lines.map { $0.trimmingCharacters(in: .whitespaces) }.joined(separator: " ")
    }
    private static func vimDQ(_ s: String) -> String {
        "\"" + s.replacingOccurrences(of: "\\", with: "\\\\")
            .replacingOccurrences(of: "\"", with: "\\\"")
            .replacingOccurrences(of: "\n", with: "\\n") + "\""
    }
    @discardableResult
    public func vimVoiceBegin() -> Bool {
        let lua = PopupWindow.vimLua([
            "(function()",
            "local ns = vim.api.nvim_create_namespace('ws_voice')",
            "local buf = vim.api.nvim_get_current_buf()",
            "local pos = vim.api.nvim_win_get_cursor(0)",
            "local row, col = pos[1] - 1, pos[2]",
            "local line = vim.api.nvim_buf_get_lines(buf, row, row + 1, false)[1] or ''",
            "if vim.api.nvim_get_mode().mode:sub(1, 1) ~= 'i' and #line > 0 then",
            "col = col + #vim.fn.matchstr(line, '.', col) end",
            "vim.api.nvim_buf_clear_namespace(buf, ns, 0, -1)",
            "local s = vim.api.nvim_buf_set_extmark(buf, ns, row, col, {right_gravity = false})",
            "local e = vim.api.nvim_buf_set_extmark(buf, ns, row, col, {right_gravity = true})",
            "local pre = line:sub(1, col):match('%S$') and ' ' or ''",
            "local post = line:sub(col + 1):match('^%S') and ' ' or ''",
            "vim.g.ws_voice = {buf = buf, s = s, e = e, pre = pre, post = post}",
            "return 1 end)()",
        ])
        return vimEval("luaeval(\(PopupWindow.vimString(lua)))")?
            .trimmingCharacters(in: .whitespacesAndNewlines) == "1"
    }
    @discardableResult
    public func vimVoiceUpdate(_ text: String) -> Bool {
        let lua = PopupWindow.vimLua([
            "(function(t)",
            "local v = vim.g.ws_voice",
            "if not v or not vim.api.nvim_buf_is_loaded(v.buf) then return 0 end",
            "local ns = vim.api.nvim_create_namespace('ws_voice')",
            "local sp = vim.api.nvim_buf_get_extmark_by_id(v.buf, ns, v.s, {})",
            "local ep = vim.api.nvim_buf_get_extmark_by_id(v.buf, ns, v.e, {})",
            "if #sp == 0 or #ep == 0 then return 0 end",
            "local body = t == '' and '' or (v.pre .. t .. v.post)",
            "local ok = pcall(vim.api.nvim_buf_set_text, v.buf, sp[1], sp[2], ep[1], ep[2],",
            "vim.split(body, '\\n', {plain = true}))",
            "if not ok then return 0 end",
            "if t ~= '' and vim.api.nvim_get_current_buf() == v.buf then",
            "local e2 = vim.api.nvim_buf_get_extmark_by_id(v.buf, ns, v.e, {})",
            "local c = e2[2] - #v.post",
            "if vim.api.nvim_get_mode().mode:sub(1, 1) ~= 'i' then c = math.max(0, c - 1) end",
            "pcall(vim.api.nvim_win_set_cursor, 0, {e2[1] + 1, c}) end",
            "return 1 end)(_A)",
        ])
        return vimEval("luaeval(\(PopupWindow.vimString(lua)), \(PopupWindow.vimDQ(text)))")?
            .trimmingCharacters(in: .whitespacesAndNewlines) == "1"
    }
    public func vimVoiceEnd() {
        let lua = PopupWindow.vimLua([
            "(function()",
            "local v = vim.g.ws_voice",
            "vim.g.ws_voice = nil",
            "if v and vim.api.nvim_buf_is_valid(v.buf) then",
            "vim.api.nvim_buf_clear_namespace(v.buf, vim.api.nvim_create_namespace('ws_voice'), 0, -1) end",
            "vim.cmd('silent! wall')",
            "return 1 end)()",
        ])
        _ = vimEval("luaeval(\(PopupWindow.vimString(lua)))")
    }

    private func vimCopySelection(cut: Bool) -> Bool {
        guard let m = vimEval("mode()")?.trimmingCharacters(in: .whitespacesAndNewlines),
              ["v", "V", "\u{16}"].contains(m) else { return false }
        vimRemote(cut ? "\"+d" : "\"+y")
        return true
    }

    fileprivate func imageMenu(_ path: String) -> NSMenu {
        let m = NSMenu()
        m.autoenablesItems = false
        let url = URL(fileURLWithPath: path)
        m.addItem(menuItem("Open Full Size") { [weak self] in
            FilePopup.show(path: path, over: self?.panel)
        })
        m.addItem(.separator())
        m.addItem(menuItem("Copy Image Path") { [weak self] in
            let pb = NSPasteboard.general
            pb.clearContents()
            pb.setString(path, forType: .string)
            self?.toastCopiedPath((path as NSString).abbreviatingWithTildeInPath)
        })
        m.addItem(menuItem("Copy Image") { [weak self] in
            guard let img = NSImage(contentsOf: url) else { return }
            let pb = NSPasteboard.general
            pb.clearContents()
            pb.writeObjects([img])
            self?.showToast("Copied image", symbol: "photo")
        })
        m.addItem(menuItem("Reveal in Finder") {
            NSWorkspace.shared.activateFileViewerSelecting([url])
        })
        m.addItem(menuItem("Open in Default App") { NSWorkspace.shared.open(url) })
        if onCopyFilePath != nil {
            m.addItem(.separator())
            m.addItem(menuItem("Copy File Path") { [weak self] in self?.onCopyFilePath?() })
        }
        return m
    }

    public var vimMenu: NSMenu? {
        get { vimView?.menu }
        set { vimView?.menu = newValue }
    }

    public func vimCopy() {
        if vimCopySelection(cut: false) { return }
        if let vv = vimView, vv.selectedRange().length > 0 { vv.copy(self) }
    }

    public func vimPaste() {
        guard let vv = vimView else { return }
        var clip = NSPasteboard.general.string(forType: .string)
        if let img = PopupTextView.image(from: .general), let saver = imageSaver,
           let rel = saver(img) {
            clip = "![](\(rel))"
        }
        guard let text = clip, !text.isEmpty else { return }
        let lines = text.components(separatedBy: "\n")
            .map { PopupWindow.vimString($0.replacingOccurrences(of: "\r", with: "")) }
        let expr = "nvim_paste(join([\(lines.joined(separator: ","))], \"\\n\"), v:true, -1)"
        if vimEval(expr) != nil { return }
        vv.send(txt: "\u{1b}[200~" + text + "\u{1b}[201~")
    }

    static func cellSize(_ f: NSFont) -> NSSize {
        let h = ceil(CTFontGetAscent(f) + CTFontGetDescent(f) + CTFontGetLeading(f))
        var glyph = CTFontGetGlyphWithName(f, "W" as CFString)
        var adv = CGSize.zero
        CTFontGetAdvancesForGlyphs(f, .horizontal, &glyph, &adv, 1)
        return NSSize(width: adv.width, height: h)
    }

    private func startVimImageWatch() {
        guard vimImageWatch == nil, let path = config.vimImageFile else { return }
        if !FileManager.default.fileExists(atPath: path) {
            FileManager.default.createFile(atPath: path, contents: Data("{}".utf8))
        }
        let fd = open(path, O_EVTONLY)
        guard fd >= 0 else { return }
        let src = DispatchSource.makeFileSystemObjectSource(
            fileDescriptor: fd, eventMask: [.write, .extend, .delete, .rename], queue: .main)
        src.setEventHandler { [weak self] in
            guard let self else { return }
            if src.data.contains(.delete) || src.data.contains(.rename) {
                src.cancel()
                self.vimImageWatch = nil
                self.startVimImageWatch()
            }
            self.reloadVimImages()
        }
        src.setCancelHandler { close(fd) }
        src.resume()
        vimImageWatch = src
    }

    private func refreshVimImageRows() {
        guard config.vimImageFile != nil, vimRunning else { return }
        let ch = Int(PopupWindow.cellSize(PopupWindow.vimFont(config)).height)
        vimCommand("let g:ws_cell_h=\(ch) | lua _G.ws_images_refresh()")
    }

    private func reloadVimImages() {
        guard let path = config.vimImageFile, let ov = vimImageOverlay,
              let data = FileManager.default.contents(atPath: path),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else { return }
        let lines = obj["lines"] as? Int ?? 0
        let items = (obj["images"] as? [[String: Any]] ?? []).compactMap { d -> VimImageOverlay.Item? in
            guard let p = d["path"] as? String, let row = d["row"] as? Int,
                  let rows = d["rows"] as? Int else { return nil }
            return VimImageOverlay.Item(path: p, row: row, rows: rows)
        }
        ov.cell = PopupWindow.cellSize(PopupWindow.vimFont(config))
        ov.textRows = max(0, lines - 1)
        ov.items = items
    }

    public func applyFonts(editor: String?? = nil, editorSize: CGFloat? = nil,
                           terminal: String? = nil, terminalSize: CGFloat? = nil) {
        if let e = editor { config.fontName = e }
        if let s = editorSize { config.editorFontSize = s }
        if let t = terminal { config.terminalFont = t }
        if let s = terminalSize { config.terminalFontSize = s }
        if let tv = editorView {
            tv.font = editorFont(config.fontName, zoom, size: config.editorFontSize)
        }
        if let term = terminalDrawer,
           let f = NSFont(name: config.terminalFont, size: config.terminalFontSize) {
            term.font = f
        }
        applyVimFont()
        vimImageOverlay?.cell = PopupWindow.cellSize(PopupWindow.vimFont(config))
        refreshVimImageRows()
        layoutForZoom()
    }

    public enum ThemeRole: String, CaseIterable {
    case browser
    case terminal
    case notepad
    case header

    public var label: String {
        switch self {
        case .browser: return "File explorer"
        case .terminal: return "Terminal"
        case .notepad: return "Notepad"
        case .header: return "Header"
        }
    }
}

    public func themeColor(_ role: ThemeRole) -> NSColor {
        switch role {
        case .browser: return config.fileBrowserBackground
        case .terminal: return config.terminalBackground
        case .notepad: return config.colors.background.withAlphaComponent(config.tintAlpha)
        case .header: return config.headerColor ?? config.colors.background
        }
    }

    public func setThemeColor(_ c: NSColor, for role: ThemeRole) {
        switch role {
        case .browser:
            config.fileBrowserBackground = c
            fileBrowser?.setBackground(c)
        case .terminal:
            config.terminalBackground = c
            if let term = terminalDrawer {
                term.nativeBackgroundColor = c
                term.nativeForegroundColor = config.terminalForeground ?? config.colors.text
            }
        case .notepad:
            let cc = c.usingColorSpace(.sRGB) ?? c
            config.colors.background = cc.withAlphaComponent(1)
            config.tintAlpha = cc.alphaComponent
            tintView?.layer?.backgroundColor =
                config.colors.background.withAlphaComponent(config.tintAlpha).cgColor
            if config.opaqueTabs { tabsBar?.fill = config.colors.background }
            chrome?.headerColorOverride = config.headerColor ?? config.colors.background
            applyThemeAppearance()
            pushColors()
        case .header:
            config.headerColor = c
            chrome?.headerColorOverride = c
        }
        panel.contentView?.needsDisplay = true
    }

    public func setTextColors(text: NSColor, dim: NSColor, highlight: NSColor,
                              accent: NSColor? = nil, palette: PopupPalette? = nil,
                              border: NSColor? = nil) {
        config.colors.text = text
        config.colors.dim = dim
        config.colors.highlight = highlight
        if let accent { config.colors.accent = accent }
        if let palette { config.colors.palette = palette }
        if let border { config.colors.border = border }
        if let tv = editorView {
            tv.textColor = text
            tv.insertionPointColor = text
            tv.selectedTextAttributes = ButtonStyle.selection(config.colors)
        }
        applyThemeAppearance()
        findField?.textColor = text
        findCountLabel?.textColor = dim
        if let vv = vimView {
            vv.nativeForegroundColor = text
            func hex(_ c: NSColor) -> String {
                let cc = c.usingColorSpace(.sRGB) ?? c
                return String(format: "#%02X%02X%02X", Int(round(cc.redComponent * 255)),
                              Int(round(cc.greenComponent * 255)), Int(round(cc.blueComponent * 255)))
            }
            let lets = (["let g:ws_fg='\(hex(text))'", "let g:ws_dim='\(hex(dim))'",
                         "let g:ws_sel='\(hex(highlight))'"] + PopupWindow.vimPaletteLets(config.colors))
                .joined(separator: " | ")
            vimCommand(lets + " | silent! doautocmd ColorScheme")
        }
        if config.terminalForeground == nil {
            terminalDrawer?.nativeForegroundColor = text
        }
        applyTerminalPalette()
        pushColors()
    }

    func pushColors() {
        let c = config.colors
        func walk(_ v: NSView) {
            (v as? PopupThemeable)?.applyColors(c)
            v.subviews.forEach(walk)
        }
        if let root = panel.contentView {
            walk(root)
            root.layer?.borderColor = c.border.cgColor
        }
        for b in [editorFocusBorder, browserFocusBorder, terminalFocusBorder] {
            b?.layer?.borderColor = ButtonStyle.focusStroke(config.colors).cgColor
        }
        for f in [field, findField].compactMap({ $0 }) where (f.layer?.borderWidth ?? 0) > 0 {
            f.layer?.backgroundColor = ButtonStyle.inputFill(c).cgColor
            f.layer?.borderColor = f === findField
                ? ButtonStyle.focusStroke(c).withAlphaComponent(0.6).cgColor
                : ButtonStyle.inputStroke(c).cgColor
        }
        panel.contentView?.needsDisplay = true
    }

    func applyThemeAppearance() {
        let light = ButtonStyle.luminance(config.colors.background) > 0.45
        panel.appearance = NSAppearance(named: light ? .aqua : .darkAqua)
        let sel = ButtonStyle.selection(config.colors)
        (panel as? PopupBaseWindow)?.selectionAttributes = sel
        (panel as? PopupPanel)?.selectionAttributes = sel
        if let ed = panel.fieldEditor(false, for: nil) as? NSTextView { ed.selectedTextAttributes = sel }
        for term in [terminalDrawer, vimView].compactMap({ $0 }) {
            term.selectedTextBackgroundColor = sel[.backgroundColor] as? NSColor ?? config.colors.highlight
            term.selectedTextForegroundColor = sel[.foregroundColor] as? NSColor ?? config.colors.text
        }
    }

    public var onOpenExternalTerminal: ((String) -> Void)?

    public func openTerminalHere(_ dir: String) {
        guard let term = terminalDrawer else {
            onOpenExternalTerminal?(dir)
            return
        }
        if !terminalShown { toggleTerminalDrawer() }
        let quoted = "'" + dir.replacingOccurrences(of: "'", with: "'\\''") + "'"
        term.send(txt: "\u{15}cd -- \(quoted)\r")
        panel.makeFirstResponder(term)
        focusedPane = .terminal
        updateFocusIndicator()
    }

    public func setFloating(_ on: Bool) {
        config.floating = on
        panel.level = on ? .floating : .normal
    }

    public static func vimPaletteLets(_ c: PopupColors) -> [String] {
        func hex(_ x: NSColor) -> String {
            let s = ButtonStyle.opaque(x)
            return String(format: "#%02X%02X%02X", Int(round(s.redComponent * 255)),
                          Int(round(s.greenComponent * 255)), Int(round(s.blueComponent * 255)))
        }
        return [("accent", c.tone(.accent)), ("accent2", c.tone(.accent2)), ("ok", c.tone(.success)),
                ("warn", c.tone(.warning)), ("err", c.tone(.danger)), ("info", c.tone(.info)),
                ("deep", c.crust)].map { "let g:ws_\($0.0)='\(hex($0.1))'" }
    }

    func applyTerminalPalette() {
        guard let term = terminalDrawer else { return }
        term.installColors(PopupWindow.ansiPalette(config.colors))
    }
    static func ansiPalette(_ c: PopupColors) -> [SwiftTerm.Color] {
        let p = c.palette
        func hsb(_ x: NSColor) -> (CGFloat, CGFloat, CGFloat) {
            let s = ButtonStyle.opaque(x)
            return (s.hueComponent * 360, s.saturationComponent, s.brightnessComponent)
        }
        func dist(_ a: CGFloat, _ b: CGFloat) -> CGFloat { let d = abs(a - b); return min(d, 360 - d) }
        func pick(_ target: CGFloat, _ from: [NSColor], shift base: NSColor) -> NSColor {
            let best = from.min { dist(hsb($0).0, target) < dist(hsb($1).0, target) } ?? base
            if dist(hsb(best).0, target) <= 50 { return best }
            let (_, sat, bri) = hsb(base)
            return NSColor(hue: target / 360, saturation: max(sat, 0.35), brightness: bri, alpha: 1)
        }
        let blue = pick(220, [c.accent, p.accent2, p.info], shift: c.accent)
        let magenta = pick(300, [c.accent, p.accent2].filter { $0 != blue }, shift: p.accent2)
        let black = c.isLight ? c.dim.blended(withFraction: 0.35, of: c.text) ?? c.dim : c.surface1
        let white = c.isLight ? c.surface1 : c.dim
        let normal = [black, p.danger, p.success, p.warning, blue, magenta, p.info, white]
        let bright = normal.enumerated().map { i, x -> NSColor in
            i == 0 ? (c.isLight ? c.dim : c.highlight.blended(withFraction: 0.25, of: c.text) ?? c.highlight)
                : i == 7 ? c.text
                : ButtonStyle.opaque(x).blended(withFraction: 0.15, of: c.isLight ? .black : .white) ?? x
        }
        return (normal + bright).map { x in
            let s = ButtonStyle.opaque(x)
            return SwiftTerm.Color(red: UInt16(s.redComponent * 65535), green: UInt16(s.greenComponent * 65535),
                                   blue: UInt16(s.blueComponent * 65535))
        }
    }

    public func setTerminalForeground(_ c: NSColor?) {
        config.terminalForeground = c
        terminalDrawer?.nativeForegroundColor = c ?? config.colors.text
    }

    public var hasTerminalDrawer: Bool { terminalDrawer != nil }
    public var hasFileBrowser: Bool { fileBrowser != nil }

    public var testState: [String: Any] {
        let f = panel.frame
        return [
            "name": config.name, "shown": isShown, "key": panel.isKeyWindow,
            "frame": [f.origin.x, f.origin.y, f.width, f.height].map { Int($0.rounded()) },
            "drawerInset": Int(drawerInsetNow.rounded()),
            "terminal": terminalDrawer != nil && terminalShown,
            "browser": fileBrowser != nil && (!fileBrowserDrawerMode || fileBrowserShown),
            "findBar": findBarShown, "accessory": topAccessory != nil,
            "pane": focusedPane.map { "\($0)" } ?? "",
            "tabs": tabTitles, "selectedTab": selectedTab,
            "selection": selection, "rowCount": rows.count, "query": field.stringValue,
            "board": testExtra?() ?? [:],
            "sidebarCursor": tabsBar?.vertical == true ? tabsBar!.cursor : -1,
            "responder": panel.firstResponder.map { String(describing: type(of: $0)) } ?? "",
            "header": headerTestRects,
        ]
    }

    private var headerTestRects: [String: [Int]] {
        guard let chrome, chrome.dragHeaderHeight > 0 else { return [:] }
        let f = panel.frame
        func screen(_ r: NSRect) -> [Int] {
            [f.minX + r.minX, f.maxY - r.maxY, r.width, r.height].map { Int($0.rounded()) }
        }
        var out: [String: [Int]] = [:]
        if chrome.config.headerCloseButton { out["close"] = screen(chrome.closeButtonRect) }
        for (id, r) in chrome.extraButtonRects { out[String(id)] = screen(r) }
        return out
    }

    public func headerButtonRect(_ id: Int) -> NSRect? {
        chrome?.extraButtonRects[id]
    }

    func installFileBrowser(_ fb: PopupFileBrowser, drawer: Bool) {
        fileBrowser = fb
        fileBrowserDrawerMode = drawer
        fb.onCopied = { [weak self] shown in
            self?.toastCopiedPath(shown)
        }
        guard let backdrop = panel.contentView else { return }
        fb.autoresizingMask = drawer ? [.width, .minYMargin] : [.width, .height]
        currentBrowserHeight = preferredBrowserHeight
        fb.onOpenInNotes = { [weak self] p in
            self?.onFileBrowserOpenInNotes?(p)
        }
        fb.onOpenTerminal = { [weak self] dir in
            self?.openTerminalHere(dir)
        }
        if let chrome = self.chrome {
            backdrop.addSubview(fb, positioned: .below, relativeTo: chrome)
        } else {
            backdrop.addSubview(fb)
        }
        if drawer && config.fileBrowserDefault {
            fileBrowserShown = true
            terminalShown = false
            drawerInsetNow = config.fileBrowserHeight
        }
        fb.isHidden = drawer ? !fileBrowserShown : false
        let bfb = NSView(frame: fb.frame)
        bfb.autoresizingMask = fb.autoresizingMask
        bfb.wantsLayer = true
        bfb.layer?.borderWidth = focusBorderWidth
        bfb.layer?.borderColor = ButtonStyle.focusStroke(config.colors).cgColor
        bfb.layer?.cornerRadius = 6
        bfb.isHidden = true
        backdrop.addSubview(bfb)
        browserFocusBorder = bfb
        layoutFileBrowser()
        layoutTerminal()
        layoutEditorScroll()
    }

    private func drawerInsetTotal() -> CGFloat {
        (terminalShown ? currentTerminalHeight : 0)
            + (fileBrowserShown ? currentBrowserHeight : 0)
    }
    public var baseFrame: NSRect {
        var f = panel.frame
        f.origin.y += drawerShift
        f.size.height = max(minEditorH, f.height - drawerInsetNow)
        return f
    }

    private var drawerShiftValue: CGFloat = 0
    private var drawerShiftFrame: NSRect = .zero
    private var drawerShift: CGFloat { panel.frame == drawerShiftFrame ? drawerShiftValue : 0 }
    private func noteDrawerFrame(baseY: CGFloat) {
        drawerShiftFrame = panel.frame
        drawerShiftValue = baseY - drawerShiftFrame.origin.y
    }

    private func setBaseFrame(_ f: NSRect) {
        let want = config.editMode ? drawerInsetTotal() : 0
        let grown = clampToScreen(NSRect(x: f.minX, y: f.minY, width: f.width,
                                         height: f.height + want))
        drawerInsetNow = 0
        panel.setFrame(grown, display: false)
        drawerInsetNow = max(0, min(want, grown.height - f.height))
        noteDrawerFrame(baseY: f.minY)
        panel.invalidateShadow()
    }

    private func syncDrawerLayout() {
        let want = drawerInsetTotal()
        if abs(want - drawerInsetNow) > 0.5 {
            let f = panel.frame, was = drawerInsetNow
            let delta = want - was, shift = drawerShift
            var y = f.origin.y
            if delta < 0 { y += shift > 0 ? min(shift, -delta) : max(shift, delta) }
            panel.setFrame(NSRect(x: f.origin.x, y: y, width: f.width, height: f.height + delta),
                           display: true)
            drawerInsetNow = max(0, was + panel.frame.height - f.height)
            noteDrawerFrame(baseY: f.origin.y + shift)
            panel.invalidateShadow()
        }
        layoutTerminal()
        layoutFileBrowser()
        layoutEditorScroll()
        updateFocusIndicator()
    }

    private func layoutFileBrowser() {
        guard let fb = fileBrowser, let backdrop = panel.contentView else { return }
        if fileBrowserDrawerMode {
            let meter = (chrome?.meterEnabled ?? false) ? chrome!.meterBarHeight : 0
            let h = fileBrowserShown ? currentBrowserHeight : 0
            let termH = terminalShown ? currentTerminalHeight : 0
            let y = max(0, backdrop.bounds.height - meter - termH - h)
            fb.frame = NSRect(x: terminalInset, y: y,
                              width: max(0, backdrop.bounds.width - 2 * terminalInset),
                              height: h)
        } else {
            let topY = config.headerHeight * zoom
                + (sidebarTabs ? 2 : (tabsBar?.frame.height ?? 0) + 2)
            fb.frame = NSRect(x: 0, y: topY, width: backdrop.bounds.width,
                              height: max(40, backdrop.bounds.height - topY))
        }
        fb.needsLayout = true
        fb.layoutSubtreeIfNeeded()
    }

    private func updateFocusIndicator() {
        fileBrowser?.updatePartFocus()
        editorFocusBorder?.isHidden = true
        browserFocusBorder?.isHidden = true
        terminalFocusBorder?.isHidden = true
        PaneNav.shared.refreshSoon(panel)
    }

    private func terminalFocused(_ term: LocalProcessTerminalView) -> Bool {
        let fr = panel.firstResponder
        if fr === term { return true }
        if let v = fr as? NSView { return v.isDescendant(of: term) }
        return false
    }

    private func focusedTerm() -> LocalProcessTerminalView? {
        guard let term = terminalDrawer, terminalShown, terminalFocused(term) else { return nil }
        return term
    }

    private func browserHasFocus(_ fb: PopupFileBrowser) -> Bool {
        if fb.searchView.currentEditor() != nil { return true }
        let fr = panel.firstResponder
        if let v = fr as? NSView { return v.isDescendant(of: fb) }
        return false
    }
    private func browserActive() -> Bool {
        guard let fb = fileBrowser else { return false }
        _ = fb
        return fileBrowserDrawerMode ? fileBrowserShown : true
    }

    private var findBarShown: Bool {
        guard let ff = findField else { return false }
        return !ff.isHidden
    }

    public func toggleFindBar() {
        if findBarShown { closeFindBar() } else { showFindBar() }
    }

    public func showFindBar() {
        guard let ff = findField, ff.isHidden else { return }
        ff.isHidden = false
        findCountLabel?.isHidden = false
        layoutFindBar()
        layoutEditorScroll()
        panel.makeFirstResponder(ff)
        if let tv = editorView, tv.selectedRange().length > 0 {
            let sel = (tv.string as NSString).substring(with: tv.selectedRange())
            ff.stringValue = sel
            ff.currentEditor()?.selectedRange =
                NSRange(location: 0, length: (sel as NSString).length)
        }
        applyFindQuery()
    }

    public func closeFindBar() {
        guard let ff = findField, !ff.isHidden else { return }
        ff.isHidden = true
        findCountLabel?.isHidden = true
        findMatches = []
        findIndex = 0
        layoutFindBar()
        layoutEditorScroll()
        if let tv = editorView { tv.window?.makeFirstResponder(tv) }
    }

    private func applyFindQuery() {
        guard let ff = findField, let tv = editorView else { return }
        let q = ff.stringValue
        let text = tv.string
        var ranges: [NSRange] = []
        if !q.isEmpty {
            let ns = text as NSString
            var loc = 0
            while loc < ns.length {
                let r = ns.range(of: q, options: .caseInsensitive,
                                 range: NSRange(location: loc, length: ns.length - loc))
                if r.location == NSNotFound { break }
                ranges.append(r)
                loc = r.location + r.length
            }
        }
        findMatches = ranges
        findIndex = 0
        if !ranges.isEmpty { jumpToFind(0) }
        updateFindCount()
    }

    private func jumpToFind(_ i: Int) {
        guard findMatches.indices.contains(i), let tv = editorView else { return }
        findIndex = i
        tv.setSelectedRange(findMatches[i])
        tv.scrollRangeToVisible(findMatches[i])
        updateFindCount()
    }

    private func findStep(_ dir: Int) {
        guard !findMatches.isEmpty else { return }
        jumpToFind(((findIndex + dir) % findMatches.count + findMatches.count) % findMatches.count)
    }

    private func updateFindCount() {
        guard let fc = findCountLabel else { return }
        if findMatches.isEmpty {
            fc.stringValue = findField?.stringValue.isEmpty ?? true ? "" : "0/0"
        } else {
            fc.stringValue = "\(findIndex + 1)/\(findMatches.count)"
        }
    }

    private func layoutFindBar() {
        guard let ff = findField, let backdrop = panel.contentView else { return }
        let y = config.headerHeight * zoom + editorTabStripHeight + 2
        let h: CGFloat = 24
        if ff.isHidden {
            ff.frame = .zero
            findCountLabel?.frame = .zero
            return
        }
        let x0 = editorLeftInset + 12
        ff.frame = NSRect(x: x0, y: y,
                          width: max(120, backdrop.bounds.width - x0 - 12 - 56),
                          height: h)
        findCountLabel?.frame = NSRect(x: backdrop.bounds.width - 50, y: y,
                                       width: 42, height: h)
    }

    private func findBarHeight() -> CGFloat {
        findBarShown ? 24 + 6 : 0
    }

    private func terminalDrawerHeight() -> CGFloat {
        terminalShown ? currentTerminalHeight : 0
    }

    private func layoutTerminal() {
        guard let drawer = terminalDrawer, let backdrop = panel.contentView else { return }
        let h = terminalDrawerHeight()
        let meter = (chrome?.meterEnabled ?? false) ? chrome!.meterBarHeight : 0
        let y = max(0, backdrop.bounds.height - meter - h)
        drawer.frame = NSRect(x: terminalInset, y: y,
                              width: max(0, backdrop.bounds.width - 2 * terminalInset),
                              height: h)
    }

    private var sidebarTabs: Bool { tabsBar?.vertical == true }
    private var sidebarWNow: CGFloat { tabsBar?.width(expanded: config.tabsSidebarWidth * zoom) ?? config.tabsSidebarWidth * zoom }
    private func sidebarRailChanged() {
        if config.editMode {
            layoutEditorScroll()
            layoutFindBar()
        } else {
            layoutForZoom()
            layoutSearchField()
        }
    }
    private var listLeft: CGFloat { sidebarTabs ? sidebarWNow + 4 : 0 }
    private func layoutListSidebar() {
        guard !config.editMode, sidebarTabs, let bar = tabsBar, let backdrop = panel.contentView else { return }
        let top = (config.dragHeader ? config.headerHeight * zoom : 0) + topAccessoryHeight
        bar.frame = NSRect(x: 0, y: top, width: sidebarWNow,
                           height: max(0, backdrop.bounds.height - top))
        bar.needsDisplay = true
    }
    public var onSidebarWidthChange: ((CGFloat) -> Void)?
    private func setSidebarWidth(_ w: CGFloat, done: Bool) {
        config.tabsSidebarWidth = (w / zoom).rounded()
        if config.editMode {
            layoutEditorScroll()
            layoutFindBar()
        } else {
            layoutForZoom()
            layoutSearchField()
        }
        if done { onSidebarWidthChange?(config.tabsSidebarWidth) }
    }
    private var editorTabStripHeight: CGFloat {
        if config.tabsSidebarWidth > 0 && config.tabs { return 2 }
        return (tabsBar?.frame.height ?? config.tabBarHeight * zoom) + 2
    }
    private var editorLeftInset: CGFloat { sidebarTabs ? sidebarWNow + 4 : 0 }

    private func installProseSwitch() {
        guard config.editMode, proseProvider != nil, proseSwitch == nil, let backdrop = panel.contentView else { return }
        let sw = ProseModeSwitch(colors: config.colors, editLabel: config.vimEditorExecutable != nil ? "nvim" : "Edit")
        sw.onChange = { [weak self] on in self?.setProse(on) }
        sw.onPopOut = { [weak self] in self?.popOutProse() }
        sw.onStyle = { [weak self, weak sw] rect in
            guard let self, let sw else { return }
            self.docTemplateMenu { menu in
                menu.popUp(positioning: nil, at: NSPoint(x: rect.minX, y: rect.maxY + 4), in: sw)
            }
        }
        if let chrome { backdrop.addSubview(sw, positioned: .below, relativeTo: chrome) } else { backdrop.addSubview(sw) }
        proseSwitch = sw
        layoutEditorScroll()
    }

    @discardableResult
    public func toggleProseFromPrefix() -> Bool {
        guard config.editMode, proseProvider != nil else { return false }
        guard proseShown || vimPaneActive else { return false }
        setProse(!proseShown)
        return true
    }

    public func setProse(_ on: Bool) {
        guard config.editMode, let backdrop = panel.contentView else { return }
        proseSwitch?.prose = on
        if !on {
            guard proseShown else { return }
            proseShown = false
            if vimView != nil, vimPaneActive, let pv = proseView {
                pv.topSourceLine { [weak self] n in
                    guard let n else { return }
                    self?.vimCommand("call cursor(\(n), 1) | normal! zt")
                }
            }
            proseView?.isHidden = true
            if let ed = primaryEditor { panel.makeFirstResponder(ed); focusedPane = .editor }
            return
        }
        if vimView != nil, vimPaneActive { vimCommand("silent! update") }
        guard let src = proseProvider?() else { proseSwitch?.prose = false; return }
        let pv = proseView ?? {
            let v = ProseView(frame: editorScroll?.frame ?? backdrop.bounds)
            if let sw = proseSwitch { backdrop.addSubview(v, positioned: .below, relativeTo: sw) } else { backdrop.addSubview(v) }
            v.onJump = { [weak self] n in
                guard let self, self.vimView != nil, self.vimPaneActive else { return }
                self.vimCommand("call cursor(\(n), 1) | normal! zt")
            }
            proseView = v
            return v
        }()
        proseShown = true
        pv.isHidden = false
        layoutEditorScroll()
        if vimView != nil, vimPaneActive, let l = vimEval("line('w0')"), let n = Int(l.trimmingCharacters(in: .whitespacesAndNewlines)) {
            pv.syncLine = n
        }
        pv.show(src, colors: config.colors, font: proseFont, size: proseFontSize * zoom, width: proseWidth)
        panel.makeFirstResponder(pv.web)
    }
    private func docTemplateMenu(done: @escaping (NSMenu) -> Void) {
        let text = proseProvider?()?.markdown ?? ""
        let configured = configSectionValue("notes", "doc-templates") ?? ""
        let cssPath = (configSectionValue("notes", "pdf-css") ?? "").trimmingCharacters(in: .whitespaces)
        let css = cssPath.isEmpty ? "" : (try? String(contentsOfFile: (cssPath as NSString).expandingTildeInPath, encoding: .utf8)) ?? ""
        pythonHelper.call("doc_templates.menu",
                          ["text": text, "configured": configured, "css": css],
                          timeout: 3) { [weak self] result in
            guard let self else { return }
            if case .success(let value) = result, let menu = value as? [String: Any],
               let names = menu["names"] as? [String] {
                done(self.buildDocTemplateMenu(current: menu["current"] as? String, names: names))
            } else {
                let m = NSMenu()
                m.autoenablesItems = false
                let head = NSMenuItem(title: "Document styles unavailable (python helper)",
                                      action: nil, keyEquivalent: "")
                head.isEnabled = false
                m.addItem(head)
                done(m)
            }
        }
    }

    private func buildDocTemplateMenu(current cur: String?, names: [String]) -> NSMenu {
        let m = NSMenu()
        m.autoenablesItems = false
        let head = NSMenuItem(title: "Document style", action: nil, keyEquivalent: "")
        head.isEnabled = false
        m.addItem(head)
        let none = ClosureMenuItem("Plain (no style)") { [weak self] in self?.applyDocTemplate(nil) }
        none.state = cur == nil ? .on : .off
        m.addItem(none)
        m.addItem(.separator())
        for name in names {
            let it = ClosureMenuItem(name) { [weak self] in self?.applyDocTemplate(name) }
            it.state = cur == name ? .on : .off
            m.addItem(it)
        }
        return m
    }

    private func applyDocTemplate(_ name: String?) {
        guard let text = proseProvider?()?.markdown else { return }
        pythonHelper.call("doc_templates.edit", ["text": text, "template": name ?? NSNull()]) { [weak self] result in
            guard let self else { return }
            if case .success(let value) = result, let edit = value as? [String: Any],
               let remove = edit["remove"] as? Int, let insert = edit["insert"] as? [String] {
                self.applyDocTemplateChange(remove: remove, insert: insert, name: name)
            } else {
                self.showToast("Style change failed (python helper)", symbol: "exclamationmark.triangle.fill")
            }
        }
    }

    private func applyDocTemplateChange(remove: Int, insert: [String], name: String?) {
        if remove == 0 && insert.isEmpty { return }
        if vimView != nil, vimPaneActive {
            let lua = PopupWindow.vimLua([
                "(function(a)",
                "local n = tonumber(a:match('^(%d+)\\n'))",
                "local rest = a:gsub('^%d+\\n', '')",
                "local new = rest == '' and {} or vim.split(rest, '\\n', {plain = true})",
                "vim.api.nvim_buf_set_lines(0, 0, n, false, new)",
                "vim.cmd('silent! update') return 1 end)(_A)",
            ])
            _ = vimEval("luaeval(\(PopupWindow.vimString(lua)), \(PopupWindow.vimDQ("\(remove)\n" + insert.joined(separator: "\n"))))")
        } else if let tv = editorView {
            let ns = tv.string as NSString
            var end = 0
            for _ in 0..<remove where end < ns.length { end = NSMaxRange(ns.lineRange(for: NSRange(location: end, length: 0))) }
            let r = NSRange(location: 0, length: end)
            let new = insert.isEmpty ? "" : insert.joined(separator: "\n") + "\n"
            if tv.shouldChangeText(in: r, replacementString: new) {
                tv.replaceCharacters(in: r, with: new)
                tv.didChangeText()
            }
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + (vimPaneActive ? 0.05 : 0.6)) { [weak self] in self?.refreshProse() }
        showToast(name.map { "Style: \($0)" } ?? "Style removed", symbol: "paintpalette")
    }

    public func popOutProse() {
        if vimView != nil, vimPaneActive { vimCommand("silent! update") }
        guard let src = proseProvider?() else { return }
        ProseProcess.launch(path: src.path, colors: config.colors, font: proseFont, size: proseFontSize, width: proseWidth)
    }
    private func refreshProse() {
        guard proseShown, let src = proseProvider?() else { return }
        proseView?.show(src, colors: config.colors, font: proseFont, size: proseFontSize * zoom, width: proseWidth)
    }

    private func layoutEditorScroll() {
        guard config.editMode, let scroll = editorScroll,
              let backdrop = panel.contentView else { return }
        let topY = config.headerHeight * zoom + editorTabStripHeight + findBarHeight()
        let meter = (chrome?.meterEnabled ?? false) ? chrome!.meterBarHeight + 4 : 0
        let drawerH = (terminalShown ? currentTerminalHeight : 0)
                    + (fileBrowserShown ? currentBrowserHeight : 0)
        let drawer = drawerH + 4
        let statusVisible = !(statusBar?.isHidden ?? true)
        let status = statusVisible ? statusBarHeight + 4 : 0
        scroll.frame.origin.y = topY
        scroll.frame.size.height = max(40, backdrop.bounds.height - topY - meter - drawer - status)
        scroll.frame.origin.x = editorLeftInset
        scroll.frame.size.width = max(80, backdrop.bounds.width - editorLeftInset)
        proseView?.frame = scroll.frame
        if let po = pageOverlay {
            let top = config.headerHeight * zoom
            po.frame = NSRect(x: 0, y: top, width: backdrop.bounds.width, height: max(0, backdrop.bounds.height - top))
        }
        if let sw = proseSwitch {
            sw.frame.origin = NSPoint(x: scroll.frame.maxX - sw.frame.width - 16, y: scroll.frame.maxY - sw.frame.height - 10)
        }
        if sidebarTabs, let bar = tabsBar {
            let top = config.headerHeight * zoom
            bar.frame = NSRect(x: 0, y: top, width: sidebarWNow,
                               height: max(0, scroll.frame.maxY - top))
            bar.needsDisplay = true
        }
        if let vv = vimView {
            vv.frame = scroll.frame.insetBy(dx: 8, dy: 4)
            vimImageOverlay?.frame = vv.frame
            vimImageOverlay?.cell = PopupWindow.cellSize(PopupWindow.vimFont(config))
        }
        if statusVisible, let sb = statusBar {
            sb.frame = NSRect(x: 6, y: backdrop.bounds.height - statusBarHeight - 4 - meter,
                              width: max(0, backdrop.bounds.width - 12),
                              height: statusBarHeight)
        }
        if config.markdownImages {
            editorView?.textContainer?.containerSize = NSSize(
                width: max(120, scroll.bounds.width - 24),
                height: CGFloat.greatestFiniteMagnitude)
            syncEditorDocWidth()
        }
    }

    public func growWidthToContent() {
        var needed: CGFloat = 0
        if let bar = filterBar { needed = max(needed, bar.naturalWidth()) }
        if let chrome = chrome { needed = max(needed, chrome.neededWidth()) }
        guard needed > panel.frame.width else { return }
        let w = min(needed, maxPanelWidth())
        guard w > panel.frame.width else { return }
        let f = panel.frame
        panel.setFrame(NSRect(x: f.origin.x, y: f.origin.y,
                              width: w, height: f.height), display: true)
        layoutSearchField()
        layoutScrollDocument()
        relayoutTabs()
    }

    private func layoutSearchField() {
        guard !config.editMode, let backdrop = panel.contentView else { return }
        let w = backdrop.bounds.width - listLeft - listRight
        let inset = config.padding + 10
        let fieldW = max(50, (w - 2 * inset) * config.searchWidthFraction)
        let x = listLeft + inset
        let headerOffset = ((config.dragHeader) ? config.headerHeight * zoom + 4 : 0) + topAccessoryHeight + listBarHeight
        let y = headerOffset + config.padding + 2
        field.frame = NSRect(x: x, y: y, width: fieldW, height: 24)
        layoutListExtras()
    }

    private func maxPanelHeight() -> CGFloat {
        let screenH = NSScreen.main?.visibleFrame.height ?? 800
        let cap = config.maxHeight > 0 ? config.maxHeight : screenH * 0.6
        return max(100, min(cap, screenH - 40))
    }

    private func maxPanelWidth() -> CGFloat {
        let screenW = NSScreen.main?.visibleFrame.width ?? 1200
        return max(config.width, screenW - 40)
    }

    private func centeredOrigin(width: CGFloat, height: CGFloat) -> NSPoint {
        let mouse = NSEvent.mouseLocation
        let screen = NSScreen.screens.first { $0.frame.contains(mouse) } ?? NSScreen.main!
        let vis = screen.visibleFrame
        return NSPoint(x: vis.midX - width / 2, y: vis.midY - height / 2)
    }

    private func startToggleServer() {
        let socketPath = popupTmpDir() + config.name + ".sock"
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            guard let fd = listenUnixSocket(socketPath) else { return }
            var lastToggle: TimeInterval = 0
            while true {
                let cfd = accept(fd, nil, nil)
                guard cfd >= 0 else { continue }
                var buf = [UInt8](repeating: 0, count: 128)
                let n = read(cfd, &buf, buf.count)
                close(cfd)
                if n > 0 {
                    let msg = String(bytes: buf[..<n], encoding: .utf8) ?? ""
                    let expected = "toggle \(self?.config.name ?? "")"
                    if msg.trimmingCharacters(in: .whitespacesAndNewlines) == expected {
                        let now = ProcessInfo.processInfo.systemUptime
                        guard now - lastToggle > 0.12 else { continue }
                        lastToggle = now
                        DispatchQueue.main.async { self?.toggle() }
                    }
                }
            }
        }
    }
}

public func popupSocketPath(name: String) -> String {
    popupTmpDir() + name + ".sock"
}

@discardableResult
public func sendToggle(name: String) -> Bool {
    let path = popupSocketPath(name: name)
    let fd = socket(AF_UNIX, SOCK_STREAM, 0)
    guard fd >= 0 else { return false }
    defer { close(fd) }
    var addr = makeUnixSockAddr(path)
    let ok = withUnsafePointer(to: &addr) { ptr -> Bool in
        ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) {
            connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) == 0
        }
    }
    guard ok else { return false }
    let msg = "toggle \(name)\n"
    msg.withCString { _ = write(fd, $0, msg.count) }
    return true
}

func makeToastPill(_ text: String, symbol: String?, colors c: PopupColors, zoom: CGFloat,
                   maxWidth: CGFloat, boxed: Bool = false) -> NSView {
    let pill = NSView()
    pill.wantsLayer = true
    pill.layer?.backgroundColor = c.crust.withAlphaComponent(0.94).cgColor
    pill.layer?.borderColor = c.text.withAlphaComponent(0.10).cgColor
    pill.layer?.borderWidth = 1
    pill.layer?.shadowColor = NSColor.black.cgColor
    pill.layer?.shadowOpacity = 0.25
    pill.layer?.shadowRadius = 8
    pill.layer?.shadowOffset = CGSize(width: 0, height: -2)

    let label = NSTextField(labelWithString: text)
    label.font = boxed ? .monospacedSystemFont(ofSize: 13.5 * zoom, weight: .medium)
                       : .systemFont(ofSize: 12.5 * zoom, weight: .medium)
    label.textColor = c.text
    label.lineBreakMode = .byTruncatingMiddle
    label.cell?.truncatesLastVisibleLine = true
    var icon: NSImageView?
    if let symbol, let img = NSImage(systemSymbolName: symbol, accessibilityDescription: nil) {
        let iv = NSImageView(image: img)
        iv.symbolConfiguration = .init(pointSize: 12.5 * zoom, weight: .medium)
        iv.contentTintColor = c.tone(.success)
        icon = iv
    }
    label.sizeToFit()
    let padX = (boxed ? 26 : 16) * zoom, gap = 8 * zoom
    let iconSize = icon?.fittingSize ?? .zero
    let groupExtra = icon == nil ? 0 : iconSize.width + gap
    let h = (boxed ? 56 : 32) * zoom
    let w = min(ceil(label.frame.width + groupExtra + padX * 2), maxWidth)
    let labelW = max(0, w - padX * 2 - groupExtra)
    let groupW = groupExtra + labelW
    var x = (w - groupW) / 2
    if let icon {
        icon.frame = NSRect(x: x, y: round((h - iconSize.height) / 2),
                            width: iconSize.width, height: iconSize.height)
        pill.addSubview(icon)
        x += groupExtra
    }
    label.frame = NSRect(x: x, y: round((h - label.frame.height) / 2),
                         width: labelW, height: label.frame.height)
    pill.addSubview(label)
    pill.frame = NSRect(x: 0, y: 0, width: w, height: h)
    if boxed {
        pill.layer?.cornerRadius = 4 * zoom
        pill.layer?.borderColor = c.tone(.success).cgColor
        pill.layer?.borderWidth = 2
        pill.layer?.backgroundColor = c.crust.cgColor
    } else {
        pill.layer?.cornerRadius = h / 2
    }
    return pill
}

func animateToastPill(_ pill: NSView, rise: CGFloat, hold: TimeInterval = 1.4, fade: TimeInterval = 0.25,
                      done: (() -> Void)? = nil) {
    let final = pill.frame
    pill.alphaValue = 0
    pill.setFrameOrigin(NSPoint(x: final.minX, y: final.minY + rise))
    NSAnimationContext.runAnimationGroup { ctx in
        ctx.duration = 0.18
        ctx.timingFunction = CAMediaTimingFunction(name: .easeOut)
        pill.animator().alphaValue = 1
        pill.animator().setFrameOrigin(final.origin)
    }
    DispatchQueue.main.asyncAfter(deadline: .now() + hold) { [weak pill] in
        guard let pill, pill.superview != nil else { return }
        NSAnimationContext.runAnimationGroup({ ctx in
            ctx.duration = fade
            ctx.timingFunction = CAMediaTimingFunction(name: .easeIn)
            pill.animator().alphaValue = 0
        }, completionHandler: { if let done { done() } else { pill.removeFromSuperview() } })
    }
}

public typealias ShortcutRows = (title: String, items: [(keys: String, what: String)])

final class ShortcutsOverlay: NSView {
    typealias Group = ShortcutRows
    var onClose: (() -> Void)?
    private weak var scroll: NSScrollView?
    private let z: CGFloat

    private final class Card: NSView {
        override func mouseDown(with e: NSEvent) {}
    }

    init(groups: [Group], colors c: PopupColors, zoom: CGFloat, in root: NSView,
         title: String = "Keyboard Shortcuts") {
        z = zoom
        super.init(frame: root.bounds)
        autoresizingMask = [.width, .height]
        wantsLayer = true
        layer?.backgroundColor = NSColor.black.withAlphaComponent(0.25).cgColor

        let titleH = 40 * z
        let w = min(640 * z, root.bounds.width - 40)
        let list = ShortcutsListView(groups: groups, colors: c, zoom: z, width: w)
        let h = min(titleH + list.contentHeight, root.bounds.height - 60)
        let card = Card(frame: NSRect(x: ((root.bounds.width - w) / 2).rounded(),
                                      y: ((root.bounds.height - h) / 2).rounded(), width: w, height: h))
        card.autoresizingMask = [.minXMargin, .maxXMargin, .minYMargin, .maxYMargin]
        card.wantsLayer = true
        let fill = (c.background.usingColorSpace(.sRGB) ?? c.background)
            .blended(withFraction: 0.25, of: .black) ?? c.background
        card.layer?.backgroundColor = fill.withAlphaComponent(0.98).cgColor
        card.layer?.cornerRadius = 12 * z
        card.layer?.borderColor = c.text.withAlphaComponent(0.15).cgColor
        card.layer?.borderWidth = 1
        card.layer?.shadowColor = NSColor.black.cgColor
        card.layer?.shadowOpacity = 0.4
        card.layer?.shadowRadius = 18
        addSubview(card)

        let t = NSTextField(labelWithString: title)
        t.font = .systemFont(ofSize: 14 * z, weight: .semibold)
        t.textColor = c.text
        t.sizeToFit()
        t.frame.origin = NSPoint(x: 14 * z, y: h - titleH / 2 - t.frame.height / 2)
        card.addSubview(t)
        let hint = NSTextField(labelWithString: "esc to close")
        hint.font = .systemFont(ofSize: 11 * z)
        hint.textColor = c.dim
        hint.sizeToFit()
        hint.frame.origin = NSPoint(x: w - hint.frame.width - 14 * z, y: h - titleH / 2 - hint.frame.height / 2)
        card.addSubview(hint)
        let rule = NSView(frame: NSRect(x: 0, y: h - titleH, width: w, height: 1))
        rule.wantsLayer = true
        rule.layer?.backgroundColor = c.hairline.cgColor
        card.addSubview(rule)

        let sv = NSScrollView(frame: NSRect(x: 0, y: 0, width: w, height: h - titleH))
        sv.drawsBackground = false
        sv.hasVerticalScroller = true
        sv.autohidesScrollers = true
        sv.borderType = .noBorder
        sv.documentView = list
        card.addSubview(sv)
        list.scroll(.zero)
        scroll = sv
        root.addSubview(self, positioned: .above, relativeTo: nil)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) not supported") }

    override func mouseDown(with e: NSEvent) { onClose?() }

    func handleKey(_ code: UInt16, _ mods: NSEvent.ModifierFlags) -> Bool {
        let ctrl = mods.contains(.control)
        var dy: CGFloat = 0
        switch (code, ctrl) {
        case (53, _), (36, _), (76, _), (49, false), (12, false):
            onClose?()
            return true
        case (44, _) where mods.contains(.command):
            onClose?()
            return true
        case (125, _), (45, true), (38, true): dy = 60 * z
        case (126, _), (35, true), (40, true): dy = -60 * z
        default: return true
        }
        if let sv = scroll, let doc = sv.documentView {
            let clip = sv.contentView
            let maxY = max(0, doc.frame.height - clip.bounds.height)
            clip.scroll(to: NSPoint(x: 0, y: min(maxY, max(0, clip.bounds.origin.y + dy))))
            sv.reflectScrolledClipView(clip)
        }
        return true
    }
}

private final class ShortcutsListView: NSView {
    let groups: [ShortcutsOverlay.Group]
    let c: PopupColors
    let z: CGFloat
    override var isFlipped: Bool { true }
    init(groups: [ShortcutsOverlay.Group], colors: PopupColors, zoom: CGFloat, width: CGFloat) {
        self.groups = groups
        c = colors
        z = zoom
        super.init(frame: NSRect(x: 0, y: 0, width: width, height: 0))
        frame.size.height = contentHeight
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) not supported") }

    private var rowH: CGFloat { 27 * z }
    private var headH: CGFloat { 30 * z }
    var contentHeight: CGFloat {
        groups.reduce(6 * z) { $0 + headH + CGFloat($1.items.count) * rowH } + 8 * z
    }
    private var capFont: NSFont { .monospacedSystemFont(ofSize: 11 * z, weight: .medium) }
    private func capsWidth(_ keys: String) -> CGFloat {
        let parts = keys.components(separatedBy: " / ")
        let slash = ("/" as NSString).size(withAttributes: [.font: capFont]).width + 8 * z
        return parts.reduce(0) { $0 + ($1 as NSString).size(withAttributes: [.font: capFont]).width + 12 * z }
            + CGFloat(max(0, parts.count - 1)) * slash
    }
    override func draw(_ dirtyRect: NSRect) {
        let pad = 14 * z
        let keyCol = min(bounds.width * 0.5,
                         groups.flatMap(\.items).map { capsWidth($0.keys) }.max() ?? 0) + pad + 16 * z
        let headFont = NSFont.systemFont(ofSize: 10 * z, weight: .bold)
        let textFont = NSFont.systemFont(ofSize: 12.5 * z)
        let trunc = NSMutableParagraphStyle()
        trunc.lineBreakMode = .byTruncatingTail
        var y = 6 * z
        for g in groups {
            (g.title.uppercased() as NSString).draw(
                at: NSPoint(x: pad, y: y + headH - 18 * z),
                withAttributes: [.font: headFont, .foregroundColor: c.accentOn, .kern: 0.8])
            y += headH
            for it in g.items {
                var x = pad
                let capH = 19 * z
                let capY = y + (rowH - capH) / 2
                for (i, part) in it.keys.components(separatedBy: " / ").enumerated() {
                    if i > 0 {
                        let a: [NSAttributedString.Key: Any] = [.font: capFont, .foregroundColor: c.dim]
                        let sz = ("/" as NSString).size(withAttributes: a)
                        ("/" as NSString).draw(at: NSPoint(x: x + 4 * z, y: capY + (capH - sz.height) / 2),
                                               withAttributes: a)
                        x += sz.width + 8 * z
                    }
                    let a: [NSAttributedString.Key: Any] = [.font: capFont, .foregroundColor: c.text]
                    let sz = (part as NSString).size(withAttributes: a)
                    let cap = NSRect(x: x, y: capY, width: sz.width + 12 * z, height: capH)
                    let path = NSBezierPath(roundedRect: cap, xRadius: 5 * z, yRadius: 5 * z)
                    c.mantle.setFill()
                    path.fill()
                    c.hairline.setStroke()
                    path.lineWidth = 1
                    path.stroke()
                    (part as NSString).draw(at: NSPoint(x: cap.minX + 6 * z, y: cap.midY - sz.height / 2),
                                            withAttributes: a)
                    x = cap.maxX
                }
                let tx = max(keyCol, x + 12 * z)
                let lineH = textFont.ascender - textFont.descender
                (it.what as NSString).draw(
                    with: NSRect(x: tx, y: y + (rowH - lineH) / 2, width: bounds.width - tx - pad, height: lineH),
                    options: [.usesLineFragmentOrigin],
                    attributes: [.font: textFont, .foregroundColor: c.text, .paragraphStyle: trunc])
                y += rowH
            }
        }
    }
}

extension PopupWindow: PaneProvider {
    var navPanes: [NavPane] {
        var out: [NavPane] = []
        if let bar = tabsBar, bar.vertical {
            out.append(NavPane("sidebar", bar, focus: { [weak bar] in bar?.takeKeyboardFocus() }))
        }
        if let acc = topAccessory { out.append(.area("search", acc)) }
        if let page = pageOverlay {
            out.append(.area("page", page))
        } else if proseShown, let pv = proseView {
            out.append(NavPane("editor", pv, focus: { [weak self, weak pv] in
                if let pv { self?.panel.makeFirstResponder(pv.web) }
            }))
        } else if config.editMode, let ed = primaryEditor {
            let area = ed === vimView ? ed : (ed.enclosingScrollView ?? ed)
            out.append(NavPane("editor", area, focus: { [weak self, weak ed] in
                if let ed { self?.panel.makeFirstResponder(ed) }
            }, intercept: { [weak self] dir in self?.vimSplitMove(dir) ?? false }))
        }
        if let fb = fileBrowser, fb.superview != nil, !fb.isHidden, !config.editMode || fileBrowserShown {
            out += fb.navPanes
        }
        if config.editMode, terminalShown, let term = terminalDrawer {
            out.append(NavPane("terminal", term))
        }
        if !config.editMode, fileBrowser == nil {
            let area: NSView = listOverlay ?? rowScroll ?? rowView
            var list = NavPane("list", area, focus: { [weak self] in
                guard let self else { return }
                self.panel.makeFirstResponder(self.field)
            }, owns: { [weak self] r in
                guard let self else { return false }
                return NavPane.inside(r, self.field) || NavPane.inside(r, area)
            })
            let walker = PopupListVim(self)
            list.vim = { [weak self] in
                guard let self else { return nil }
                if let ov = self.listOverlay { return VimTarget.find(in: ov) }
                return self.config.enableNavigation ? .rows(walker) : nil
            }
            list.normal = { [weak self] in
                guard let self else { return }
                if self.panel.firstResponder !== self.field.currentEditor() { self.panel.makeFirstResponder(self.field) }
                VimKeys.shared.enterFieldNormal(self.field, in: self.panel)
            }
            list.insert = { [weak self] in
                guard let self else { return }
                if self.panel.firstResponder !== self.field.currentEditor() { self.panel.makeFirstResponder(self.field) }
            }
            out.append(list)
            if let iv = inspectorView, inspectorShown { out.append(.area("issue", iv)) }
        }
        return out
    }

    func paneFocusMoved() { updateFocusedPane() }

    private func vimSplitMove(_ dir: PaneDir) -> Bool {
        guard focusedVim() != nil else { return false }
        let k = dir.rawValue
        guard let r = vimEval("winnr('\(k)') != winnr()"), r.trimmingCharacters(in: .whitespacesAndNewlines) == "1"
        else { return false }
        vimCommand("wincmd \(k)")
        return true
    }
}

extension PopupFileBrowser: PaneProvider {
    var navPanes: [NavPane] {
        var out: [NavPane] = []
        if let bar = sidebar {
            out.append(NavPane("files-sidebar", bar, focus: { [weak bar] in bar?.takeKeyboardFocus() }))
        }
        var filter = NavPane("files-filter", searchField, focus: { [weak self] in
            guard let self else { return }
            self.window?.makeFirstResponder(self.searchField)
        })
        filter.normal = { [weak self] in
            guard let self else { return }
            self.window?.makeFirstResponder(self.listPane)
        }
        out.append(filter)
        var list = NavPane("files-list", listScroll, focus: { [weak self] in
            guard let self else { return }
            self.window?.makeFirstResponder(self.listPane)
        })
        list.insert = filter.focus
        out.append(list)
        if !previewListScroll.isHidden {
            out.append(NavPane("files-preview", previewListScroll, focus: { [weak self] in
                guard let self else { return }
                self.window?.makeFirstResponder(self.previewList)
            }))
        } else if !previewScroll.isHidden {
            out.append(NavPane("files-preview", previewScroll, focus: { [weak self] in
                guard let self else { return }
                self.window?.makeFirstResponder(self.previewText)
            }))
        } else if !previewImage.isHidden, previewImage.image != nil {
            out.append(NavPane("files-preview", previewImage))
        }
        return out
    }
}

extension PopupTabsBar: VimRows {
    var vimCount: Int { rowCount }
    var vimCursor: Int { cursor }
    var vimPage: Int { max(1, Int(vListRect.height / (vRowH + 1))) }
    func vimText(_ row: Int) -> String {
        guard row < pinnedShown else { return rowTitle(row) }
        return rowTitle(row) + " " + (pinnedSection?(pinned[row]) ?? pinnedTitle)
    }
    func vimMove(to row: Int) { moveCursor(to: row) }
    func vimShownRows() -> (view: NSView, rows: [(row: Int, rect: NSRect)])? {
        guard vertical, !collapsed else { return nil }
        let l = vListRect
        var out = vPinnedRows().prefix(pinnedShown).enumerated().map { (row: $0.offset, rect: $0.element.rect) }
        out += vRows().map { (row: pinnedShown + $0.index, rect: $0.rect.intersection(l)) }
        return (self, out)
    }
}

extension FileListPane: VimRows {
    var vimCount: Int { rows.count }
    var vimCursor: Int { selection }
    var vimPage: Int { pageRows }
    func vimText(_ row: Int) -> String { rows.indices.contains(row) ? rows[row].name : "" }
    func vimMove(to row: Int) {
        moveSelection(row - selection)
        scrollToVisible(NSRect(x: 0, y: CGFloat(selection) * rowH, width: 1, height: rowH))
    }
    func vimShownRows() -> (view: NSView, rows: [(row: Int, rect: NSRect)])? {
        Self.shownRows(in: self, count: rows.count, rowH: rowH, rect: rowRect)
    }
}

final class PopupListVim: VimRows {
    private weak var w: PopupWindow?
    init(_ w: PopupWindow) { self.w = w }
    var vimCount: Int { w?.rows.count ?? 0 }
    var vimCursor: Int { w?.selection ?? 0 }
    var vimPage: Int { w?.vimVisibleRows ?? 10 }
    func vimText(_ row: Int) -> String { w?.vimRowText(row) ?? "" }
    func vimMove(to row: Int) {
        guard let w, w.rows.indices.contains(row) else { return }
        w.selection = row
    }
    func vimShownRows() -> (view: NSView, rows: [(row: Int, rect: NSRect)])? { w?.vimShownRows() }
}

extension PopupWindow {
    var vimVisibleRows: Int {
        let h = rowScroll?.contentView.bounds.height ?? rowView.bounds.height
        return max(1, Int(h / max(18, config.rowHeight * zoom * textZoom)))
    }
    func vimShownRows() -> (view: NSView, rows: [(row: Int, rect: NSRect)]) {
        (rowView, rowView.shownRows())
    }
    func vimRowText(_ i: Int) -> String {
        guard rows.indices.contains(i) else { return "" }
        let r = rows[i]
        var parts = [r.title, r.content ?? "", r.detail ?? "", r.trailing ?? ""]
        for c in config.tableColumns { parts.append(r.cellText(c.field) ?? "") }
        return parts.joined(separator: " ")
    }
}
