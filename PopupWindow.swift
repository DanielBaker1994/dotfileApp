import AppKit
import PDFKit
import SwiftTerm
import Foundation
import Darwin
import UniformTypeIdentifiers

// Keep a frame fully inside the screen's visible area (used by both the
// popup window and the chrome/backdrop drag-resize handlers so the scrollbar
// and bottom pills never go off-screen).
func clampToScreen(_ f: NSRect) -> NSRect {
    let screen = NSScreen.screens.first { $0.frame.contains(f.origin) }
        ?? NSScreen.main!
    let vis = screen.visibleFrame
    var r = f
    r.size.width = min(r.width, vis.width - 8)
    r.size.height = min(r.height, vis.height - 8)
    if r.origin.x < vis.minX { r.origin.x = vis.minX }
    if r.origin.y < vis.minY { r.origin.y = vis.minY }
    if r.maxX > vis.maxX { r.origin.x = vis.maxX - r.width }
    if r.maxY > vis.maxY { r.origin.y = vis.maxY - r.height }
    return r
}

// MARK: - ANSI SGR rendering

// Parse ANSI SGR escapes ("ESC [ 32 m" etc.) into an attributed string so
// terminal output (doctor's colored PASS/FAIL/WARN lines) keeps its colors in
// the popup editor. Text without escapes gets the base style; unhandled codes
// are dropped. fg: 30-37 / 90-97 basic + bright, 39 reset; 1 bold, 22 normal.
// editor font for the given family + zoom (13pt base, scales with zoom)
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

// MARK: - Syntax highlighting (JSON / XML)

// Syntax-highlight prettyprinted JSON/XML with the window colors: keys/tags in
// blue, string values green, numbers amber, booleans/null + attribute names
// purple, punctuation dim. Plain text renders in the base color untouched.
// The highlight lives entirely in the framework so every editor window can opt
// in (the prettyprint window does via setEditorSyntaxHighlighted).
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

// palette shared by the JSON/XML highlighters (hues borrowed from the app's
// existing accents so a colored value reads consistently across windows)
private let popupKeyColor = NSColor(srgbRed: 0.48, green: 0.70, blue: 1.00, alpha: 1)   // keys / tag names
private let popupStrColor = NSColor(srgbRed: 0.55, green: 0.80, blue: 0.52, alpha: 1)   // string values
private let popupNumColor = NSColor(srgbRed: 0.95, green: 0.66, blue: 0.30, alpha: 1)   // numbers
private let popupKwColor  = NSColor(srgbRed: 0.73, green: 0.62, blue: 0.95, alpha: 1)   // true/false/null + attr names

private func popupAttr(_ s: String, _ font: NSFont, _ color: NSColor,
                       italic: Bool = false) -> NSAttributedString {
    var attrs: [NSAttributedString.Key: Any] = [.font: font, .foregroundColor: color]
    if italic { attrs[.obliqueness] = 0.15 }
    return NSAttributedString(string: s, attributes: attrs)
}

// JSON: one regex finds strings / numbers / keywords; a string followed by
// `:` (ignoring whitespace) is a key, everything else is a value. Unmatched
// runs get base color with punctuation (`{ } [ ] , :`) dimmed.
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
            // string: a key when the next non-whitespace char is a colon
            var after = NSMaxRange(r)
            while after < len {
                let c = ns.character(at: after)
                if c == 32 || c == 9 { after += 1; continue }
                break
            }
            let isKey = after < len && ns.character(at: after) == 58 // ':'
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

// XML: scan for tags (`<…>`), coloring the tag name, attributes and their
// values; comments are dimmed italic; the text between tags stays base color.
private func popupHighlightXML(_ text: String, font: NSFont,
                               colors: PopupColors) -> NSAttributedString {
    let out = NSMutableAttributedString()
    let ns = text as NSString
    let len = ns.length
    var i = 0
    while i < len {
        if ns.character(at: i) == 60 { // '<'
            // comment
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
            // find the `>` closing the tag (ignore `>` inside quoted values)
            var gt = i + 1
            var quote: unichar = 0
            while gt < len {
                let c = ns.character(at: gt)
                if quote != 0 {
                    if c == quote { quote = 0 }
                } else if c == 34 || c == 39 { // " or '
                    quote = c
                } else if c == 62 { break }    // >
                gt += 1
            }
            if gt >= len { gt = len - 1 }
            let tagRange = NSRange(location: i, length: gt - i + 1)
            out.append(popupAttributedTag(ns.substring(with: tagRange), font: font,
                                          colors: colors))
            i = gt + 1
        } else {
            // text content up to the next '<'
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

// One tag's internals: < / name / attr="val" … / >  with each part colored.
private func popupAttributedTag(_ tag: String, font: NSFont,
                                colors: PopupColors) -> NSAttributedString {
    let out = NSMutableAttributedString()
    let ns = tag as NSString
    let len = ns.length
    var k = 0
    // opening: `<`, `</`, `<?`, `<!`
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
    // tag name (up to whitespace / `>` / `/` / `?`)
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
    // attributes
    while k < len {
        let c = ns.character(at: k)
        // end / self-close, or a `?` closing a processing instruction (`<?xml …?>`)
        if c == 62 || c == 47 || c == 63 { break }
        if c == 32 || c == 9 {            // whitespace run
            var w = k
            while w < len && (ns.character(at: w) == 32 || ns.character(at: w) == 9) { w += 1 }
            out.append(popupAttr(ns.substring(with: NSRange(location: k, length: w - k)),
                                 font, colors.dim))
            k = w
            continue
        }
        var nameEnd = k
        while nameEnd < len && ns.character(at: nameEnd) != 61 { nameEnd += 1 } // '='
        out.append(popupAttr(ns.substring(with: NSRange(location: k, length: nameEnd - k)),
                             font, popupKwColor))
        k = nameEnd
        if k < len && ns.character(at: k) == 61 {
            out.append(popupAttr("=", font, colors.dim))
            k += 1
            let v = ns.character(at: k)
            if v == 34 || v == 39 {       // quoted value
                var e = k + 1
                while e < len && ns.character(at: e) != v { e += 1 }
                if e < len { e += 1 }
                out.append(popupAttr(ns.substring(with: NSRange(location: k, length: e - k)),
                                     font, popupStrColor))
                k = e
            } else {                      // unquoted value
                var e = k
                while e < len, ns.character(at: e) != 32,
                      ns.character(at: e) != 9, ns.character(at: e) != 62 { e += 1 }
                out.append(popupAttr(ns.substring(with: NSRange(location: k, length: e - k)),
                                     font, popupStrColor))
                k = e
            }
        }
    }
    // trailing `>` / `/>` / `?>`
    if k < len {
        out.append(popupAttr(ns.substring(from: k), font, colors.dim))
    }
    return out
}

// ============================================================================
// PopupWindow — a reusable AppKit popup framework.
//
// Everything about *building* a searchable popup window lives here:
//   - window/panel construction (borderless, nonactivating, shadow, blur)
//   - dimensions, colors, fonts (all via PopupConfig)
//   - search field + optional handlers (search / navigation / escape / toggle)
//   - row drawing (selection pill + title + optional icons + trailing text)
//
// The host app supplies rows and behavior through closures. No app-specific
// logic (aerospace, commands, icons-by-bundle) lives in this file.
// ============================================================================

// MARK: - Shared socket helpers (framework + host app both use these)

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

// MARK: - Colors

public struct PopupColors {
    public var background: NSColor   // card fill (blur tinted by tintAlpha)
    public var border: NSColor       // card outline
    public var text: NSColor         // row titles + input text
    public var dim: NSColor          // secondary text (e.g. "+N")
    public var highlight: NSColor    // selected-row pill fill
    public var accent: NSColor       // the signature hue: active tab, focus, on-state
    // the rest of a theme's palette, so every element can wear its own role
    // (the catppuccin-tmux idea: one theme, many distinct hues) instead of
    // one tint everywhere
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

// Secondary hues of a theme (Theme ▸ presets carry the official ones):
//   accent2 — links / keys / fuzzy-match highlights / folder marks
//   success · warning · danger · info — status colors (jira statuses and
//   priorities, poll state, the ✕ hover, the terminal's ANSI palette)
public struct PopupPalette {
    public var accent2: NSColor
    public var success: NSColor
    public var warning: NSColor
    public var danger: NSColor
    public var info: NSColor

    // Catppuccin Macchiato — matches the default card
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

// A table cell's semantic color (host decides per field/value; the window
// maps it onto the live palette so a theme switch recolors it)
public enum PopupTone { case text, dim, accent, accent2, success, warning, danger, info }

// Depth + role tokens derived from the palette. Surfaces are layered like a
// terminal theme: crust (deepest — header strip, status line) < mantle
// (tab strip, toolbars, recessed inputs) < base (the card) < surface
// (raised buttons). Light themes get the same order (deeper = darker).
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
    // the accent lifted until it reads on the card
    var accentOn: NSColor { ButtonStyle.accent(self) }
    // text drawn ON an accent fill (the active tab): the deep crust when it
    // contrasts, else black/white
    var onAccent: NSColor {
        let a = accentOn
        return ButtonStyle.contrast(crust, a) >= 4.5 ? crust
            : ButtonStyle.contrast(.white, a) >= ButtonStyle.contrast(.black, a) ? .white : .black
    }
    // a palette hue nudged toward the text until it reads on the card
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
    // hairline between regions (header/tab strip, table rows)
    var hairline: NSColor { text.withAlphaComponent(isLight ? 0.12 : 0.08) }
    // the card's outline: the accent sunk into the card, so every theme gets
    // a frame in its own hue instead of one fixed color
    var outline: NSColor {
        (accentOn.blended(withFraction: 0.45, of: base) ?? accentOn).withAlphaComponent(0.85)
    }
}

// MARK: - Config

public struct PopupConfig {
    // identity
    public var name: String           // used for the toggle socket + window title

    // window geometry (pts)
    public var width: CGFloat = 250
    public var rowHeight: CGFloat = 30
    public var padding: CGFloat = 8
    public var headerHeight: CGFloat = 30
    public var cornerRadius: CGFloat = 9

    // fonts (used by the framework's own input field + default row drawing)
    public var inputFontSize: CGFloat = 12
    public var rowFontSize: CGFloat = 11

    // standardized button theme (tabs, path chips, header segments, filter
    // chips) — one look across every window: squared-off chips, a hover
    // fill, and the active item marked by an accent underline (no capsules)
    public var buttonRadius: CGFloat = 4
    public var buttonFontSize: CGFloat = 10.5
    // hover/pressed shading applied over the normal fill (0 = none)
    public var buttonHoverAlpha: CGFloat = 0.18
    public var buttonPressedAlpha: CGFloat = 0.32

    // UI zoom: scales fonts, row heights and chrome sizes proportionally
    // (Ctrl/Cmd+± drives it alongside the window resize)
    public var zoom: CGFloat = 1.0

    // appearance
    public var tintAlpha: CGFloat = 0.78            // card fill opacity over the blur
    // tabs strip sits on a SOLID card fill: a window's transparency never
    // reaches its tab pills (any section's `tabs-opaque`, default on)
    public var opaqueTabs: Bool = true
    public var material: NSVisualEffectView.Material = .hudWindow
    public var hasShadow: Bool = true
    public var colors: PopupColors = PopupColors()
    // drag-header look: headerColor tints the header strip (nil = window
    // background); titlePill draws the gray pill behind the centered title
    public var headerColor: NSColor? = nil
    public var titlePill: Bool = true
    // stretch the right-side header buttons to fill the whole header strip
    // (from the right edge back to the icon/meta) instead of a compact
    // cluster hugging the right edge — cleaner for title-less editor windows
    public var stretchHeaderButtons: Bool = false

    // optional behaviors (turn on/off at construction time)
    public var enableSearch: Bool = true            // input field + filtering
    public var enableNavigation: Bool = true        // Down/Up/Tab/ctrl+n/ctrl+p cycling
    public var wrapNavigation: Bool = true          // wrap list navigation at the
                                                    // ends (dropdowns turn this off)
    public var enableEscape: Bool = true            // Esc dismiss (or onEscape hook)
    public var enableToggle: Bool = true            // Unix-socket toggle server
    public var dismissOnClickOff: Bool = true       // click outside hides
    public var dynamicHeight: Bool = false          // shrink window when filtering narrows rows
    public var enableResize: Bool = false           // drag corners/edges to resize; rows fill
    public var enableDrag: Bool = false             // drag anywhere on the frame to move the window

    // sticky: clicking another app/screen focuses it WITHOUT closing this
    // window; it stays visible until Esc (local when focused, global when
    // another app has focus) dismisses it. Implies no click-off dismiss.
    public var sticky: Bool = false

    // float: the window stays above every normal app window (default). Off =
    // an ordinary window that other apps can cover (commands.conf `float`)
    public var floating: Bool = true

    // multi-line rows: if a row supplies `content`, it is drawn wrapped under
    // the title (up to 3 lines) and the window grows to fit
    public var wrapContent: Bool = false
    // hard cap on window height (pts). 0 = 60% of the screen's visible height.
    public var maxHeight: CGFloat = 0
    // edit mode: the window becomes a plain-text editor (no search/rows).
    // Host sets editorText before show() and receives text back via
    // onEditorCommit (Cmd+S) and onEditorClose (window hiding). height is the
    // starting window height in pts.
    public var editMode: Bool = false
    public var height: CGFloat = 420

    // edit mode: an embedded terminal drawer at the bottom of the window
    // (SwiftTerm's LocalProcessTerminalView — a real shell, session survives
    // the drawer being hidden). Toggle via toggleTerminalDrawer().
    public var terminal: Bool = false
    public var terminalHeight: CGFloat = 240
    // starting directory for the embedded shell (commands.conf `terminal-dir`)
    public var terminalDir = "/tmp/"
    // embedded file-browser drawer (notes etc.): toggled like the terminal;
    // both drawers can be open at once (they stack, window grows)
    public var fileBrowserHeight: CGFloat = 300
    // file browser ([files] in commands.conf): sort key (name | modified |
    // created | size | kind) + direction, the recursive-search result cap and
    // excluded globs, and the filter words that open a terminal in the cwd
    public var browserSort = "name"
    public var browserSortDescending = false
    public var browserSearchLimit = 2000
    public var browserSearchExcludes = ["/Library", "node_modules", ".Trash"]
    public var browserTerminalWords = ["term", "terminal", "cmd"]
    // silvery-blue "panel" background shared by the file browser and the
    // embedded terminal drawer; commands.conf `browser-background` /
    // `terminal-background` override it. The interactive color picker (the
    // paint-brush header button) edits this live and persists the hex back
    // to commands.conf so the pick survives a restart.
    public var fileBrowserBackground = NSColor(srgbRed: 0.31, green: 0.35, blue: 0.43, alpha: 0.55)
    // the embedded terminal's own background (same silvery blue by default so
    // terminal + file explorer share one "panel" look); the terminal's text
    // color is derived from it automatically for contrast
    public var terminalBackground = NSColor(srgbRed: 0.31, green: 0.35, blue: 0.43, alpha: 0.78)
    // shell drawer text color (nil = colors.text)
    public var terminalForeground: NSColor? = nil
    // when a file-browser drawer is installed, open it (and close the
    // terminal) from the start instead of the terminal being the default
    public var fileBrowserDefault = false
    // the terminal drawer is open at launch (when no file browser takes the
    // default slot); false = start with the drawer closed
    public var terminalStartsOpen = true
    // shell the terminal drawer (and the host's command runner) spawn
    public var shell = "/opt/homebrew/bin/bash"
    // font for the terminal drawer (a Nerd Font so glyphs/powerline render)
    public var terminalFont = "Hack Nerd Font"
    // args passed to that shell: --login -i makes it read the profile AND
    // rc files (~/.bash_profile + ~/.bashrc), so aliases/functions/zoxide etc.
    // defined there work in the embedded terminal
    public var shellArgs: [String] = ["--login", "-i"]
    // point size of the terminal drawer font (commands.conf `terminal-font-size`)
    public var terminalFontSize: CGFloat = 13
    // point size of the note editor font (commands.conf `font-size`)
    public var editorFontSize: CGFloat = 13
    // vim mode (edit windows): a long-lived editor process (nvim) runs in a
    // chrome-less terminal pane that takes the text editor's place. Tabs,
    // drawers and chrome keep working; the host swaps files over the RPC
    // socket instead of quitting/relaunching. nil = plain text editor.
    public var vimEditorExecutable: String?
    // launch args (the host's vimLaunchArgs closure overrides these on every
    // (re)launch so a restarted editor opens the CURRENT note)
    public var vimEditorArgs: [String] = []
    // nvim --listen socket path (RPC for tab switches / saves / queries)
    public var vimEditorSocket: String?
    // this many rapid Esc presses close the window (editor, file browser,
    // list, shell drawer; in the vim pane vim must already be in Normal mode
    // on the last one). Earlier presses still reach vim / the shell.
    // 1 = a single Esc closes, 0 = Esc never closes.
    public var escCloseCount: Int = 1
    // toast shown after Cmd+K copies a path in the file browser; "{}" is
    // replaced by the (~-abbreviated) path. Empty = no toast.
    public var copyToast: String = "Copied {} to clipboard"
    // vim pane: JSON file the editor writes inline-image placements to
    // (vim/notes-init.vim); the window draws the images over those rows
    public var vimImageFile: String?
    // show the standard window close button (red traffic light). By default
    // it's hidden on titled windows (Esc closes instead); set true to show it
    // and wire it to onCloseWindow.
    public var showCloseButton: Bool = false
    // themed ✕ glyph at the far left of the drag header (left of the app
    // icon): closes the window the same way Esc does
    public var headerCloseButton: Bool = true

    // visible search bar: the query field gets a rounded background and a
    // placeholder, so the window clearly reads as "type to filter"
    public var showSearchBar: Bool = false
    // search bar width as a fraction of the window width
    public var searchWidthFraction: CGFloat = 0.8

    // filter bar: a row of dropdown pills below the search field. Host sets
    // filterLabels/filterValues/filterSelections; changing a selection fires
    // onFilterChange so the host can re-filter its rows.
    public var filters: Bool = false
    public var filterBarHeight: CGFloat = 26

    // drag header strip at the very top (like the note editor's): shows
    // chromeHeaderTitle and drags the window; clicks on it fire
    // onChromeHeaderClick. Search windows opt in for a titled look.
    public var dragHeader: Bool = false

    // tabs: a pill tab strip below the search field / drag header. Host sets
    // tabTitles + selectedTab; onTabChange tells it to swap content.
    public var tabs: Bool = false
    public var tabBarHeight: CGFloat = 30
    // a trailing "+" pill on the tab strip that fires onAddTab (e.g. create a
    // new note pad)
    public var tabsAddButton: Bool = false

    // scrollable rows: the row list lives in a scroll view, the window height
    // is capped at maxHeight, and overflowing rows scroll instead of clipping.
    // dynamicHeight is ignored when this is on.
    public var scrollableRows: Bool = false

    // click a row to select/highlight it without accepting
    public var clickToSelect: Bool = false

    // selectable rows: a checkbox is drawn at the left of every row; clicking
    // it toggles the row into the copy selection (PopupWindow.selectedIndices).
    // The header grows a "copy all" / "copy N" button that fires onCopyRows
    // with the picked rows (all rows when nothing is ticked).
    public var selectableRows: Bool = false
    // false = keep the checkboxes but drop the header "copy selected" button
    // (the host offers the copy through an action picker, e.g. Cmd+K)
    public var copyRowsButton: Bool = true
    // a ☆ bookmark toggle right of each row's checkbox (filled when
    // PopupRow.starred); a click fires PopupWindow.onToggleStar
    public var rowStars: Bool = false
    // where row content starts: padding + the checkbox / ☆ columns (rows,
    // table header and hit-testing all share it)
    public var rowLeadInset: CGFloat {
        padding + 10 + (selectableRows ? 22 : 0) + (rowStars ? 20 : 0)
    }

    // cap on how much a single row may stretch when the window is resized
    // larger than its content: filling a tall window with few rows would
    // otherwise leave huge empty gaps between the pills
    public var maxRowStretch: CGFloat = 26

    // highlight the query's matched characters in row titles/content
    // (fzf-style), using the current search text
    public var highlightMatches: Bool = false

    // table mode: rows render as spreadsheet cells under a sticky header
    // (click a sortable title to sort, drag a divider to resize). Empty =
    // the classic preview rows. Needs scrollableRows.
    public var tableColumns: [PopupTableColumn] = []
    public var tableHeaderHeight: CGFloat = 24
    // semantic color of one table cell (field, text) → tone; nil = plain
    public var tableCellTone: ((String, String) -> PopupTone?)?

    // optional font family for row/header/search/editor text (nil = system);
    // commands.conf `font` drives it so windows can look distinct
    public var fontName: String?

    // note windows: `![alt](rel)` in the file renders as an inline image and
    // pasted/dropped photos are saved by the host (imageSaver) + inserted as
    // attachments; saves serialize attachments back to markdown links
    public var markdownImages = false

    public func rowFont(_ size: CGFloat, weight: NSFont.Weight = .regular) -> NSFont {
        if let f = fontName, let n = NSFont(name: f, size: size) { return n }
        return NSFont.systemFont(ofSize: size, weight: weight)
    }

    // max wrapped lines a row's `body` may occupy (hosts can raise it for
    // longer multi-line previews; explicit \n breaks count as lines)
    public var bodyMaxLines: Int = 5

    public init(name: String) {
        self.name = name
    }
}

// MARK: - Row model

public protocol PopupRow {
    var title: String { get }
    var icons: [NSImage] { get }   // drawn after the title (optional)
    var trailing: String? { get }  // dim text after the icons (optional)
    var content: String? { get }   // with wrapContent: body text next to the
                                   // title (line 1, truncated); otherwise a
                                   // wrapped body under the title
    var detail: String? { get }    // with wrapContent: dim text on line 2 (left)
    var body: String? { get }      // with wrapContent: wrapped multi-line text
                                   // under line 2 (capped at 2 lines)
    var loadMore: Bool { get }     // synthetic "load next page" row
    var starred: Bool? { get }     // config.rowStars: filled ☆ / empty ☆ / nil = none
    // table mode: the text of one cell (config.tableColumns field)
    func cellText(_ field: String) -> String?
}

public extension PopupRow {
    var icons: [NSImage] { [] }
    var trailing: String? { nil }
    var content: String? { nil }
    var detail: String? { nil }
    var body: String? { nil }
    var loadMore: Bool { false }
    var starred: Bool? { nil }
    func cellText(_ field: String) -> String? { nil }
}

// One table column: `width` is a percent of the row's usable width (0 =
// share whatever the explicit widths leave over). Explicit widths adding up
// past 100 are scaled down to fit.
public struct PopupTableColumn {
    public var field: String
    public var title: String
    public var width: CGFloat
    public var align: NSTextAlignment
    public var sortable: Bool
    // the header shows a ▾ that fires PopupWindow.onTableFilter
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
    // (x, width) of every column across [x0, x0 + usable) — the header and
    // the rows share this so cells always sit under their titles
    func frames(x0: CGFloat, usable: CGFloat) -> [(x: CGFloat, w: CGFloat)] {
        guard !isEmpty, usable > 0 else { return map { _ in (x0, 0) } }
        let explicit = reduce(CGFloat(0)) { $0 + Swift.max(0, $1.width) }
        let autos = filter { $0.width <= 0 }.count
        let leftover = Swift.max(0, 100 - explicit)
        var pcts = map { $0.width > 0 ? $0.width : (autos > 0 ? leftover / CGFloat(autos) : 0) }
        // autos with no room left still get a sliver so they stay visible
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

// MARK: - Fuzzy search (generic, framework-level)

// Port of fzf's FuzzyMatchV2 (default scoring scheme, case-insensitive):
// Smith-Waterman-style DP over the pattern and text with word-boundary /
// camelCase / delimiter bonus points, so exact phrases and acronyms rank
// above lucky sparse matches. The query is split on whitespace into tokens
// (fzf semantics); every token must match, best combined score wins.
public enum PopupFuzzy {
    // --- scoring constants (from fzf's algo.go, default scheme) ---
    private static let scoreMatch: Int16 = 16
    private static let bonusBoundary: Int16 = scoreMatch / 2
    private static let bonusBoundaryWhite: Int16 = bonusBoundary + 2

    // One token's match against the text (all tokens must match; sum of
    // scores = overall score, total matched length for tiebreaking).
    // LESS PERMISSIVE: a token must appear as a CONTIGUOUS substring
    // (case-insensitive) — a typed word like "magazine" only matches rows
    // that actually contain "magazine", never letters scattered mid-word.
    // Matches at word boundaries are preferred, then earlier matches.
    private static func matchToken(_ token: [Character], _ text: [Character])
        -> (score: Int, length: Int, positions: [Int])? {
        let len = token.count
        guard len > 0, len <= text.count else { return nil }
        var bestStart = -1
        var bestScore = Int.min
        var i = 0
        while i + len <= text.count {
            var equal = true
            for k in 0..<len where text[i + k] != token[k] {
                equal = false
                break
            }
            if equal {
                let isWordStart = i == 0 || !(text[i - 1].isLetter || text[i - 1].isNumber)
                let sc = Int(isWordStart ? bonusBoundaryWhite : 0) - i / 8
                if sc > bestScore {
                    bestScore = sc
                    bestStart = i
                }
            }
            i += 1
        }
        guard bestStart >= 0 else { return nil }
        let positions = (0..<len).map { bestStart + $0 }
        return (Int(bestScore) + Int(scoreMatch) * len, len, positions)
    }

    private static func tokens(of query: String) -> [String] {
        query.lowercased()
            .split(whereSeparator: { $0.isWhitespace })
            .map(String.init)
    }

    // Total score of the query against the text; nil = not a match.
    public static func score(_ query: String, against text: String) -> Double? {
        let ts = tokens(of: query)
        guard !ts.isEmpty else { return 0 }
        let t = Array(text.lowercased())
        var total = 0
        for tok in ts {
            guard let m = matchToken(Array(tok), t) else { return nil }
            total += m.score
        }
        return Double(total)
    }

    // Filter rows by fzf score against their searchable text, best first
    // (score desc, then shorter total match, then input order — fzf's
    // default tiebreaks). Empty query returns everything unchanged.
    public static func filter<T>(_ rows: [T], query: String,
                                 search: (T) -> String) -> [T] {
        let q = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !q.isEmpty else { return rows }
        let ts = tokens(of: q)
        let textCache = rows.map { Array(search($0).lowercased()) }
        let scored: [(T, Int, Int)] = rows.enumerated().compactMap { i, row in
            var total = 0
            var length = 0
            for tok in ts {
                guard let m = matchToken(Array(tok), textCache[i]) else { return nil }
                total += m.score
                length += m.length
            }
            return (row, total, length)
        }
        return scored
            .sorted { a, b in
                if a.1 != b.1 { return a.1 > b.1 }
                if a.2 != b.2 { return a.2 < b.2 }
                return false
            }
            .map { $0.0 }
    }

    // Character ranges of the query's matched characters within `text`
    // (adjacent matches merged). nil = no match.
    public static func matchRanges(_ query: String, against text: String) -> [NSRange]? {
        let ts = tokens(of: query)
        guard !ts.isEmpty else { return [] }
        let t = Array(text.lowercased())
        var hits: [NSRange] = []
        for tok in ts {
            guard let m = matchToken(Array(tok), t) else { return nil }
            for p in m.positions {
                hits.append(NSRange(location: p, length: 1))
            }
        }
        // merge adjacent single-character matches into runs
        var merged: [NSRange] = []
        for r in hits.sorted(by: { $0.location < $1.location }) {
            if let last = merged.last, NSMaxRange(last) == r.location {
                merged[merged.count - 1] = NSRange(location: last.location,
                                                   length: last.length + 1)
            } else {
                merged.append(r)
            }
        }
        return merged
    }
}

// MARK: - Panel

// Borderless windows can't become key by default; without this the popup
// never gets focus (no caret, no keyboard input). Two window flavors:
//   - PopupPanel (NSPanel): borderless popups like the workspace switcher.
//     NSPanel is REQUIRED here — a plain NSWindow can't become key while
//     another app is frontmost, and the switcher is toggled from other apps.
//   - PopupPlainWindow (NSWindow): titled note/list windows. NSWindow is
//     REQUIRED here so the AX subrole is AXStandardWindow — NSPanels always
//     report AXSystemDialog, which AeroSpace's isWindowHeuristic rejects.
public protocol EscapableWindow: AnyObject {
    var onEscape: (() -> Void)? { get set }
}

public class PopupBaseWindow: NSWindow, EscapableWindow {
    public var onEscape: (() -> Void)?

    // themed text selection for every NSTextField in this window (search /
    // filter / find bars share the window's field editor)
    var selectionAttributes: [NSAttributedString.Key: Any]?
    public override func fieldEditor(_ createFlag: Bool, for object: Any?) -> NSText? {
        let ed = super.fieldEditor(createFlag, for: object)
        if let tv = ed as? NSTextView, let a = selectionAttributes {
            tv.selectedTextAttributes = a
            tv.insertionPointColor = a[.foregroundColor] as? NSColor ?? tv.insertionPointColor
        }
        return ed
    }
    // click-to-copy band at the top of the window: the invisible titlebar
    // swallows mouse events in its area, so clicks there never reach the
    // chrome — intercept them here instead. The click is fired WITHOUT
    // consuming the mouseUp: the titlebar must receive it to finish its
    // drag-tracking state, otherwise the next drag attempt jitters (two drag
    // systems fighting over the window). Drags (movement > 4pt) are ignored.
    var headerClickBand: CGFloat = 0
    var onHeaderClick: ((NSPoint) -> Void)?   // click point in window coords
    private var headerDown: NSPoint?

    public override var canBecomeKey: Bool { true }
    public override var canBecomeMain: Bool { true }

    public override func sendEvent(_ event: NSEvent) {
        if headerClickBand > 0 {
            let loc = event.locationInWindow
            switch event.type {
            case .leftMouseDown:
                if loc.y >= frame.height - headerClickBand {
                    // store ABSOLUTE mouse position: window-relative coords
                    // don't change during a native titlebar drag, which would
                    // misclassify a drag as a click (copy fires on every drag)
                    headerDown = NSEvent.mouseLocation
                }
            case .leftMouseUp:
                if let down = headerDown {
                    headerDown = nil
                    let up = NSEvent.mouseLocation
                    if abs(up.x - down.x) < 4, abs(up.y - down.y) < 4 {
                        onHeaderClick?(loc)
                    }
                }
            default:
                break
            }
        }
        super.sendEvent(event)
    }

    public override func cancelOperation(_ sender: Any?) {
        onEscape?()  // Esc even when the input field isn't first responder
    }

    public override func mouseDown(with event: NSEvent) {
        // clicking anywhere on the popup focuses the search field
        if let field = contentView?.subviews.compactMap({ $0 as? NSTextField }).first {
            makeFirstResponder(field)
        }
        super.mouseDown(with: event)
    }
}

// Borderless variant (workspace switcher).
public final class PopupPanel: NSPanel, EscapableWindow {
    public var onEscape: (() -> Void)?

    // themed text selection for every NSTextField in this window (search /
    // filter / find bars share the window's field editor)
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
        onEscape?()  // Esc even when the input field isn't first responder
    }

    public override func mouseDown(with event: NSEvent) {
        // clicking anywhere on the popup focuses the search field
        if let field = contentView?.subviews.compactMap({ $0 as? NSTextField }).first {
            makeFirstResponder(field)
        }
        super.mouseDown(with: event)
    }
}

// Titled variant (note editor / list windows): the hidden titlebar exists
// purely so the Accessibility API reports an AX close button — AeroSpace's
// isWindowHeuristic excludes accessory apps whose windows have no close
// button, which would make the popups invisible to focus navigation.
public final class PopupPlainWindow: PopupBaseWindow {
    // titled windows carry the system's own rounded frame (16pt on macOS 26):
    // its rim + fill peeked out around our smaller rounded card. The window
    // server asks the window for its radius — answer with the card's.
    var cornerRadius: CGFloat = 9 { didSet { invalidateShadow() } }
    @objc func _cornerRadius() -> CGFloat { cornerRadius }
}

// decoration overlay (focus rings) that never takes clicks
final class PopupPassThroughView: NSView {
    override var isFlipped: Bool { true }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
}

// MARK: - Button style

// ONE look for every clickable pill in the app: note tabs, the "+" tab, the
// header button bar, file-browser pills, filter dropdowns and the app-icon
// menu button. Quiet translucent "ghost" surfaces on the dark card; hover
// lifts, press sinks, and the selected / on state is a cool tint of the
// border color with a crisp hairline — no heavy accent blocks.
enum ButtonState { case idle, hover, pressed, on, onHover }

// Live retheme (Theme ▸ presets): every view that holds its own PopupConfig
// copy takes the window's new palette and redraws — no window rebuild, so
// the shell session and the vim pane survive.
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
    // Soft, borderless surfaces. Idle / hover / pressed read from a ghost
    // fill derived from the text color (right contrast on light and dark
    // presets alike); the ACTIVE ("on") state wears the theme accent — a
    // tinted fill with accent text — so "what's selected" reads the same in
    // every window: pinned folder, applied filter, header toggle, sort key.
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
    // the active marker: the theme accent, lifted until it reads on the card
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
    // 2pt accent bar along the bottom edge of an active chip / selected tab
    static func indicator(_ rect: NSRect, _ c: PopupColors) {
        let inset = min(8, rect.width * 0.2)
        let bar = NSRect(x: rect.minX + inset, y: rect.maxY - 2.5,
                         width: max(4, rect.width - inset * 2), height: 2)
        accent(c).setFill()
        NSBezierPath(roundedRect: bar, xRadius: 1, yRadius: 1).fill()
    }
    // text inputs are RECESSED wells (the mantle tone, a hairline edge) —
    // the opposite of the raised buttons around them, so a field never
    // reads as a button. Focus swaps the hairline for the accent.
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

    // Text selection (Cmd+A, drag-select, find jumps) in EVERY text input.
    // AppKit's default follows the machine's accent color + light/dark mode,
    // so the same build rendered unreadable selections on some Macs. Pin it
    // to the theme: an opaque highlight that stands off the card, and a
    // foreground picked for contrast against that highlight.
    static func selection(_ c: PopupColors) -> [NSAttributedString.Key: Any] {
        let card = opaque(c.background)
        var bg = opaque(c.highlight)
        // a highlight too close to the card is invisible — lift it toward
        // the text color until it reads as a selection
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
    // WCAG contrast ratio (1 … 21)
    static func contrast(_ a: NSColor, _ b: NSColor) -> CGFloat {
        let la = luminance(a), lb = luminance(b)
        return (max(la, lb) + 0.05) / (min(la, lb) + 0.05)
    }
    // `preferred` when it's comfortably readable on `bg`, else black/white
    static func readable(on bg: NSColor, preferred: NSColor) -> NSColor {
        if contrast(preferred, bg) >= 4.5 { return opaque(preferred) }
        return contrast(.white, bg) >= contrast(.black, bg) ? .white : .black
    }

    // surface: squared-off rounded rect; the on state adds the accent bar
    // `flat`: no surface at rest (tabs, icon buttons) — it appears on hover
    // (callers are flipped views, so the bar lands on the visual bottom)
    static func draw(_ rect: NSRect, _ st: ButtonState, _ c: PopupColors, radius: CGFloat,
                     flat: Bool = false, indicator showBar: Bool = false) {
        if flat && st == .idle { return }
        let r = rect.insetBy(dx: 0.5, dy: 0.5)
        let path = NSBezierPath(roundedRect: r, xRadius: radius, yRadius: radius)
        fill(st, c).setFill()
        path.fill()
        if showBar, st == .on || st == .onHover { indicator(r, c) }
    }

    // SF Symbol tinted to `color`, centered in `rect` (sharper than text
    // glyphs like ★ / ←)
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

    // centered label (optionally within a sub-rect)
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

    // crisp vector glyphs (text "+"/"✕" render blurry and off-center)
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
    // small "▾" menu chevron (flipped coordinates)
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

// MARK: - Backdrop (rounded container, optional resize via edges/corners)

final class PopupBackdrop: NSView {
    struct Edge: OptionSet {
        let rawValue: Int
        static let left = Edge(rawValue: 1 << 0)
        static let right = Edge(rawValue: 1 << 1)
        static let top = Edge(rawValue: 1 << 2)
        static let bottom = Edge(rawValue: 1 << 3)
    }

    let config: PopupConfig
    private var trackingArea: NSTrackingArea?
    private var startFrame: NSRect = .zero
    private var startPoint: NSPoint = .zero
    private var dragEdges: Edge = []
    private var draggingWindow = false

    private let minW: CGFloat = 120
    private let minH: CGFloat = 100
    private let hit: CGFloat = 8  // resize hit zone around edges/corners

    override var isFlipped: Bool { true }

    init(config: PopupConfig, frame: NSRect) {
        self.config = config
        super.init(frame: frame)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) not supported") }

    // Native (.resizable) windows: the outer band belongs to the window's
    // resize handling. Without this, whatever sits flush against an edge
    // (the file browser's scroller, the terminal) swallows the click and
    // the edge can't be grabbed there.
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

    private func edges(at p: NSPoint) -> Edge {
        var e: Edge = []
        if p.x <= hit { e.insert(.left) }
        if p.x >= bounds.width - hit { e.insert(.right) }
        if p.y <= hit { e.insert(.top) }
        if p.y >= bounds.height - hit { e.insert(.bottom) }
        return e
    }

    private func cursor(for e: Edge) -> NSCursor {
        switch e {
        case [.left, .right]: return .resizeLeftRight
        case [.top, .bottom]: return .resizeUpDown
        case [.top, .left], [.bottom, .right]: return .resizeLeftRight  // diagonal-ish
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
        // native (.resizable) windows get their cursors from the system
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
            dragEdges = e
            startFrame = win.frame
            startPoint = NSEvent.mouseLocation   // absolute screen coords
            return
        }
        if config.enableDrag, let win = window {
            // dragging anywhere else on the frame moves the window (text
            // views/rows consume their own drags for selection). Use the
            // native drag so the hidden titlebar doesn't fight it.
            win.performDrag(with: event)
            return
        }
        super.mouseDown(with: event)  // let the panel focus the field
    }

    override func mouseDragged(with event: NSEvent) {
        guard let win = window else { return }
        let m = NSEvent.mouseLocation   // absolute screen coords, not
        let dx = m.x - startPoint.x     // locationInWindow — window-relative
        let dy = m.y - startPoint.y     // deltas collapse once the window
                                        // catches up with the mouse (jitter)
        if !dragEdges.isEmpty {
            var f = startFrame
            var w = f.width
            var h = f.height
            if dragEdges.contains(.right) {
                w = max(minW, startFrame.width + dx)
            } else if dragEdges.contains(.left) {
                let nw = max(minW, startFrame.width - dx)
                f.origin.x += startFrame.width - nw
                w = nw
            }
            if dragEdges.contains(.top) {
                h = max(minH, startFrame.height + dy)
            } else if dragEdges.contains(.bottom) {
                let nh = max(minH, startFrame.height - dy)
                f.origin.y += startFrame.height - nh
                h = nh
            }
            f.size = NSSize(width: w, height: h)
            win.setFrame(clampToScreen(f), display: true)
            win.invalidateShadow()
        } else if draggingWindow {
            win.setFrameOrigin(NSPoint(x: startFrame.origin.x + dx,
                                       y: startFrame.origin.y + dy))
            win.invalidateShadow()
        }
    }

    override func mouseUp(with event: NSEvent) {
        dragEdges = []
        draggingWindow = false
        super.mouseUp(with: event)
    }

    // macOS-style resize grip drawn in the bottom-right corner.
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

// MARK: - Tab strip

// Horizontal pill tab bar (notepad-style). Titles are drawn as pills; the
// selected tab is highlighted. With many tabs the pills WRAP to the next row
// (never hidden); the "+" add button comes first. Clicking fires onSelect.
// A tab's status badge (e.g. jira poll freshness): a colored dot before the
// title, short dim text after it ("12m"), and a hover tooltip.
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
    // parallel to titles (nil = no badge)
    var badges: [PopupTabBadge?] = [] {
        didSet { needsDisplay = true }
    }
    private func badge(_ i: Int) -> PopupTabBadge? { badges.indices.contains(i) ? badges[i] : nil }
    private var badgeFont: NSFont { .systemFont(ofSize: max(9, config.buttonFontSize * zoom - 1.5), weight: .medium) }
    private var dotW: CGFloat { 7 * zoom }
    // extra width a badge adds to its pill: dot + gap, and " text"
    private func badgeWidth(_ i: Int) -> CGFloat {
        guard let b = badge(i) else { return 0 }
        let tw = b.text.isEmpty ? 0 : (b.text as NSString).size(withAttributes: [.font: badgeFont]).width + 5 * zoom
        return dotW + 5 * zoom + tw
    }
    var selected = 0 {
        didSet { needsDisplay = true }
    }
    var onSelect: ((Int) -> Void)?      // fired when a DIFFERENT tab is clicked
    var onClick: ((Int) -> Void)?       // fired for EVERY tab click (host uses
                                        // this for click-the-active-tab = copy)
    var onAddTab: (() -> Void)?         // fired when the "+" pill is clicked
    var onCloseTab: ((Int) -> Void)?    // fired when a tab's ✕ badge is clicked
    var onCopyPath: ((Int) -> Void)?    // right-click a tab -> copy its path
    private var tabH: CGFloat { 22 * zoom }
    // breathing room above/below the pills inside the (deeper) strip
    private var vpad: CGFloat { 4 * zoom }
    private let gap: CGFloat = 6
    private var addW: CGFloat { 24 * zoom }
    // ✕ close target at each tab's right end: shown on the selected tab and
    // on whichever tab the cursor is over
    private var closeSize: CGFloat { 16 * zoom }
    // index (into pillRects) of the pill under the cursor / of its ✕
    private var hoverIndex: Int?
    private var hoverCloseIndex: Int?
    private var pressedIndex: Int?
    private var trackingArea: NSTrackingArea?
    // solid strip behind the pills (PopupConfig.opaqueTabs); nil = see-through
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

    // hover tracking so the ✕ only pops in when the cursor sits over a pill's
    // top-left corner (it stays hidden otherwise, keeping tabs clean)
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
        var over: Int?
        var overClose: Int?
        for (i, (rect, title, close)) in pillRects().enumerated() where rect.contains(p) {
            over = i
            if title != "+", let close, close.insetBy(dx: -2, dy: -2).contains(p) { overClose = i }
            break
        }
        let tip = over.flatMap { i -> String? in
            let t = pillRects()[i].title
            return titles.firstIndex(of: t).flatMap { badge($0)?.tip }
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
        // 12pt lead-in + label + room for the ✕ at the right end
        return (title as NSString).size(withAttributes: attrs).width + 12 * zoom + 22 * zoom
            + badgeWidth(index)
    }

    // Number of wrapped rows the pills occupy at `width` (the "+" first).
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

    // Full height this bar needs so every pill is visible (wraps to rows).
    func heightNeeded(forWidth width: CGFloat) -> CGFloat {
        CGFloat(rowCount(forWidth: width)) * (tabH + gap) - gap + vpad * 2
    }

    // Wrapped layout of every pill; the "+" add button is first. Each tab pill
    // carries its ✕ badge rect (top-left corner), nil for the "+" button.
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
            out.append((rect, t, close))
            x += tw + gap
        }
        return out
    }

    override func draw(_ dirtyRect: NSRect) {
        let c = config.colors
        let radius = config.buttonRadius * zoom
        // the strip sits one level DEEPER than the card (mantle), like a
        // terminal theme's tab line; see-through windows keep it translucent
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
            // active tab: a SOLID accent pill with deep text (the current
            // window in a themed tmux status line); others stay flat
            let fg: NSColor
            if isSelectedTab {
                let pill = NSBezierPath(roundedRect: rect.insetBy(dx: 0.5, dy: 0.5),
                                        xRadius: radius, yRadius: radius)
                (hovered ? c.accentOn.blended(withFraction: 0.12, of: c.onAccent) ?? c.accentOn
                         : c.accentOn).setFill()
                pill.fill()
                fg = c.onAccent
            } else {
                ButtonStyle.draw(rect, st, c, radius: radius, flat: true)
                fg = ButtonStyle.text(st, c)
            }
            if title == "+" {
                ButtonStyle.plus(in: rect, color: fg, arm: 4.5 * zoom)
                continue
            }
            // label centered in the space left of the ✕ slot (so it never
            // shifts when the ✕ appears)
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
                // [● title age]: centered as one group in the label slot
                let battrs: [NSAttributedString.Key: Any] = [
                    .font: badgeFont, .foregroundColor: fg.withAlphaComponent(isSelectedTab ? 0.75 : 0.65)]
                let bs = b.text as NSString
                let bsz = b.text.isEmpty ? .zero : bs.size(withAttributes: battrs)
                let groupW = dotW + 5 * zoom + tsz.width + (b.text.isEmpty ? 0 : 5 * zoom + bsz.width)
                var x = labelRect.midX - groupW / 2
                let dot = NSRect(x: x, y: labelRect.midY - dotW / 2, width: dotW, height: dotW)
                c.tone(b.tone).setFill()
                NSBezierPath(ovalIn: dot).fill()
                if isSelectedTab {
                    // keep the hue readable on the solid accent pill
                    c.onAccent.withAlphaComponent(0.55).setStroke()
                    let ring = NSBezierPath(ovalIn: dot.insetBy(dx: -0.5, dy: -0.5))
                    ring.lineWidth = 1
                    ring.stroke()
                }
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
            // ✕ on the selected tab and the hovered one; its own hover disc
            if let close, isSelectedTab || hovered {
                if hoverCloseIndex == i {
                    (isSelectedTab ? c.onAccent.withAlphaComponent(0.18)
                                   : c.tone(.danger).withAlphaComponent(0.22)).setFill()
                    NSBezierPath(ovalIn: close).fill()
                }
                let xc = isSelectedTab ? c.onAccent.withAlphaComponent(hoverCloseIndex == i ? 1 : 0.7)
                    : hoverCloseIndex == i ? c.tone(.danger) : c.dim.withAlphaComponent(0.8)
                ButtonStyle.cross(in: close, color: xc, arm: 3 * zoom)
            }
        }
    }

    override func mouseUp(with event: NSEvent) {
        if pressedIndex != nil { pressedIndex = nil; needsDisplay = true }
        super.mouseUp(with: event)
    }

    override func mouseDown(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        // the ✕ badge has priority over selecting the tab
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

    // right-click a note tab -> "Copy Path" (the host dropped the dedicated
    // copy-path header button in favor of this)
    override func rightMouseDown(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        for (rect, title, _) in pillRects() where rect.contains(p) {
            guard title != "+", let i = titles.firstIndex(of: title) else { break }
            let item = NSMenuItem(title: "Copy Path",
                                  action: #selector(copyPath(_:)),
                                  keyEquivalent: "")
            item.target = self
            item.representedObject = i
            let menu = NSMenu()
            menu.addItem(item)
            NSMenu.popUpContextMenu(menu, with: event, for: self)
            return
        }
        super.rightMouseDown(with: event)
    }
    @objc private func copyPath(_ sender: NSMenuItem) {
        guard let i = sender.representedObject as? Int else { return }
        onCopyPath?(i)
    }
}

// MARK: - Filter bar

// Row of dropdown pills (one per filter dimension). Each pill shows
// "label: current", click opens an NSMenu with "All" + the unique values;
// picking one fires onSelect(dimension, valueIndex).
final class PopupFilterBar: NSView {
    var config: PopupConfig
    var zoom: CGFloat = 1.0
    var labels: [String] = []
    var values: [[String]] = []      // per dimension; index 0 = "All"
    // display titles parallel to `values` (empty = show the raw value) — lets
    // a dropdown show "13.1 (2026-10-15)" while matching on the raw "13.1"
    var valueLabels: [[String]] = []
    var selections: [Int] = []       // selected value index per dimension
    var onSelect: ((Int, Int) -> Void)?
    // host-driven mode (e.g. a searchable multi-select popover): a pill
    // click fires onOpen(dimension, pill rect) instead of the value menu;
    // summaries[dim] (non-empty) replaces the "label: value" text and
    // `active` marks the pills that narrow the rows
    var onOpen: ((Int, NSRect) -> Void)?
    var summaries: [String] = []
    var active: Set<Int> = []
    private var pillH: CGFloat { 22 * zoom }
    // joined segmented bar: segments touch (no gap) with thin | dividers
    private let sepW: CGFloat = 1
    private var segPad: CGFloat { 12 * zoom }
    // flash highlight after a selection change (dimension index, -1 = none)
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

    // display text for a dropdown option (labeled when valueLabels provides one)
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

    // Total width the bar needs with FULL (untruncated) titles — used to
    // decide whether segments must shrink to fit the window.
    func naturalWidth() -> CGFloat {
        var w: CGFloat = 0
        for i in 0..<labels.count {
            // + the vector chevron drawn after the label
            w += (currentTitle(i) as NSString).size(withAttributes: fontAttrs).width + segPad * 2 + 12
        }
        w += CGFloat(max(0, labels.count - 1)) * sepW
        return w + 2 * barInset
    }

    // shared left/right inset with the search field and the rows, so the
    // whole window reads as one aligned column
    private var barInset: CGFloat { config.padding + 10 }

    private func pillRects() -> [(NSRect, Int, String)] {
        // FULL labels always: segments keep their natural width and the
        // WINDOW grows when the bar needs more room (see growWidthToContent)
        // — clipping a label with an ellipsis is never acceptable here
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
            // label + dropdown chevron
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

    // selection change + flash so the change reads as an action
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

// MARK: - Search field cell

// Field that draws its placeholder CENTERED while keeping the caret and typed
// text LEFT-aligned. (Flipping the field's alignment on the first keystroke
// made the caret jump and dropped fast typing.)
final class PopupSearchFieldCell: NSTextFieldCell {
    // left/right text inset inside the well (the browser's filter bar)
    var hInset: CGFloat = 0
    // The search bar is taller than the cell's natural height, and AppKit pins
    // a bezel-less cell's text/field-editor rect near the TOP of the control —
    // the caret and typed text then sit visibly high in the bar. Center every
    // text rect vertically so the caret lands on the bar's midline.
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
            // draw the centered placeholder ONLY when not being edited —
            // while editing, AppKit renders its own (left-aligned) copy
            // behind the field editor, which doubled up with ours
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

// MARK: - Row view

// The framework knows row geometry (rowHeight) but NOT what a row looks like —
// that's the host app's business. If onDrawRow is set, the app renders each
// row rect itself (full control: pill, icons, anything). Otherwise a minimal
// generic default (highlight pill + title) is drawn.
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
    // fired with the row index on a plain click (mouseUp without drag on the
    // same row), after the click-to-select selection update
    var onRowClick: ((Int) -> Void)?
    // double-click (native clickCount == 2) on the same row
    var onRowDoubleClick: ((Int) -> Void)?
    private var downIndex = -1
    private var downPoint = NSPoint.zero
    // where rows start inside this view (search field / tab strip above it,
    // or just a small padding when the view lives in a scroll view)
    var topInset: CGFloat = 0
    // extra space below the last row (scrollable lists) so the last pill can
    // always be scrolled into a fully-visible position
    var bottomInset: CGFloat = 0
    // current search text, used for fzf-style match highlighting
    var highlightQuery = ""
    // row count the window was last sized for. Rows only stretch to fill the
    // window when they match this count (i.e. the window was resized for the
    // current list); when filtering narrows the list inside a fixed-size
    // window, rows keep their natural height instead of ballooning.
    var sizingRowCount = 0
    // set once the user manually resized (or used Cmd+±): rows stretch to
    // fill the window from then on, and the scroll document fills the space
    var stretchToFill = false
    // rows ticked for copying (config.selectableRows); the host reads/writes
    // this through PopupWindow.selectedIndices
    var selected: Set<Int> = []
    var onToggleSelect: ((Int) -> Void)?
    var onToggleStar: ((Int) -> Void)?

    override var isFlipped: Bool { true }

    init(config: PopupConfig) {
        self.config = config
        super.init(frame: .zero)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) not supported") }

    // left inset of row text: the checkbox column is reserved when present,
    // so height measurement and drawing always agree
    private var contentX: CGFloat { config.rowLeadInset }

    // table geometry for a given view width (the header view uses the same)
    static func tableFrames(_ config: PopupConfig, width: CGFloat) -> [(x: CGFloat, w: CGFloat)] {
        let x0 = config.rowLeadInset
        return config.tableColumns.frames(x0: x0, usable: width - x0 - (config.padding + 10))
    }

    private func checkBoxRect(in band: NSRect) -> NSRect {
        let s: CGFloat = 13
        return NSRect(x: config.padding + 2, y: band.midY - s / 2, width: s, height: s)
    }

    // the ☆ right of the checkbox (config.rowStars)
    private func starRect(in band: NSRect) -> NSRect {
        let s: CGFloat = 14
        return NSRect(x: config.padding + 2 + (config.selectableRows ? 22 : 0) - 1,
                      y: band.midY - s / 2, width: s, height: s)
    }

    // the row's natural-height band inside its (possibly stretched) rect —
    // drawing and hit-testing must agree on it
    private func band(for index: Int) -> NSRect {
        let r = rect(for: index)
        let natural = heights(forWidth: bounds.width)[index]
        let h = min(r.height, natural)
        return NSRect(x: r.minX, y: r.minY + (r.height - h) / 2,
                      width: r.width, height: h)
    }

    override func draw(_ dirtyRect: NSRect) {
        // Rows stretch to fill extra vertical space ONLY after the window was
        // manually resized for this exact row count (stretchToFill). Filtering
        // to fewer rows inside a fixed-size window keeps natural row heights
        // unless the user resized — then rows always fill the space. The
        // bottom inset is always reserved so the last pill never clips.
        let extra = stretchExtra()
        let hs = heights(forWidth: bounds.width)
        var y: CGFloat = topInset
        // visible-band culling: with hundreds of rows, only draw what's on
        // screen (heights are cached, so scanning is cheap)
        for (i, row) in rows.enumerated() {
            let h = hs[i] + extra
            if y + h > 0, y < bounds.height {
                let rect = NSRect(x: 0, y: y, width: bounds.width, height: h)
                if let onDrawRow {
                    onDrawRow(rect, row, i == selection)
                } else {
                    // hs[i] is the row's NATURAL height: the highlight pill
                    // hugs it instead of ballooning with a stretched row
                    drawDefault(row, in: rect, natural: hs[i], index: i,
                                isSel: i == selection)
                }
            }
            y += h
        }
    }

    // Stretch amount added to every row when the user resized the window
    // (stretchToFill). Kept in ONE place so drawing and the scroll-follow
    // geometry always agree — otherwise the focused pill gets cut off.
    private func stretchExtra() -> CGFloat {
        // scrollable lists keep a FIXED height with native scrolling: rows
        // always sit at their natural height (empty space below when few),
        // so pills never gain stretch gaps and never jump between states
        guard stretchToFill, !config.scrollableRows,
              rows.count == sizingRowCount, rows.count > 0 else { return 0 }
        let hs = heights(forWidth: bounds.width)
        let minTotal = topInset + hs.reduce(0, +)
        let usable = max(0, bounds.height - bottomInset)
        let slack = max(0, usable - minTotal)
        // cap the per-row stretch: with few rows in a tall window, filling all
        // the slack would put huge empty gaps between the pills (the pills
        // keep their natural height and center in the band)
        return min(slack / CGFloat(rows.count), config.maxRowStretch * zoom)
    }

    // Height of one row: fixed rowHeight, or (with wrapContent) a preview:
    //   title block — key + content on line 1, content WRAPS to as many lines
    //   as needed (never truncated)
    //   meta line — detail (left) + trailing (right)
    //   body      — wrapped, capped at bodyMaxLines
    // Extra bottom padding so the meta text clears the pill.
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
            // measure at the EXACT width drawText draws the body at
            // (textW = rect.width - contentX - padding - 6, see drawDefault)
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

    // Rendered height of the title block (key + wrapped content), measured
    // with the same line-height math the drawing uses so nothing clips.
    private func titleBlockHeight(_ row: PopupRow, font: NSFont, rowW: CGFloat) -> CGFloat {
        CGFloat(titleBlockLines(row, font: font, rowW: rowW)) * probeLineH(font)
    }

    private func probeLineH(_ font: NSFont) -> CGFloat {
        ("Ag" as NSString).boundingRect(
            with: NSSize(width: 1000, height: CGFloat.greatestFiniteMagnitude),
            options: [.usesLineFragmentOrigin, .usesFontLeading],
            attributes: [.font: font]).height
    }

    // Lines the title block takes: key on line 1 with the content wrapping
    // after it. Content ALWAYS wraps — never truncated — so long titles stay
    // fully visible. Measured at the EXACT width the content is drawn at
    // (next to the key, see drawDefault), so the wrapped count always
    // matches the render and the title is never clipped.
    private func titleBlockLines(_ row: PopupRow, font: NSFont, rowW: CGFloat) -> Int {
        guard let content = row.content, !content.isEmpty else { return 1 }
        let attrs: [NSAttributedString.Key: Any] = [.font: font]
        let probeH = probeLineH(font)
        let keyW = (row.title as NSString).size(withAttributes: attrs).width
        // drawDefault draws the content in ONE rect next to the key:
        //   x = padding+10, width = rect.width - (x + keyW) - padding - 12
        //   rowW = rect.width - 2*(padding+10)  =>  drawnWidth = rowW - keyW - 2
        let drawnW = max(40, rowW - keyW - 2)
        let fullH = (content as NSString).boundingRect(
            with: NSSize(width: drawnW, height: .greatestFiniteMagnitude),
            options: [.usesLineFragmentOrigin, .usesFontLeading], attributes: attrs).height
        return max(1, Int(ceil(fullH / probeH)))
    }

    // Y-rect of a row, for scroll-into-view math. Stretch-aware: when rows are
    // stretched to fill the window, the rect matches the DRAWN height so the
    // focused pill is never positioned half off-screen.
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

    // Total height the window needs for the current rows (inset + all rows).
    // Measured at the LIVE document width (bounds.width) — after a width
    // resize the wrap changes, and a stale width would leave the document
    // frame shorter than the drawn rows, cutting off the last pill.
    func contentHeight() -> CGFloat {
        var h = topInset
        for rowH in heights(forWidth: bounds.width) {
            h += rowH
        }
        return h + config.padding + bottomInset
    }

    // click-to-select/accept: select + fire onRowClick when the mouseUp lands on
    // the same row it went down on, without dragging
    override func mouseDown(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        // the checkbox column toggles the copy selection instead of
        // selecting/accepting the row
        if config.selectableRows {
            let i = rowIndex(at: p)
            if i >= 0, checkBoxRect(in: band(for: i)).contains(p) {
                onToggleSelect?(i)
                downIndex = -1
                return
            }
        }
        if config.rowStars {
            let i = rowIndex(at: p)
            if i >= 0, rows.indices.contains(i), !rows[i].loadMore, rows[i].starred != nil,
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

    // Cached per-row heights so scrolling/filtering thousands of rows doesn't
    // re-measure every string on every frame. Invalidated implicitly when the
    // row count or width changes.
    private var cachedHeights: [CGFloat] = []
    private var cacheWidth: CGFloat = 0

    // the cache is keyed on (row count, width) ONLY — same-count row
    // replacements (filter reloads, json refreshes) and zoom changes must
    // drop it or pills/rows render at stale heights and jump
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


    // Minimal built-in rendering for hosts that don't customize rows.
    //
    // Default (single line): title + icons + trailing right.
    // Preview (wrapContent, two lines):
    //   line 1: title [content…]           (content truncated to the width)
    //   line 2: detail (dim, left)         trailing (dim, right)
    private func drawDefault(_ row: PopupRow, in rect: NSRect, natural: CGFloat,
                             index: Int, isSel: Bool) {
        // A stretched row (window resized / Cmd+± with few rows) gets a tall
        // band; the highlight pill and the content stay at the row's NATURAL
        // height, centered in the band, so a single search hit never paints a
        // rectangle over the whole results pane.
        let bandH = min(rect.height, natural)
        let top = rect.minY + (rect.height - bandH) / 2
        let band = NSRect(x: rect.minX, y: top, width: rect.width, height: bandH)
        if isSel {
            // generous, even margins: the pill breathes inside the row band
            // and the stroke is a hairline, not a 2pt outline
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
            // accent edge on the left: the cursor mark, same as the file list
            NSGraphicsContext.current?.saveGraphicsState()
            p.addClip()
            config.colors.accentOn.setFill()
            NSRect(x: pill.minX, y: pill.minY, width: 3, height: pill.height).fill()
            NSGraphicsContext.current?.restoreGraphicsState()
        }
        if config.selectableRows, !row.loadMore || config.tableColumns.isEmpty {
            drawCheckBox(checkBoxRect(in: band), on: selected.contains(index))
        }
        if config.rowStars, !row.loadMore, let on = row.starred {
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
            // title block: key + content on line 1, content wraps to as many
            // lines as needed (never truncated); matched query characters are
            // highlighted (fzf-style) when config.highlightMatches is on
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
            // body: word-wrapped, clipped at the row's body height (bodyMaxLines) —
            // long lines span naturally, never truncated with an ellipsis
            if let body = row.body, !body.isEmpty {
                let textW = rect.width - x - config.padding - 6
                // body draw starts at y + lineH + 2, so the rect must span from
                // THERE to the band's bottom (y is absolute): exactly the
                // bodyMaxLines the height math reserved — no 6th line poking
                // under the focused pill
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

    // Table row: one truncated, aligned cell per column (fzf highlight on),
    // a faint divider under the row. A "load more" row spans the table.
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
        let frames = PopupRowView.tableFrames(config, width: band.width)
        for (i, col) in config.tableColumns.enumerated() where i < frames.count {
            let f = frames[i]
            guard f.w > 8 else { continue }
            let text = row.cellText(col.field) ?? ""
            guard !text.isEmpty else { continue }
            // one line per cell: newlines (descriptions) collapse to spaces
            let flat = text.replacingOccurrences(of: "\n", with: " ")
            // semantic cell color (host: status → success/info, priority →
            // danger/warning, key → accent2 …) mapped onto the live palette
            let tone = config.tableCellTone?(col.field, flat)
            let color = tone.map { config.colors.tone($0) }
                ?? (i == 0 ? config.colors.text : config.colors.text.withAlphaComponent(0.92))
            let cellFont = (tone == .accent2 || i == 0)
                ? NSFontManager.shared.convert(font, toHaveTrait: .boldFontMask) : font
            let cellRect = NSRect(x: f.x + 3, y: y, width: f.w - 8, height: lineH)
            if let tone, [.success, .warning, .danger, .info, .accent].contains(tone) {
                // status-ish cells get a leading dot in their hue
                let dot: CGFloat = 6 * zoom
                color.setFill()
                NSBezierPath(ovalIn: NSRect(x: cellRect.minX, y: band.midY - dot / 2,
                                            width: dot, height: dot)).fill()
                drawText(flat, in: NSRect(x: cellRect.minX + dot + 5 * zoom, y: y,
                                          width: max(0, cellRect.width - dot - 5 * zoom), height: lineH),
                         font: cellFont, baseColor: color, accent: config.colors.tone(.accent2),
                         wrap: false, highlight: highlight, align: col.align)
                continue
            }
            drawText(flat, in: cellRect,
                     font: cellFont, baseColor: color, accent: config.colors.tone(.accent2),
                     wrap: false, highlight: highlight, align: col.align)
        }
        let line = NSBezierPath()
        line.move(to: NSPoint(x: contentX, y: band.maxY - 0.5))
        line.line(to: NSPoint(x: band.maxX - config.padding - 10, y: band.maxY - 0.5))
        config.colors.hairline.setStroke()
        line.lineWidth = 1
        line.stroke()
    }

    // ☆ bookmark: a faint outline until pinned, then a solid star in the
    // palette's warning (gold) hue
    private func drawStar(_ r: NSRect, on: Bool) {
        let p = NSBezierPath()
        let c = NSPoint(x: r.midX, y: r.midY + 0.5)
        let outer = r.width / 2, inner = outer * 0.45
        for k in 0..<10 {
            // flipped view: start at the top point (-90°)
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

    // Rounded checkbox: dim outline when unticked, filled + check mark when in
    // the copy selection.
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

    // text with optional fzf-style match highlighting: matched query characters
    // are drawn in the accent color
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
        // clip to the rect: wrapped lines that measure/draw off-by-one must
        // never bleed into the row below (e.g. huge descriptions)
        NSGraphicsContext.current?.saveGraphicsState()
        NSBezierPath(rect: rect).addClip()
        attr.draw(with: rect, options: [.usesLineFragmentOrigin, .usesFontLeading])
        NSGraphicsContext.current?.restoreGraphicsState()
    }
}

// MARK: - Table header

// Sticky column header for table-mode lists (a floating subview of the row
// scroll view, so it never scrolls away). Click a sortable title to sort
// (click again to flip the direction); drag the gap between two titles to
// resize them — the pair trades width, so the table total never changes.
final class PopupTableHeaderView: NSView {
    var config: PopupConfig
    var zoom: CGFloat = 1 { didSet { needsDisplay = true } }
    var sortColumn: Int? { didSet { needsDisplay = true } }
    var sortAscending = true { didSet { needsDisplay = true } }
    var onSort: ((Int) -> Void)?
    // filterable columns: the ▾ at a title's right end (or a right-click on
    // the title) fires onFilter(column, its rect); filtered columns wear
    // the accent
    var activeFilters: Set<Int> = [] { didSet { needsDisplay = true } }
    var onFilter: ((Int, NSRect) -> Void)?
    // effective percent widths after a drag; final = true on mouseUp
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

    // the ▾ filter target at the right end of a filterable column
    private func filterRect(_ i: Int) -> NSRect? {
        guard config.tableColumns.indices.contains(i), config.tableColumns[i].filterable,
              frames.indices.contains(i), frames[i].w > 30 else { return nil }
        let f = frames[i]
        let s: CGFloat = 16
        return NSRect(x: f.x + f.w - s - 5, y: bounds.midY - s / 2, width: s, height: s)
    }

    // the grab zone of the divider after column i (every column but the last)
    private func dividerRect(_ i: Int) -> NSRect {
        let f = frames[i]
        return NSRect(x: f.x + f.w - 4, y: 0, width: 8, height: bounds.height)
    }

    override func resetCursorRects() {
        let n = config.tableColumns.count
        for i in 0..<n {
            if let fr = filterRect(i) { addCursorRect(fr, cursor: .pointingHand) }
        }
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
        if over != hoverFilter {
            hoverFilter = over
            toolTip = over.map { "Filter \(config.tableColumns[$0].title)" }
            needsDisplay = true
        }
    }
    override func mouseExited(with event: NSEvent) {
        if hoverFilter != nil { hoverFilter = nil; needsDisplay = true }
    }

    // right-click a filterable title = its filter
    override func rightMouseDown(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        if let i = frames.firstIndex(where: { p.x >= $0.x && p.x < $0.x + $0.w }),
           let fr = filterRect(i) {
            onFilter?(i, fr)
            return
        }
        super.rightMouseDown(with: event)
    }

    override func draw(_ dirtyRect: NSRect) {
        let c = config.colors
        // one level deeper than the rows (mantle), opaque: rows scroll under it
        c.mantle.setFill()
        bounds.fill()
        let base = config.rowFont(config.rowFontSize * zoom * 0.86)
        let font = NSFontManager.shared.convert(base, toHaveTrait: .boldFontMask)
        let lineH = font.ascender + abs(font.descender) + font.leading
        let fs = frames
        for (i, col) in config.tableColumns.enumerated() where i < fs.count {
            let f = fs[i]
            // small caps-style titles; the sorted column wears the accent
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
                // ▾: solid accent chip while a filter narrows this column
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
        if n > 1, let i = (0..<(n - 1)).first(where: { dividerRect($0).contains(p) }) {
            drag = (i, p.x, frames.map { $0.w / usable * 100 })
        } else {
            drag = nil
        }
    }

    override func mouseDragged(with event: NSEvent) {
        guard let d = drag else { return }
        let p = convert(event.locationInWindow, from: nil)
        moved = true
        var pcts = d.start
        let minPct: CGFloat = 3
        var delta = (p.x - d.startX) / usable * 100
        delta = max(minPct - pcts[d.divider], min(delta, pcts[d.divider + 1] - minPct))
        pcts[d.divider] += delta
        pcts[d.divider + 1] -= delta
        onResize?(pcts.map { ($0 * 10).rounded() / 10 }, false)
    }

    override func mouseUp(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        if let d = drag {
            drag = nil
            if moved {
                var pcts = d.start
                let minPct: CGFloat = 3
                var delta = (p.x - d.startX) / usable * 100
                delta = max(minPct - pcts[d.divider], min(delta, pcts[d.divider + 1] - minPct))
                pcts[d.divider] += delta
                pcts[d.divider + 1] -= delta
                onResize?(pcts.map { ($0 * 10).rounded() / 10 }, true)
                window?.invalidateCursorRects(for: self)
            }
            return
        }
        guard abs(p.x - downX) < 5 else { return }
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

// Helper for drawing NSImages in a flipped (row) context — NSImage.draw(in:)
// mirrors vertically there, so flip the CTM around the rect's center first.
public func popupDrawImage(_ img: NSImage, in rect: NSRect) {
    guard let ctx = NSGraphicsContext.current else { return }
    ctx.saveGraphicsState()
    let t = NSAffineTransform()
    t.translateX(by: 0, yBy: rect.origin.y * 2 + rect.height)
    t.scaleX(by: 1, yBy: -1)
    t.concat()
    img.draw(in: rect, from: .zero, operation: .sourceOver, fraction: 1)
    ctx.restoreGraphicsState()
}

// MARK: - Editor text view (image paste / drop)

// NSTextView that hands pasted or dropped IMAGES to the host instead of
// inserting Apple's RTF garbage: clipboard photos (screenshots, copied
// images) and Finder-copied image files both arrive as a clean NSImage.
final class PopupTextView: NSTextView {
    var onPasteImage: ((NSImage) -> Void)?
    // fired after any user-initiated text change (typing / paste / delete) —
    // lets the host react live (e.g. prettyprint auto-format)
    var onTextChange: (() -> Void)?
    // character index -> absolute path of the image under it (nil = no image)
    var absolutePathAt: ((Int) -> String?)?
    private static let imageExts = Set(["png", "jpg", "jpeg", "gif", "heic", "webp", "tif", "tiff"])

    // The system color panel can deliver `changeColor:` up the responder
    // chain to the first responder. The notes editor must never be restyled
    // by the color picker — its text color is theme-driven only.
    override func changeColor(_ sender: Any?) { }

    override func didChangeText() {
        super.didChangeText()
        onTextChange?()
    }

    // right-click on a rendered photo: copy its ABSOLUTE file path
    override func menu(for event: NSEvent) -> NSMenu? {
        let m = NSMenu()
        let pt = convert(event.locationInWindow, from: nil)
        let idx = characterIndexForInsertion(at: pt)
        if let path = absolutePathAt?(idx) {
            let item = NSMenuItem(title: "copy image path",
                                  action: #selector(copyImagePath(_:)),
                                  keyEquivalent: "")
            item.target = self
            item.representedObject = path
            m.addItem(item)
            m.addItem(.separator())
        }
        // right-click the note itself -> copy the note file's absolute path
        // (the host drops the dedicated header button in favor of this)
        if onCopyFilePath != nil {
            let fpath = NSMenuItem(title: "Copy File Path",
                                   action: #selector(copyFilePath(_:)),
                                   keyEquivalent: "")
            fpath.target = self
            m.addItem(fpath)
        }
        // "open a file at an exact path" — prompts for a path and opens it
        // in this note window as a tab
        let open = NSMenuItem(title: "Open file at path…",
                              action: #selector(openFileAtPath(_:)),
                              keyEquivalent: "")
        open.target = self
        m.addItem(open)
        return m
    }

    var onCopyFilePath: (() -> Void)?
    @objc private func copyFilePath(_ sender: NSMenuItem) {
        onCopyFilePath?()
    }

    var onOpenFileAtPath: (() -> Void)?
    @objc private func openFileAtPath(_ sender: NSMenuItem) {
        onOpenFileAtPath?()
    }

    @objc private func copyImagePath(_ sender: NSMenuItem) {
        guard let path = sender.representedObject as? String else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(path, forType: .string)
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

// MARK: - Embedded file browser

// A small theme-aware push button (pills, star, parent) drawn with the
// window colors so it fits the dark chrome instead of the system accent.
final class ThemeButton: NSView {
    private var config: PopupConfig
    var title: String { didSet { needsDisplay = true } }
    // optional SF Symbol drawn before the title (icon-only when title is "")
    var symbol: String? { didSet { needsDisplay = true } }
    // no surface at rest; the fill appears on hover (toolbar icon buttons)
    var flat = false
    // "on" state (e.g. pinned): rendered with the raised active fill so an
    // active toggle reads clearly against the idle buttons
    var isOn = false { didSet { needsDisplay = true } }
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

    // a trailing ▾ (vector) for buttons that open a menu
    var chevron = false { didSet { needsDisplay = true } }
    private static let iconW: CGFloat = 12, iconGap: CGFloat = 4, chevW: CGFloat = 12
    static let hPad: CGFloat = 9

    // symbol + label + chevron as one centered group
    private func contentWidth(_ t: String, _ st: ButtonState) -> CGFloat {
        let tw = t.isEmpty ? 0 : (t as NSString).size(withAttributes: [
            .font: ButtonStyle.font(config.buttonFontSize, st)]).width
        var w = tw
        if symbol != nil { w += Self.iconW + (t.isEmpty ? 0 : Self.iconGap) }
        if chevron { w += Self.chevW }
        return w
    }
    // the width this button needs for `title` (layout uses it so padding is
    // the same on every toolbar button, whatever its label)
    func fittingWidth(for t: String? = nil) -> CGFloat {
        let t = t ?? title
        if t.isEmpty && !chevron { return bounds.height > 0 ? bounds.height + 4 : 28 }
        return ceil(contentWidth(t, .on) + Self.hPad * 2)
    }

    override func draw(_ dirty: NSRect) {
        let st: ButtonState = down ? .pressed
            : isOn ? (hover ? .onHover : .on)
            : hover ? .hover : .idle
        ButtonStyle.draw(bounds, st, config.colors, radius: config.buttonRadius, flat: flat)
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

// AppKit push button in the popup button language, for plain AppKit forms
// (Jira Config): ghost fill + hairline, hover lift; `.primary` = a solid
// accent button (Save, Enable), `.danger` = danger-tinted (Delete, Stop).
// A drop-in for NSButton(title:target:action:) — target/action unchanged.
// palette the themed AppKit controls start with (a host window sets it
// before building its form; live changes go through applyColors)
enum PopupThemeDefaults {
    static var colors = PopupColors()
}

final class ThemedPushButton: NSButton, PopupThemeable {
    enum Role { case normal, primary, danger }
    var colors = PopupThemeDefaults.colors { didSet { needsDisplay = true } }
    var role: Role = .normal { didSet { needsDisplay = true } }
    private var hover = false
    private var tracking: NSTrackingArea?
    func applyColors(_ c: PopupColors) { colors = c }

    private var labelFont: NSFont {
        .systemFont(ofSize: controlSize == .small ? 11 : 12, weight: .semibold)
    }
    override var intrinsicContentSize: NSSize {
        let w = (title as NSString).size(withAttributes: [.font: labelFont]).width
        return NSSize(width: ceil(w) + (controlSize == .small ? 20 : 26),
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
        let r = bounds.insetBy(dx: 1, dy: 1)
        let path = NSBezierPath(roundedRect: r, xRadius: 6, yRadius: 6)
        var fg: NSColor
        switch role {
        case .primary:
            let a = c.accentOn
            (st == .pressed ? a.blended(withFraction: 0.18, of: .black) ?? a
             : st == .hover ? a.blended(withFraction: 0.12, of: .white) ?? a : a).setFill()
            path.fill()
            fg = c.onAccent
        case .danger:
            let d = c.tone(.danger)
            d.withAlphaComponent(st == .pressed ? 0.32 : st == .hover ? 0.24 : 0.15).setFill()
            path.fill()
            fg = d
        case .normal:
            ButtonStyle.fill(st == .idle ? .hover : st == .hover ? .pressed : .onHover, c).setFill()
            if st == .pressed { ButtonStyle.fill(.pressed, c).setFill() }
            path.fill()
            ButtonStyle.inputStroke(c).setStroke()
            path.lineWidth = 1
            path.stroke()
            fg = c.text
        }
        if !isEnabled { fg = fg.withAlphaComponent(0.4) }
        let attrs: [NSAttributedString.Key: Any] = [.font: labelFont, .foregroundColor: fg]
        let sz = (title as NSString).size(withAttributes: attrs)
        (title as NSString).draw(at: NSPoint(x: (bounds.midX - sz.width / 2).rounded(),
                                             y: (bounds.midY - sz.height / 2).rounded()),
                                 withAttributes: attrs)
    }
}

// NSPopUpButton drawn like ThemedPushButton + a vector chevron (the menu
// itself stays native). Pull-downs show their first item as the title.
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

// Table/sidebar row in the popup cursor language: highlight pill with an
// accent edge instead of the system-blue selection; group rows draw no
// floating background.
final class PopupTableRowView: NSTableRowView {
    var colors = PopupThemeDefaults.colors
    // zebra stripe (odd rows) in a faint text tint, not the system gray
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
    // keep cell text colors ours (no auto-inversion on the selection)
    override var interiorBackgroundStyle: NSView.BackgroundStyle { .normal }
}

// The browser's file list: custom-drawn rows (icon + name + size), click to
// select (preview), double-click/Return to open, arrows to move. Printable
// keys hand focus to the search field.
final class FileListPane: NSView {
    private var config: PopupConfig
    var rows: [PopupFileBrowser.Entry] = [] {
        didSet {
            if let h = hover, !rows.indices.contains(h) { hover = nil }
            needsDisplay = true
        }
    }
    var selection = 0 { didSet { needsDisplay = true } }
    private(set) var hover: Int?
    var onSelect: ((Int) -> Void)?
    var onOpen: ((Int) -> Void)?
    var onParent: (() -> Void)?
    var onFocusSearch: ((String) -> Void)?
    var onHover: ((Int?) -> Void)?
    var onCopyPath: ((Int) -> Void)?
    var onOpenInNotes: ((Int) -> Void)?
    var onOpenTerminal: ((Int) -> Void)?

    private let rowH: CGFloat = 22
    private static let iconSize: CGFloat = 16
    // right gutter for the size/date column: clears the overlay scroller
    private static let trailingInset: CGFloat = 16
    private var trackingArea: NSTrackingArea?

    init(config: PopupConfig) {
        self.config = config
        super.init(frame: .zero)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) not supported") }
    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { true }

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

    override func draw(_ dirty: NSRect) {
        let w = bounds.width
        // only the rows in the dirty rect (search results can be thousands)
        let first = max(0, Int(dirty.minY / rowH))
        let last = min(rows.count - 1, Int(dirty.maxY / rowH))
        guard first <= last else { return }
        for i in first...last {
            let e = rows[i]
            let r = rowRect(i)
            // the same cursor language as the list windows: a rounded
            // highlight pill with an accent edge; hover = a quiet surface
            let pillR = r.insetBy(dx: 4, dy: 1)
            if i == selection {
                let p = NSBezierPath(roundedRect: pillR, xRadius: config.buttonRadius,
                                     yRadius: config.buttonRadius)
                config.colors.highlight.setFill()
                p.fill()
                NSGraphicsContext.saveGraphicsState()
                p.addClip()
                config.colors.accentOn.setFill()
                NSRect(x: pillR.minX, y: pillR.minY, width: 3, height: pillR.height).fill()
                NSGraphicsContext.restoreGraphicsState()
            } else if let h = hover, h == i {
                ButtonStyle.fill(.hover, config.colors).setFill()
                NSBezierPath(roundedRect: pillR, xRadius: config.buttonRadius,
                             yRadius: config.buttonRadius).fill()
            }
            let icon = e.icon ?? NSWorkspace.shared.icon(forFile: e.path)
            var ir = r
            ir.origin.x += 10
            ir.size.width = Self.iconSize
            let img = icon
            NSGraphicsContext.saveGraphicsState()
            let clip = NSBezierPath(roundedRect: ir.insetBy(dx: 1, dy: (rowH - Self.iconSize) / 2),
                                    xRadius: 2, yRadius: 2)
            clip.addClip()
            img.draw(in: ir.insetBy(dx: 1, dy: (rowH - Self.iconSize) / 2))
            NSGraphicsContext.restoreGraphicsState()

            let nameAttrs: [NSAttributedString.Key: Any] = [
                .font: NSFont.systemFont(ofSize: 12, weight: .regular),
                .foregroundColor: i == selection ? config.colors.text : config.colors.text.withAlphaComponent(0.85),
            ]
            var tx = ir.maxX + 6
            if e.isDir && e.name != ".." { tx += 4 }   // folder emoji leading space kept small
            let name = e.name as NSString
            let avail = w - tx - 8 - (e.trailingWidth > 0 ? e.trailingWidth + Self.trailingInset + 4 : 0)
            name.draw(with: NSRect(x: tx, y: r.minY + (rowH - 15) / 2, width: max(20, avail), height: 15),
                      options: [.truncatesLastVisibleLine, .usesLineFragmentOrigin],
                      attributes: nameAttrs)
            if e.isDir && e.name != ".." {
                // folder mark after the name, in the theme's second hue
                let dirAttrs: [NSAttributedString.Key: Any] = [
                    .font: NSFont.systemFont(ofSize: 11, weight: .bold),
                    .foregroundColor: config.colors.tone(.accent2),
                ]
                let d = (e.name as NSString).size(withAttributes: nameAttrs)
                let g = "/" as NSString
                g.draw(at: NSPoint(x: tx + d.width + 2, y: r.minY + (rowH - 14) / 2), withAttributes: dirAttrs)
            }
            if e.trailingWidth > 0 {
                let szAttrs: [NSAttributedString.Key: Any] = [
                    .font: NSFont.systemFont(ofSize: 10, weight: .regular),
                    .foregroundColor: config.colors.dim,
                ]
                let s = e.trailingText as NSString
                s.draw(at: NSPoint(x: w - e.trailingWidth - Self.trailingInset, y: r.minY + (rowH - 12) / 2),
                       withAttributes: szAttrs)
            }
        }
    }

    override func mouseDown(with e: NSEvent) {
        let p = convert(e.locationInWindow, from: nil)
        let idx = Int(p.y / rowH)
        guard rows.indices.contains(idx) else { return }
        selection = idx
        onSelect?(idx)
        if e.clickCount >= 2 { onOpen?(idx) }
    }

    // right-click a row -> context menu: open in the notes window / copy the
    // absolute path / open in the default app / reveal in Finder
    override func rightMouseDown(with e: NSEvent) {
        let p = convert(e.locationInWindow, from: nil)
        let idx = Int(p.y / rowH)
        guard rows.indices.contains(idx) else { return }
        selection = idx
        onSelect?(idx)
        let menu = NSMenu()
        menu.addItem(menuItem("Open in Notes", #selector(rowOpenInNotes(_:)), idx))
        menu.addItem(menuItem("Copy Path", #selector(rowCopyPath(_:)), idx))
        menu.addItem(NSMenuItem.separator())
        menu.addItem(menuItem("Open", #selector(rowOpen(_:)), idx))
        menu.addItem(menuItem("Reveal in Finder", #selector(rowRevealInFinder(_:)), idx))
        menu.addItem(menuItem("Open Terminal Here", #selector(rowOpenTerminal(_:)), idx))
        NSMenu.popUpContextMenu(menu, with: e, for: self)
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
        switch e.keyCode {
        case 126:   // up
            selection = max(0, selection - 1)
            onSelect?(selection)
        case 125:   // down
            selection = min(max(0, rows.count - 1), selection + 1)
            onSelect?(selection)
        case 36:    // return
            onOpen?(selection)
        case 123:   // left — parent dir
            onParent?()
        case 124:   // right — open selection
            onOpen?(selection)
        case 53:    // escape — let the window handle it
            super.keyDown(with: e)
        default:
            if let chars = e.charactersIgnoringModifiers, !chars.isEmpty {
                onFocusSearch?(chars)
            } else {
                super.keyDown(with: e)
            }
        }
    }

    // move the selection from the filter bar (which doesn't own it)
    func moveSelection(_ delta: Int) {
        guard !rows.isEmpty else { return }
        selection = min(max(0, rows.count - 1), max(0, selection + delta))
        onSelect?(selection)
    }
}

// Search field inside the browser. Return/Up/Down are handled by the browser
// via the field's NSControlTextEditingDelegate (control(_:textView:doCommandBy:))
// — a plain keyDown override never fires while the field editor is active.
// vertically centered text (same cell as the search / find bars)
final class BrowserSearchField: NSTextField {
    override class var cellClass: AnyClass? {
        get { PopupSearchFieldCell.self }
        set {}
    }
}

// Drag handle between the file list and the preview pane in the browser.
// Dragging it left/right rebalances the split; the fraction is clamped so
// neither pane can be collapsed entirely.
final class PaneSplitter: NSView {
    var onFractionChange: ((CGFloat) -> Void)?
    override var isFlipped: Bool { true }
    override func resetCursorRects() {
        super.resetCursorRects()
        addCursorRect(bounds, cursor: .resizeLeftRight)
    }
    // Consume mouseDown so drag-anywhere (which moves the WINDOW) never gets
    // the event — a drag that starts on the splitter only rebalances the panes.
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

// A read-only, keyboard-driven file browser panel: toolbar (search + pin +
// up), a favorites pill row, a directory listing with a right-hand preview
// split. Reused by the floating "files" window (fills the content) and the
// notes window (bottom drawer, toggled like the terminal).

// SAX collector for DOCX word/document.xml: keeps <w:t> text and turns each
// <w:p> paragraph into a line break
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

final class PopupFileBrowser: NSView, NSTextFieldDelegate {
    var config: PopupConfig
    var onOpen: ((String) -> Void)?          // open a FILE in its default app
    var onDirChange: ((String) -> Void)?     // cwd changed (host labels)
    var onCopyDir: ((String) -> Void)?       // copy current dir path
    var onCopyPath: ((String) -> Void)?      // copy an arbitrary path (hover/right-click)
    var onOpenInNotes: ((String) -> Void)?   // right-click "Open in Notes"
    var onStatus: ((String) -> Void)?        // transient feedback line
    var onOpenTerminal: ((String) -> Void)?  // `term` in the filter / right-click
    var onSortChange: ((String, Bool) -> Void)?  // persist (sort key, descending)

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
        // what picking the key defaults to: newest / largest first
        var naturalDescending: Bool { self == .modified || self == .created || self == .size }
    }
    private var sortKey: SortKey
    private var sortDescending: Bool

    // What the filter bar currently means (see parseQuery)
    enum QueryMode {
        case all                         // empty: the cwd listing
        case terminal(String)            // `term [path]`: Enter opens a shell there
        case local(String)               // filter the cwd (substring or glob)
        case dir(String, String)         // a typed path: list that dir, filter by the tail
        case recursive(String, String)   // `**` / wildcard dirs: rg under base with glob
    }
    private var mode: QueryMode = .all
    // cwd incl. dotfiles, listed on demand for `.*` style filters
    private var hiddenAll: [Entry]?
    // last typed-path listing (dir, includesHidden, entries) — reused per keystroke
    private var dirCache: (String, Bool, [Entry])?
    // recursive search bookkeeping: a newer query bumps searchGen so stale
    // results are dropped; the debounce keeps rg from spawning per keystroke
    private var searchGen = 0
    private var searchProcess: Process?
    private var searchWork: DispatchWorkItem?

    private let favURL: URL
    // starred dirs (persisted to favURL) — the other two sources come from
    // commands.conf ([files] favorites + zoxide top-N)
    private var pinnedFavorites: [String] = []
    private let staticFavorites: [String]
    private let zoxideFavorites: [String]
    private var shownFavorites: [String] = []
    private(set) var cwd: String
    private var all: [Entry] = []
    private var rows: [Entry] = []
    private var selection = 0
    private var query = ""

    private let searchField = BrowserSearchField()
    private let parentButton: ThemeButton
    private let starButton: ThemeButton
    private let sortButton: ThemeButton
    private let orderButton: ThemeButton
    // dim one-line feedback under the list: match counts, "↵ cd …", search progress
    private let statusLine = NSTextField(labelWithString: "")
    private let listPane: FileListPane
    private let listScroll = NSScrollView()
    private let splitter = PaneSplitter()
    // list-pane share of the browser width (0.2-0.8); the splitter drags it
    private var splitFraction: CGFloat = 0.56
    private let previewScroll = NSScrollView()
    private let previewText = NSTextView()
    private let previewImage = NSImageView()
    private let previewHint = NSTextField(labelWithString: "")
    // folder preview: selecting a directory shows its contents as a REAL file
    // list (icons, hover, right-click Open in Notes / Copy Path) on the right
    private let previewList: FileListPane
    private let previewListScroll = NSScrollView()
    private var favPills: [ThemeButton] = []
    private static let imageExts = Set(["png", "jpg", "jpeg", "gif", "heic", "webp", "tif", "tiff", "pdf"])
    private static let textLimit = 262144

    // which part holds the keyboard: the filter bar, the list (left) or the
    // preview (right). The window's border says "the browser has focus";
    // this ring (plus a bright filter-bar outline) says WHERE inside it.
    enum FocusPart { case filter, list, preview }
    private let partRing = PopupPassThroughView()
    private var focusObservation: NSKeyValueObservation?
    private var keyObservers: [NSObjectProtocol] = []
    private(set) var focusedPart: FocusPart?

    // listPane needs to be focusable from the window (drawer toggle)
    var listView: FileListPane { listPane }
    // the search field is exposed so the window can route Cmd+V/C/A etc. to
    // the filter bar (otherwise they land in the notes editor / hidden field)
    var searchView: NSTextField { searchField }

    // live restyle from the color picker: swap the panel background without
    // rebuilding the browser (the color's own alpha sets the translucency)
    func setBackground(_ c: NSColor) {
        config.fileBrowserBackground = c
        layer?.backgroundColor = c.cgColor
        needsDisplay = true
    }

    // re-apply every cached color after a live theme change (the picker edits
    // config.colors on the window; the browser's fields cache colors at init)
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
         staticFavorites: [String] = [], zoxideFavorites: [String] = []) {
        self.config = config
        self.cwd = (startDir as NSString).standardizingPath
        self.staticFavorites = staticFavorites
        self.zoxideFavorites = zoxideFavorites
        let home = NSHomeDirectory()
        self.favURL = favoritesURL ?? URL(fileURLWithPath: home + "/.cache/workspace-switcher/files-favorites.json")
        self.parentButton = ThemeButton(config: config, title: "", symbol: "arrow.up")
        self.starButton = ThemeButton(config: config, title: "Pin", symbol: "star")
        self.sortKey = SortKey(rawValue: config.browserSort.lowercased()) ?? .name
        self.sortDescending = config.browserSortDescending
        self.sortButton = ThemeButton(config: config, title: "", symbol: "line.3.horizontal.decrease")
        self.sortButton.chevron = true
        self.orderButton = ThemeButton(config: config, title: "", symbol: "arrow.up")
        self.listPane = FileListPane(config: config)
        self.previewList = FileListPane(config: config)
        super.init(frame: .zero)
        wantsLayer = true
        // translucent drawer background — the alpha rides IN the color
        // (config file / color picker opacity slider), so the explorer can
        // be anywhere from see-through to opaque without a code change
        layer?.backgroundColor = config.fileBrowserBackground.cgColor

        listPane.onSelect = { [weak self] i in
            self?.selection = i
            self?.previewSelection()
            self?.scrollSelectionVisible()
        }
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
        }
        listPane.onOpenInNotes = { [weak self] i in
            guard let self, self.rows.indices.contains(i) else { return }
            self.onOpenInNotes?(self.rows[i].path)
        }
        listPane.onOpenTerminal = { [weak self] i in
            guard let self, self.rows.indices.contains(i) else { return }
            self.openTerminal(self.terminalDir(for: self.rows[i]))
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
        // input well in the shared button language: ghost fill + hairline
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

        // the file list lives in a scroll view so rows longer than the pane
        // scroll INSIDE it — they can never bleed off the window's edge
        listScroll.hasVerticalScroller = true
        listScroll.autohidesScrollers = true
        listScroll.drawsBackground = false
        listScroll.borderType = .noBorder
        listScroll.documentView = listPane

        // folder preview list: same file rows as the left pane, with the same
        // right-click actions (Open in Notes / Copy Path / Open / Reveal)
        previewListScroll.hasVerticalScroller = true
        previewListScroll.autohidesScrollers = true
        previewListScroll.drawsBackground = false
        previewListScroll.borderType = .noBorder
        previewListScroll.documentView = previewList
        previewList.onOpen = { [weak self] i in self?.previewOpen(i) }
        previewList.onParent = { [weak self] in self?.cdParent() }
        previewList.onCopyPath = { [weak self] i in
            guard let self, self.previewList.rows.indices.contains(i) else { return }
            let p = self.previewList.rows[i].path
            self.onCopyPath?(p)
            self.onStatus?("copied \(p)")
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

    // window resizes (drawer toggle, drag) must re-run the pane layout
    override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        needsLayout = true
    }

    // MARK: layout

    override func layout() {
        super.layout()
        layoutPanes()
        updatePartFocus()
    }

    // follow first-responder changes (Tab, clicks, Cmd+L, Ctrl+J/K) and key
    // status so the ring always marks the part that receives typing
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
        partRing.layer?.borderColor = ButtonStyle.focusStroke(c).withAlphaComponent(0.9).cgColor
        var ring: NSRect?
        switch part {
        case .list: ring = listScroll.frame
        case .preview: ring = previewListScroll.isHidden ? previewScroll.frame : previewListScroll.frame
        case .filter, nil: ring = nil
        }
        if let r = ring, r.width > 8, r.height > 8 {
            partRing.frame = r.insetBy(dx: 3, dy: 3)
            partRing.isHidden = false
            addSubview(partRing, positioned: .above, relativeTo: nil)
        } else {
            partRing.isHidden = true
        }
    }
    private func layoutPanes() {
        let w = bounds.width
        let toolbarY: CGFloat = 4
        let toolbarH: CGFloat = 24
        // pin first (left), then parent, then the sort pair (sort-key
        // dropdown + labelled Asc/Desc toggle — kept away from the parent
        // arrow so the two never read as one control), then the filter bar
        // every button is sized by its own label (equal side padding); the
        // pin button reserves its longer "Pinned" label so toggling it never
        // shifts the row
        let x0: CGFloat = 8
        starButton.frame = NSRect(x: x0, y: toolbarY,
                                  width: max(starButton.fittingWidth(for: "Pin"),
                                             starButton.fittingWidth(for: "Pinned")),
                                  height: toolbarH)
        parentButton.frame = NSRect(x: starButton.frame.maxX + 4, y: toolbarY,
                                    width: toolbarH + 4, height: toolbarH)
        let sortW = SortKey.allCases.map { sortButton.fittingWidth(for: $0.short) }.max() ?? 80
        sortButton.frame = NSRect(x: parentButton.frame.maxX + 10, y: toolbarY,
                                  width: sortW, height: toolbarH)
        orderButton.frame = NSRect(x: sortButton.frame.maxX + 4, y: toolbarY,
                                   width: max(orderButton.fittingWidth(for: "Asc"),
                                              orderButton.fittingWidth(for: "Desc")),
                                   height: toolbarH)
        searchField.frame = NSRect(x: orderButton.frame.maxX + 10, y: toolbarY,
                                   width: max(60, w - orderButton.frame.maxX - 10 - x0),
                                   height: toolbarH)
        // favorites wrap to as many lines as their paths need
        let favY = toolbarY + toolbarH + 5
        let favH = layoutFavorites(from: favY)
        let listY = favY + favH + 4
        let bottomH: CGFloat = 18
        statusLine.frame = NSRect(x: 8, y: bounds.height - bottomH + 1,
                                  width: max(0, w - 16), height: bottomH - 3)
        let splitW: CGFloat = 6
        // the split is draggable; clamp so neither pane gets tiny
        let splitX = min(max(splitFraction * w, 200), max(200, w - 200))
        let contentH = max(0, bounds.height - listY - bottomH)
        listScroll.frame = NSRect(x: 0, y: listY, width: splitX, height: contentH)
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
    // the list's document (the row pane) grows to fit every row; the scroll
    // view clips it at the pane height so long lists scroll in place
    private func layoutListDocument() {
        let clipH = max(0, listScroll.bounds.height)
        let docH = max(clipH, CGFloat(listPane.rows.count) * 22)
        listPane.frame = NSRect(x: 0, y: 0, width: max(0, listScroll.bounds.width), height: docH)
        listPane.needsDisplay = true
    }
    private func layoutPreviewListDocument() {
        let clipH = max(0, previewListScroll.bounds.height)
        let docH = max(clipH, CGFloat(previewList.rows.count) * 22)
        previewList.frame = NSRect(x: 0, y: 0, width: max(0, previewListScroll.bounds.width), height: docH)
        previewList.needsDisplay = true
    }
    // keep the keyboard/cursor selection inside the visible area (never let
    // the selection scroll off-screen)
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
    // wrap the favorite pills to multiple lines when the paths are long;
    // returns the total height consumed (0 when there are no favorites)
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
    // ~/notes instead of /Users/me/notes for pinned/config paths under home
    private func displayPath(_ p: String) -> String {
        let home = NSHomeDirectory()
        if p == home { return "~" }
        if p.hasPrefix(home + "/") { return "~" + p.dropFirst(home.count) }
        return p
    }

    // MARK: data

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
        for n in names where hidden || !n.hasPrefix(".") {
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
    // stat one path into an Entry (thread-safe: no icon, no decoration).
    // Plain stat(2) on purpose: FileManager.attributesOfItem also reads
    // extended attributes, and getxattr blocks forever on a stale network /
    // FUSE mount — which froze the whole app while previewing ~.
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
    // trailing column follows the sort: dates when sorting by date, else size
    private func decorate(_ e: inout Entry) {
        switch sortKey {
        case .modified: e.trailingText = e.name == ".." ? "" : Self.shortDate(e.modified)
        case .created: e.trailingText = e.name == ".." ? "" : Self.shortDate(e.created)
        default: e.trailingText = e.isDir ? "" : Self.humanSize(e.size)
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
    // ".." first, folders on top (Finder style), then the chosen key; ties
    // fall back to the name so the order is stable
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
        all = listDir(cwd)
        hiddenAll = nil
        dirCache = nil
        refilter()
    }

    // MARK: query

    private static let globChars = CharacterSet(charactersIn: "*?[")
    private static func hasGlob(_ s: String) -> Bool {
        s.rangeOfCharacter(from: globChars) != nil
    }
    // ~ / relative (against the cwd) -> absolute, standardized
    private func resolvePath(_ s: String) -> String {
        var p = Self.expandTilde(s)
        if !p.hasPrefix("/") { p = (cwd as NSString).appendingPathComponent(p) }
        return (p as NSString).standardizingPath
    }

    // Filter-bar grammar:
    //   term | terminal | cmd [path]   Enter opens a terminal there
    //   notes / *.md / .*              filter this folder (.* shows dotfiles)
    //   ~/notes/to  /etc/ho  ../x      list THAT folder, filtered by the tail
    //   **/*.swift  ~/src/**/todo      recursive (ripgrep) below the folder
    //   ~/src/*/README*                wildcard folders are recursive too
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
        // split at the last "/": the head is the folder, the tail the filter
        let slash = q.range(of: "/", options: .backwards)
        let head = slash.map { String(q[..<$0.upperBound]) } ?? ""
        let tail = slash.map { String(q[$0.upperBound...]) } ?? q
        if Self.hasGlob(head) {
            // wildcard folders: search from the deepest literal folder
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

    // name filter shared by the cwd and typed-path listings: a glob matches
    // the whole name, plain text is a case-insensitive substring
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

    // MARK: recursive search (ripgrep)

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
    // debounced: typing "~/src/**/foo" must not spawn rg per keystroke
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
        // a bare word as the last segment means "name contains":
        // **/sink -> **/*sink*
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
                // no ripgrep: walk with FileManager (slower, same semantics)
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
    // FileManager fallback for startSearch; returns true when capped
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
    // gitignore-style glob over a relative path: * stays in one folder,
    // ** crosses folders, and a slash-free glob matches at any depth
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
    // one icon per file type (search results can be thousands of files)
    private static var typeIcons: [String: NSImage] = [:]
    private static func typeIcon(_ e: Entry) -> NSImage {
        let ext = (e.path as NSString).pathExtension.lowercased()
        if let i = typeIcons[ext] { return i }
        let img = NSWorkspace.shared.icon(for: UTType(filenameExtension: ext) ?? .data)
        typeIcons[ext] = img
        return img
    }

    // "*.md", "note*", "?og*" -> anchored, case-insensitive regex
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

    // MARK: sort

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
    func cd(_ dir: String) {
        cwd = (dir as NSString).standardizingPath
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
        let parent = (cwd as NSString).deletingLastPathComponent
        if parent != cwd { cd(parent) }
    }
    // the filter bar doubles as the address bar: at rest it shows the current
    // directory; clicking it (select-all) lets you type a filter or a path
    func showCwdInFilter() {
        guard searchField.currentEditor() == nil else { return }
        searchField.stringValue = cwd
        searchField.placeholderString = nil
    }
    func copyDir() {
        onCopyDir?(cwd)
    }
    // copy a list row's path (Cmd+C while the list is focused)
    @discardableResult
    func copyRowPath(_ i: Int) -> String? {
        guard rows.indices.contains(i) else { return nil }
        let p = rows[i].path
        onCopyPath?(p)
        onStatus?("copied \(p)")
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

    // Windows-explorer style: when the filter bar holds an EXISTING path
    // (~/…, /…, ../…) and the user hits Enter, jump straight to it (cd into a
    // dir / open a file). Anything else falls through to opening the list
    // selection (typed-path and wildcard queries list their matches there).
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

    // `term` / right-click "Open Terminal Here": a folder opens itself, a
    // file its folder
    private func terminalDir(for e: Entry) -> String {
        e.isDir ? e.path : (e.path as NSString).deletingLastPathComponent
    }
    private func openTerminal(_ dir: String) {
        onOpenTerminal?(dir)
        setStatus("terminal opened in \(displayPath(dir))")
    }

    // MARK: preview

    private func previewSelection() {
        guard rows.indices.contains(selection) else { return showHint(""); }
        let e = rows[selection]
        let ext = (e.path as NSString).pathExtension.lowercased()
        if e.isDir {
            // show the folder's CONTENTS as a real file list on the right
            // (icons, sizes, hover, right-click Open in Notes / Copy Path)
            previewList.rows = listDir(e.path).filter { $0.name != ".." }
            previewList.selection = 0
            layoutPreviewListDocument()
            showFolderList()
        } else if Self.imageExts.contains(ext) {
            let img: NSImage?
            if ext == "pdf" {
                // NSImage renders PDFs transparent — composite onto white so
                // the blue drawer doesn't show through the page
                img = Self.pdfPreviewImage(e.path)
            } else {
                img = NSImage(contentsOfFile: e.path)
            }
            if let img {
                previewImage.image = img
                showImage()
            } else {
                showHint("unable to preview")
            }
        } else if ext == "rtf", let img = Self.rtfPreviewImage(e.path) {
            // render the rich text onto white (same white-backed treatment)
            previewImage.image = img
            showImage()
        } else if ext == "docx", let text = Self.docxText(e.path), !text.isEmpty {
            // extract the text out of the zip's document.xml
            previewText.string = text
            previewText.scrollRangeToVisible(NSRange(location: 0, length: 0))
            showText()
        } else if let text = textPreview(e.path) {
            previewText.string = text
            previewText.scrollRangeToVisible(NSRange(location: 0, length: 0))
            showText()
        } else {
            showHint("no preview")
        }
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
    // double-click / Enter on a row in the folder preview: drill in or open
    private func previewOpen(_ i: Int) {
        guard previewList.rows.indices.contains(i) else { return }
        let e = previewList.rows[i]
        if e.isDir { cd(e.path) } else { onOpen?(e.path) }
    }
    private func textPreview(_ path: String) -> String? {
        guard let sz = ((try? FileManager.default.attributesOfItem(atPath: path))?[.size] as? NSNumber)?.intValue else { return nil }
        if sz > Self.textLimit {
            guard let h = FileHandle(forReadingAtPath: path) else { return nil }
            defer { try? h.close() }
            let data = h.readData(ofLength: Self.textLimit)
            return String(data: data, encoding: .utf8)
        }
        guard let data = FileManager.default.contents(atPath: path), !data.isEmpty else { return nil }
        return String(data: data, encoding: .utf8)
    }

    // render a PDF's first page onto an OPAQUE white background: NSImage
    // loads a PDF with a transparent background, which bleeds the blue drawer
    // through the page. 2x resolution so it reads crisp when scaled up.
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

    // render an RTF file's rich text onto an OPAQUE white background (2x), so
    // it previews like a document instead of raw RTF markup or blue bleed
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

    // DOCX is a zip — pull word/document.xml out via `unzip -p` and collect
    // the paragraph text
    private static func docxText(_ path: String) -> String? {
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/usr/bin/unzip")
        p.arguments = ["-p", path, "word/document.xml"]
        let pipe = Pipe()
        p.standardOutput = pipe
        p.standardError = Pipe()
        do { try p.run() } catch { return nil }
        p.waitUntilExit()
        guard p.terminationStatus == 0 else { return nil }
        let xmlData = pipe.fileHandleForReading.readDataToEndOfFile()
        guard let xml = String(data: xmlData, encoding: .utf8) else { return nil }
        let parser = XMLParser(data: Data(xml.utf8))
        let ex = DocxTextExtractor()
        parser.delegate = ex
        parser.parse()
        return ex.text.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    // MARK: favorites

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
    // config favorites + zoxide top-N + starred, deduped, order-preserving
    private func mergedFavorites() -> [String] {
        var out: [String] = []
        var seen = Set<String>()
        for raw in staticFavorites + zoxideFavorites + pinnedFavorites {
            let p = (raw as NSString).expandingTildeInPath
            if seen.insert(p).inserted { out.append(p) }
        }
        return out
    }
    private func toggleStar() {
        if shownFavorites.contains(cwd) {
            // it's already a favorite somewhere — pull it out of the pinned
            // set only if it wasn't config/zoxide-supplied (those are fixed)
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
        for fav in shownFavorites {
            let p = ThemeButton(config: config, title: displayPath(fav), symbol: "folder")
            p.isOn = fav == cwd
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

    // MARK: search

    // clicking the filter bar selects the whole path so typing replaces it
    // (address-bar behavior) instead of inserting into the middle
    func controlTextDidBeginEditing(_ obj: Notification) {
        guard (obj.object as AnyObject?) === searchField else { return }
        searchField.currentEditor()?.selectAll(nil)
    }
    // leaving the filter bar with no query restores the directory display
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
    // one dim line under the list describing what Enter will do
    private func updateStatus() {
        let items = rows.filter { $0.name != ".." }.count
        let count = "\(items) item\(items == 1 ? "" : "s")"
        switch mode {
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
            break   // the search reports its own progress / result count
        }
    }

    // Field-editor commands for the filter bar: the field editor owns
    // Return/Tab/Up/Down while editing, so this delegate hook (the only
    // reliable interception) routes them — Return runs `term`, jumps to a
    // typed path or opens the list selection; Tab completes the selected
    // row into the bar (shell style); Up/Down move the list selection.
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

// MARK: - Status bar (bottom strip)

// Rounded strip pinned to the bottom of an editor window for transient
// feedback (e.g. prettyprint parse errors). Error state = red tint + hairline
// + red monospace text; normal state = subtle highlight matching the pill
// theme. Hidden when there's nothing to say.
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
        // a themed status line (the tmux bottom bar): deep crust strip, a
        // solid lead segment in the accent — danger when reporting an error
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

// MARK: - Chrome (drag + resize overlay)

// Transparent overlay above the content that owns the window chrome: resize
// edges (when enableResize) and a drag area (a top header strip for editors,
// or drag-anywhere for list windows). Non-chrome areas return nil from
// hitTest so the content below keeps its own events (text selection, typing).
final class PopupChrome: NSView {
    var config: PopupConfig
    // live header fill pushed by the color picker — the chrome holds its own
    // copy of the config struct, so the window re-pushes the picked color
    // here for the drag-header strip to restyle without a rebuild
    var headerColorOverride: NSColor? {
        didSet { needsDisplay = true }
    }
    var zoom: CGFloat = 1.0
    var dragHeaderHeight: CGFloat = 0      // top strip that drags the window
    var dragAnywhere: Bool = false         // drag on any non-reserved area
    var reservedRect: NSRect = .zero       // pass-through zone (e.g. search field)
    var headerTitle: String?
    // small glyph drawn just left of the centered title pill (app identity)
    var headerIcon: NSImage?
    // live item count (e.g. filtered list size), drawn dim on the left side
    var itemCount: String?
    // dim metadata drawn in the header right after itemCount (e.g. last write)
    var footerText: String?
    // live mic meter (recording control bar): when meterEnabled, the chrome
    // draws a permanent bottom bar with a big record/stop button, pause/
    // resume, real-time level bars and the elapsed time
var meterEnabled = false {
        didSet {
            window?.invalidateCursorRects(for: self)
            needsDisplay = true
        }
    }
    var meterState: Int = 0        // 0 idle, 1 recording, 2 paused, 3 transcribing
    var meterLevel: Float = 0      // 0-1, live
    var meterElapsed: TimeInterval = 0
    // hit rects for the bar's buttons (set during draw; the chrome owns
    // clicks in the bar area via hitTest + mouseDown)
    var meterRecordRect: NSRect = .zero
    var meterPauseRect: NSRect = .zero
    var onMeterRecord: (() -> Void)?
    var onMeterPause: (() -> Void)?
    let meterBarHeight: CGFloat = 36
    // header button labels — the host refreshes them as state changes
    // (active tab, selection count); nil-safe defaults keep old behavior
    var copyPathLabel = "copy path"
    var copyConfigLabel = "copy config"
    // extra host-defined buttons (ids >= 10), drawn leftmost; clicks route
    // through PopupWindow.onHeaderButton with the button's id
    var extraButtons: [(label: String, id: Int)] = []
    // left-to-right segment order by button id (copy path=1, copy config=2,
    // copy rows=3, host buttons = their id). nil = default (copy buttons
    // first, then host buttons); unlisted ids trail in that default order.
    var headerOrder: [Int]?
    var extraButtonRects: [Int: NSRect] = [:]
    // extra buttons whose feature is currently ON (e.g. the terminal / file
    // browser drawer is open) — drawn darker than the idle state
    var activeButtonIDs: Set<Int> = []
    // header button hit rects (flipped coords, set during draw) — the window
    // uses these to route header clicks to the right action
    var copyButtonRect: NSRect = .zero
    var configButtonRect: NSRect = .zero
    // "copy all" / "copy N" (row copy selection); nil = not drawn
    var copyRowsButtonRect: NSRect = .zero
    var copyRowsLabel: String?
    private var startFrame: NSRect = .zero
    private var startPoint: NSPoint = .zero
    private var dragEdges: PopupBackdrop.Edge = []
    private var draggingWindow = false
    // header button feedback: flips to "✓ …" for a moment after a copy
    private var feedback: Int = 0          // 0 none, 1 copy, 2 config
    private var feedbackTimer: DispatchWorkItem?
    // header button hover feedback: the id of the segment under the cursor
    private var hoveredSegment: Int?
    // app-icon menu button (header far left)
    var iconButtonRect: NSRect {
        NSRect(x: config.headerCloseButton ? 32 : 6, y: (dragHeaderHeight - 22) / 2, width: 40, height: 22)
    }
    // ✕ close glyph (far left, before the icon); .zero when off
    var closeButtonRect: NSRect {
        config.headerCloseButton && dragHeaderHeight > 0
            ? NSRect(x: 6, y: (dragHeaderHeight - 22) / 2, width: 22, height: 22) : .zero
    }
    // where the dim meta line starts: just past the close glyph / icon
    var leftInset: CGFloat {
        if headerIcon != nil { return iconButtonRect.maxX + 8 }
        return config.headerCloseButton ? closeButtonRect.maxX + 8 : 10
    }
    var iconHovered = false { didSet { if iconHovered != oldValue { needsDisplay = true } } }
    var closeHovered = false { didSet { if closeHovered != oldValue { needsDisplay = true } } }
    var iconMenuOpen = false { didSet { if iconMenuOpen != oldValue { needsDisplay = true } } }
    // segment rects (fb id -> rect) set during draw, used for hover hit-testing
    private var headerSegRects: [(Int, NSRect)] = []
    private var trackingArea: NSTrackingArea?

    private let minW: CGFloat = 120
    private let minH: CGFloat = 100
    private let hit: CGFloat = 8

    override var isFlipped: Bool { true }

    // Keep the plain arrow over the header + record bar: without cursor
    // rects the editor's I-beam shows through the bar and the top strip.
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

    // mouse-move tracking for the header-button hover highlight (and the
    // record bar): the chrome owns the header strip, so the tracking area
    // lives here, not on the subviews
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
        }
        iconHovered = headerIcon != nil && dragHeaderHeight > 0 && iconButtonRect.contains(p)
        closeHovered = closeButtonRect.contains(p)
        if hovered != hoveredSegment {
            hoveredSegment = hovered
            needsDisplay = true
        }
    }

    private func edges(at p: NSPoint) -> PopupBackdrop.Edge {
        var e: PopupBackdrop.Edge = []
        if p.x <= hit { e.insert(.left) }
        if p.x >= bounds.width - hit { e.insert(.right) }
        if p.y <= hit { e.insert(.top) }
        if p.y >= bounds.height - hit { e.insert(.bottom) }
        return e
    }

    // hand-rolled edge resize only for windows the system can't resize
    // (borderless panels); titled windows use native .resizable
    private var customResize: Bool {
        config.enableResize && !(window?.styleMask.contains(.resizable) ?? false)
    }

    // Resize zones for hit-testing: the top edge never resizes when a drag
    // header exists — the header owns the top strip, otherwise grabbing it
    // near the edge starts a resize while trying to move the window.
    private func resizeEdges(at p: NSPoint) -> PopupBackdrop.Edge {
        var e = edges(at: p)
        if dragHeaderHeight > 0 {
            e.remove(.top)
        }
        return e
    }

    // Only claim chrome zones; everything else falls through to the content.
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
        // record-bar buttons first (bottom strip); everything else drags
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
        if customResize && !resizeEdges(at: p).isEmpty {
            dragEdges = resizeEdges(at: p)
            startFrame = window?.frame ?? .zero
            // ABSOLUTE screen coords, not locationInWindow: window-relative
            // deltas go to ~0 once the window catches up with the mouse, which
            // makes the window jitter instead of following the cursor
            startPoint = NSEvent.mouseLocation
        } else {
            // Move via the native window drag. These windows have a real
            // (hidden) titlebar for the AX close button; hand-rolling
            // setFrameOrigin fights the titlebar's own drag and makes the
            // window shake. performWindowDrag is a single, smooth drag system.
            window?.performDrag(with: event)
        }
    }

    override func mouseDragged(with event: NSEvent) {
        guard let win = window else { return }
        let m = NSEvent.mouseLocation
        let dx = m.x - startPoint.x
        let dy = m.y - startPoint.y
        if !dragEdges.isEmpty {
            var f = startFrame
            var w = f.width
            var h = f.height
            if dragEdges.contains(.right) {
                w = max(minW, startFrame.width + dx)
            } else if dragEdges.contains(.left) {
                let nw = max(minW, startFrame.width - dx)
                f.origin.x += startFrame.width - nw
                w = nw
            }
            if dragEdges.contains(.top) {
                h = max(minH, startFrame.height + dy)
            } else if dragEdges.contains(.bottom) {
                let nh = max(minH, startFrame.height - dy)
                f.origin.y += startFrame.height - nh
                h = nh
            }
            f.size = NSSize(width: w, height: h)
            win.setFrame(clampToScreen(f), display: true)
            win.invalidateShadow()
        } else if draggingWindow {
            // moves don't need shadow invalidation — the shadow follows the
            // frame; invalidating on every event causes drag jank
            win.setFrameOrigin(NSPoint(x: startFrame.origin.x + dx,
                                       y: startFrame.origin.y + dy))
        }
    }

    override func mouseUp(with event: NSEvent) {
        let didResize = !dragEdges.isEmpty
        dragEdges = []
        draggingWindow = false
        if didResize {
            window?.invalidateShadow()
        }
        super.mouseUp(with: event)
    }

    // Nerd-font glyphs (BMP PUA + Supplementary PUA-A codepoints, e.g. the
// terminal / file-browser icons) only measure and render correctly in a Nerd
// Font — a label carrying one gets the terminal's font so the segment sizes
// to the real glyph instead of a missing-glyph box (which both under-measures
// and draws a tofu square). Nerd Fonts v3 moved MDI icons to U+F0000-U+F2FDF.
private func headerButtonFont(_ label: String) -> NSFont {
        let scalars = label.unicodeScalars
        let pua = { (v: UInt32) in
            (0xE000...0xF8FF).contains(v)
                || (0xF0000...0xFFFFD).contains(v)
                || (0x100000...0x10FFFD).contains(v)
        }
        let needsNerd = scalars.contains { pua($0.value) }
        if needsNerd, let f = NSFont(name: config.terminalFont, size: config.buttonFontSize) {
            // pure icon glyphs (a single PUA character — terminal / finder /
            // mic toggles) render BIGGER than text labels so the icons read
            // clearly; labels that mix in text keep the normal size
            if scalars.allSatisfy({ pua($0.value) }) {
                return NSFont(name: config.terminalFont, size: config.buttonFontSize + 3.5) ?? f
            }
            return f
        }
        return NSFont.systemFont(ofSize: config.buttonFontSize, weight: .semibold)
    }

    // header button segments at their FULL label width (the "✓ " feedback
    // prefix widens the active one): shared by draw and neededWidth so the
    // window-growth math and the render can never disagree
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

    // full width the header needs (left icon + meta + title pill + every
    // button at full label): the window grows to this rather than clip text
    func neededWidth() -> CGFloat {
        var w: CGFloat = 34
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

    // Slim drag header for editors: solid background + hairline + title.
    override func draw(_ dirtyRect: NSRect) {
        guard dragHeaderHeight > 0 else { return }
        let header = NSRect(x: 0, y: 0, width: bounds.width, height: dragHeaderHeight)
        (headerColorOverride ?? config.headerColor ?? config.colors.background).setFill()
        header.fill()
        config.colors.hairline.setStroke()
        let line = NSBezierPath()
        line.lineWidth = 1
        line.move(to: NSPoint(x: 0, y: dragHeaderHeight - 0.5))
        line.line(to: NSPoint(x: bounds.width, y: dragHeaderHeight - 0.5))
        line.stroke()
        // button cluster first: the centered title must avoid it when a window
        // carries many header buttons (e.g. the doctor's poll targets)
        let segs = headerSegs()
        // dim metadata line (live item count, last file write) — measured here
        // so a stretched button bar can stop just past it instead of hiding it
        var meta = ""
        for t in [itemCount, footerText].compactMap({ $0 }) {
            meta += meta.isEmpty ? t : "   " + t
        }
        let metaAttrs: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: 9),
            .foregroundColor: config.colors.dim,
        ]
        let metaWidth = (meta as NSString).size(withAttributes: metaAttrs).width
        // stretchHeaderButtons: the joined bar fills the whole header strip
        // from the right edge back to just past the icon/meta, instead of a
        // compact cluster hugging the right edge
        let stretch = config.stretchHeaderButtons && !segs.isEmpty
        let leftContent: CGFloat = stretch
            ? leftInset + (meta.isEmpty ? 0 : metaWidth + 8)
            : 0
        // the joined bar's | dividers (one less than the segment count)
        let naturalBarW = segs.map { $0.w }.reduce(0, +)
            + CGFloat(max(0, segs.count - 1))
        var buttonsWidth: CGFloat = 10 + naturalBarW
        if stretch {
            buttonsWidth = max(naturalBarW, bounds.width - 10 - leftContent)
        }
        if let title = headerTitle {
            // centered app-title: bold + full-strength text on a subtle pill
            // so the window's identity reads at a glance from across the desk
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
        // app glyph pinned to the FAR-LEFT edge of the header (drawn even
        // when the title is gone — e.g. jira/notes have no header label),
        // vertically centered to line up with the header buttons
        let closeRect = closeButtonRect
        if !closeRect.isEmpty {
            // ✕ close glyph: a quiet ghost chip, red on hover (traffic-light cue)
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
        if let icon = headerIcon {
            // the app glyph is a MENU button (all window actions): a ghost
            // pill with the icon + a ▾ chevron, lit on hover / while open
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
        // header buttons (right side): "copy config" (copy the config file
        // path) and "copy path" (copy the open file path); each flips to
        // "✓ …" for a moment after a copy. The row-copy button (host-enabled
        // via selectableRows) sits leftmost of the three. All segments join
        // into ONE bar with thin | dividers between them.
        let barH: CGFloat = 20
        let barW = stretch ? max(naturalBarW, bounds.width - 10 - leftContent)
                           : naturalBarW
        let barRect = NSRect(x: stretch ? leftContent : bounds.width - 10 - barW,
                             y: (dragHeaderHeight - barH) / 2,
                             width: barW, height: barH)
        // one ghost bar holding the segments; the on / active / hovered
        // segment gets its own inset chip in the shared button style
        if !segs.isEmpty {
            ButtonStyle.draw(barRect, .idle, config.colors, radius: config.buttonRadius)
        }
        // stretch mode shares the leftover width evenly across the segments
        let perSegExtra = stretch ? max(0, barW - naturalBarW) / CGFloat(segs.count) : 0
        headerSegRects = []
        // a button whose label was cleared must stop catching clicks
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
                // hairline divider between two idle segments
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
        // dim metadata line (live item count, last file write) on the SAME row as
        // the far-left icon — truncated so it never runs into the right-side
        // header buttons (measured above for the stretched-bar layout)
        if !meta.isEmpty {
            let x0: CGFloat = leftInset
            let maxW = max(60, bounds.width - buttonsWidth - x0 - 10)
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
        // live recording control bar (bottom): big record/stop button, pause/
        // resume, real-time level bars and elapsed — a permanent, unmistakable
        // control while the voice window is open
        if meterEnabled {
            let strip = NSRect(x: 0, y: bounds.height - meterBarHeight,
                               width: bounds.width, height: meterBarHeight)
            // OPAQUE base so the editor's text never bleeds through the bar
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
                // transcribing: just a dim label, buttons inactive
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
                // big record / stop button (left)
                meterRecordRect = NSRect(x: 12, y: strip.midY - 13, width: 26, height: 26)
                let mid = NSPoint(x: meterRecordRect.midX, y: meterRecordRect.midY)
                if active {
                    // stop: solid red rounded square
                    NSColor.systemRed.setFill()
                    NSBezierPath(roundedRect: NSRect(x: mid.x - 7, y: mid.y - 7,
                                                     width: 14, height: 14),
                                 xRadius: 3, yRadius: 3).fill()
                } else {
                    // record: red circle with a bright pulse ring
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
                // pause / resume button (only during a session)
                if active {
                    meterPauseRect = NSRect(x: 46, y: strip.midY - 11, width: 30, height: 22)
                    NSColor.systemRed.withAlphaComponent(0.35).setFill()
                    NSBezierPath(roundedRect: meterPauseRect,
                                 xRadius: config.buttonRadius - 1,
                                 yRadius: config.buttonRadius - 1).fill()
                    if meterState == 1 {
                        // pause: two bars
                        NSColor.white.withAlphaComponent(0.9).setFill()
                        NSRect(x: meterPauseRect.midX - 7, y: meterPauseRect.midY - 5,
                               width: 4, height: 10).fill()
                        NSRect(x: meterPauseRect.midX + 3, y: meterPauseRect.midY - 5,
                               width: 4, height: 10).fill()
                    } else {
                        // resume: right-pointing triangle
                        NSColor.white.withAlphaComponent(0.9).setFill()
                        let tri = NSBezierPath()
                        tri.move(to: NSPoint(x: meterPauseRect.midX - 4, y: meterPauseRect.midY - 5))
                        tri.line(to: NSPoint(x: meterPauseRect.midX - 4, y: meterPauseRect.midY + 5))
                        tri.line(to: NSPoint(x: meterPauseRect.midX + 6, y: meterPauseRect.midY))
                        tri.close()
                        tri.fill()
                    }
                }
                // live level bars
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
                // elapsed, right-aligned
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
    }

    // visual "copied" feedback after a header copy click (1 = copy, 2 = config)
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

// MARK: - Terminal auto-restart

// SwiftTerm reports the shell exiting via LocalProcessTerminalViewDelegate;
// this tiny adapter forwards it so the window can respawn the shell (the
// drawer must never be left dead after `exit` / Ctrl-D).
final class TerminalAutoRestart: NSObject,
                                 @preconcurrency LocalProcessTerminalViewDelegate {
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

// Inline images for the vim pane: drawn over the blank virtual rows the
// editor reserves under each ![](path) link. Click-through (hitTest nil) so
// the mouse always reaches vim; clipped to the text rows (never over the
// command line).
final class VimImageOverlay: NSView {
    struct Item: Equatable { let path: String; let row: Int; let rows: Int }
    var cell = NSSize(width: 8, height: 16) { didSet { if cell != oldValue { relayout() } } }
    var textRows = 0 { didSet { if textRows != oldValue { relayout() } } }
    var items: [Item] = [] { didSet { if items != oldValue { relayout() } } }
    private var views: [NSView] = []   // top-level children (image or clip)
    private var cache: [String: (mtime: Date, image: NSImage)] = [:]

    override var isFlipped: Bool { true }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

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
        let limit = CGFloat(textRows) * cell.height
        for it in items {
            guard let img = image(it.path), img.size.width > 0, img.size.height > 0 else { continue }
            let top = CGFloat(it.row) * cell.height + 2
            guard top < limit else { continue }
            // aspect-fit into the reserved rows, never upscaled
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
            // clip at the command line row
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
                continue
            }
            addSubview(iv)
            views.append(iv)
        }
    }
}

// Close button target for windows with showCloseButton = true (e.g. vim mode).
// The button's action calls onClose, which the host sets to handle the close
// (e.g. send :wq to Vim before closing).
final class WindowCloseTarget: NSObject {
    var onClose: (() -> Void)?
    @objc func close(_ sender: Any?) { onClose?() }
}

// Right-click menu actions for the embedded terminal drawer: "Copy" reads the
// current selection safely and writes it to the clipboard; "Open in Notes"
// forwards the selected text (a path) to the host's onTerminalOpenInNotes hook.
final class TerminalMenuTarget: NSObject {
    var copy: (() -> Void)?
    var openInNotes: (() -> Void)?
    var openInDefault: (() -> Void)?
    var revealInFinder: (() -> Void)?
    @objc func copySelection(_ sender: Any?) { copy?() }
    @objc func openSelectionInNotes(_ sender: Any?) { openInNotes?() }
    @objc func openSelectionInDefaultApp(_ sender: Any?) { openInDefault?() }
    @objc func revealSelectionInFinder(_ sender: Any?) { revealInFinder?() }
}

// MARK: - Popup window

public final class PopupWindow: NSObject, NSTextFieldDelegate, NSWindowDelegate {
    public var config: PopupConfig

    // behavior hooks
    public var onShow: (() -> Void)?                // called each time the popup becomes visible
    public var onFilter: ((String) -> [PopupRow])?   // query -> rows to display
    public var onAccept: ((PopupRow) -> Void)?       // Enter/Return on a row
    public var onRowClick: ((Int) -> Void)?          // mouse click on a row (after selection)
    public var onRowDoubleClick: ((Int) -> Void)?    // double-click on a row
    public var onEscape: (() -> Void)?               // Esc (overrides default hide)
    public var onHide: ((Bool) -> Void)?             // called after hide, with the restore flag
    // host hook fired when the window hides (e.g. stop a voice recording
    // session owned by the host before the window disappears)
    public var onHideVoiceStop: (() -> Void)?
    // vim mode: fired (main thread) when the embedded editor process exits
    // (e.g. `:q`). The pane relaunches itself on the current note right
    // after, so the editor is never left dead — hosts only log/observe.
    public var onVimExit: (() -> Void)?
    // vim mode: args for every (re)launch of the editor — hosts return the
    // CURRENT note so `:q` + relaunch reopens what the tab strip shows.
    // nil = config.vimEditorArgs.
    public var vimLaunchArgs: (() -> [String])?
    // Cmd+Opt+= / Cmd+Opt+- : font size step (+1 / -1); the host persists it
    public var onFontSizeStep: ((Int) -> Void)?
    // host hook called when the window close button (X) is clicked. If the
    // hook returns true, it handled the close (e.g. sent :wq to Vim); if
    // false or nil, the default close behavior applies.
    public var onCloseWindow: (() -> Void)?
    // edit-mode hooks: editorText is the initial content (set before show);
    // onEditorCommit fires on Cmd+S (window stays open); onEditorClose fires
    // with the final text whenever the window hides.
    public var editorText: String = ""
    public var onEditorCommit: ((String) -> Void)?
    public var onEditorClose: ((String) -> Void)?
    // fired after each user-initiated editor change (typing / paste / delete);
    // hosts use it for live reactions (prettyprint auto-format, live preview)
    public var onEditorTextChange: (() -> Void)? {
        didSet { wireEditorTextChange() }
    }
    // transient status strip at the bottom of an editor window (e.g. the
    // prettyprint parse error). Call setStatus(nil) to clear.
    public func setStatus(_ text: String?, isError: Bool) {
        guard let bar = statusBar else { return }
        let visible = !(text?.isEmpty ?? true)
        bar.text = text ?? ""
        bar.isError = isError
        bar.isHidden = !visible
        layoutEditorScroll()
    }
    // row rendering: if set, the app draws each row rect itself (pill, icons,
    // etc.); otherwise the framework draws a minimal generic default.
    public var onDrawRow: ((NSRect, PopupRow, Bool) -> Void)? {
        didSet { rowView.onDrawRow = onDrawRow }
    }

    public private(set) var rows: [PopupRow] = []
    public var selection = 0 {
        didSet {
            rowView.selection = selection
            rowView.needsDisplay = true
            scrollSelectionIntoView()
        }
    }

    // MARK: Copy selection (config.selectableRows)

    // Rows ticked via their checkbox (or Ctrl+Space on the selected row).
    // Indices track the CURRENT row list; setRows prunes stale ones.
    public var selectedIndices: Set<Int> {
        get { rowView.selected }
        set {
            rowView.selected = newValue.filter { rows.indices.contains($0) }
            rowView.needsDisplay = true
            updateCopyRowsLabel()
        }
    }

    // Host-supplied serializer: picked rows -> clipboard text (e.g. TSV).
    // The framework owns the pasteboard write + header feedback.
    public var onCopyRows: (([PopupRow]) -> String)?

    // Cmd+K in the row list (not the file browser, which keeps its own
    // Cmd+K = copy path): the host usually opens showActionPicker
    public var onCommandK: (() -> Void)?
    // Cmd+F in the row list (list windows only; the notes editor keeps its
    // find bar): e.g. the jira live-search panel
    public var onCommandF: (() -> Void)?

    // rows an action applies to: the ticked rows, else the highlighted one
    public var actionRows: [PopupRow] {
        let idx = rowView.selected.isEmpty ? [selection] : rowView.selected.sorted()
        return idx.filter { rows.indices.contains($0) }.map { rows[$0] }
    }

    // extra header buttons (ids >= 10) and their click callback
    public var headerButtons: [(String, Int)] = [] {
        didSet {
            chrome?.extraButtons = headerButtons
            chrome?.needsDisplay = true
        }
    }
    // left-to-right segment order by button id (see PopupChrome.headerOrder);
    // e.g. [30, 1, 2] puts the host "open file" button before the copy buttons
    public var headerOrder: [Int]? {
        didSet {
            chrome?.headerOrder = headerOrder
            chrome?.needsDisplay = true
        }
    }
    public var onHeaderButton: ((Int) -> Void)?

    // mark an extra header button as ON (its feature/drawer is open) so it
    // renders darker — the host flips this when toggling the terminal / file
    // browser drawer
    private var headerButtonOn: Set<Int> = []
    public func setHeaderButtonOn(_ id: Int, _ on: Bool) {
        if on { headerButtonOn.insert(id) } else { headerButtonOn.remove(id) }
        chrome?.activeButtonIDs = headerButtonOn
        chrome?.needsDisplay = true
    }

    // edit mode: present the text view read-only (detail viewers)
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

    // "copy all" when nothing is ticked, else "copy N"; hidden unless the host
    // turned on selectableRows
    private func updateCopyRowsLabel() {
        guard config.selectableRows, config.copyRowsButton else {
            chrome?.copyRowsLabel = nil
            return
        }
        let n = rowView.selected.count
        chrome?.copyRowsLabel = n == 0 ? "copy selected" : "copy \(n)"
    }

    // Serialize the ticked rows (or every row when none are ticked) and put
    // the result on the pasteboard.
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
    // When the header icon menu is being shown, suppress the active observer
    // so the app's deactivate/reactivate cycle during the menu doesn't trigger
    // makeKeyAndOrderFront and mess up the window's size/position/zoom.
    private var isShowingMenu = false


    // the underlying NSWindow (e.g. for attaching a sheet like the new-note
    // prompt — sheets always appear above their parent window)
    public var nativeWindow: NSWindow { panel }

    private let panel: NSWindow
    private let field: NSTextField
    private let rowView: PopupRowView
    private var editorView: NSTextView?
    // the card-fill tint view over the blur; cached so the color picker can
    // restyle it live (applyThemeColors)
    private var tintView: NSView?
    private var terminalDrawer: LocalProcessTerminalView?
    // tiny side padding for the embedded terminal so its first/last columns
    // never sit flush against the window edges
    private let terminalInset: CGFloat = 4
    // in-note find (Ctrl/Cmd+F): a small field above the editor + its match
    // counter. Query finds in the note's plain text, Enter/Shift+Enter cycle.
    private var findField: NSTextField?
    private var findCountLabel: NSTextField?
    private var findMatches: [NSRange] = []
    private var findIndex = 0
    // polls the shell's health so a dead drawer ALWAYS comes back (the
    // delegate's fast restart can land inside SwiftTerm's windingDown window,
    // where startProcess is silently ignored)
    private var terminalRestartTimer: Timer?
    public private(set) var terminalShown = true
    // embedded file-browser drawer (notes): host installs a PopupFileBrowser;
    // toggled like the terminal, only one drawer is open at a time
    private(set) var fileBrowser: PopupFileBrowser?
    public private(set) var fileBrowserShown = false
    private var fileBrowserDrawerMode = false

    // Pane focus tracking + visual indicators: when the user cycles between
    // editor / file-browser / terminal with Ctrl+J/K (or clicks into one),
    // the active pane gets a bright colored border so it is immediately
    // obvious which surface owns the keyboard.
    enum FocusedPane { case editor, browser, terminal }
    private var focusedPane: FocusedPane?
    private var editorFocusBorder: NSView?
    private var browserFocusBorder: NSView?
    private var terminalFocusBorder: NSView?
    // Bright blue focus indicator — thick border + subtle glow
    // pane focus rings wear the theme accent (was a fixed system blue)
    private var focusBorderColor: NSColor { ButtonStyle.focusStroke(config.colors) }
    private let focusBorderWidth: CGFloat = 3

    // total drawer height currently folded into the window frame (baseline =
    // no drawers); the terminal is on at init when config.terminal is set
    private var drawerInsetNow: CGFloat = 0
    // current drawer heights: what fitDrawersToWindow() could give them
    private var currentTerminalHeight: CGFloat = 0
    private var currentBrowserHeight: CGFloat = 0
    // the heights the user WANTS (config, or Ctrl+Shift+J/K). A resize never
    // changes these — drawers only shrink below them while the window is too
    // small, and spring back as it grows (a pure function of window height,
    // so repeated resizes can never drift)
    private lazy var preferredTerminalHeight: CGFloat = config.terminalHeight
    private lazy var preferredBrowserHeight: CGFloat = config.fileBrowserHeight
    private let minEditorH: CGFloat = 80
    private let minTerminalH: CGFloat = 40
    private let minBrowserH: CGFloat = 50
    private var editorScroll: NSScrollView?
    private var tabsBar: PopupTabsBar?
    private var filterBar: PopupFilterBar?
    // close button target (retained so it survives menu/window dismiss)
    private var windowCloseTarget: WindowCloseTarget?
    // retained restarter so we can wire onTerminated after super.init
    private var terminalRestarter: TerminalAutoRestart?
    // vim mode: the chrome-less editor terminal that replaces the text view
    private var vimView: LocalProcessTerminalView?
    private var vimRestarter: TerminalAutoRestart?
    // set while the window is being torn down so an exiting editor is NOT
    // relaunched
    private var vimShuttingDown = false
    // false while the active tab is a read-only preview (PDF/image): the
    // native editor shows it instead of the vim pane
    private var vimPaneActive = true
    // inline images drawn over the vim pane (click-through overlay)
    private var vimImageOverlay: VimImageOverlay?
    private var vimImageWatch: DispatchSourceFileSystemObject?
    // rapid-Esc streak (see config.escCloseCount)
    private var escStreak = 0
    private var lastEsc = Date.distantPast
    private var rowScroll: NSScrollView?
    // table mode (config.tableColumns): sticky column header over the rows
    private var tableHeader: PopupTableHeaderView?
    // host-owned sort state, mirrored in the header (arrow on that column)
    public var tableSort: (column: Int, ascending: Bool)? {
        didSet {
            tableHeader?.sortColumn = tableSort?.column
            tableHeader?.sortAscending = tableSort?.ascending ?? true
        }
    }
    // a sortable column title was clicked (index into config.tableColumns)
    public var onTableSort: ((Int) -> Void)?
    // a filterable column's ▾ was clicked: (column, header view, the ▾'s
    // rect in it) — the host anchors its filter popover there
    public var onTableFilter: ((Int, NSView, NSRect) -> Void)?
    // columns with an active filter (their ▾ turns into an accent chip)
    public var tableFilterActive: Set<Int> = [] {
        didSet { tableHeader?.activeFilters = tableFilterActive }
    }
    // config.rowStars: the ☆ of row i was clicked
    public var onToggleStar: ((Int) -> Void)?
    // a divider drag changed the column widths (percent); final = mouseUp
    public var onTableColumnsResized: (([CGFloat], Bool) -> Void)?
    private var chrome: PopupChrome?
    // transparent resize edge views that sit ON TOP of all content so drag
    // resize works even when the editor/terminal/browser fills the window
    // transient status strip (prettyprint errors etc.); nil until an editMode
    // window opts into it via setStatus
    private var statusBar: PopupStatusBar?
    private let statusBarHeight: CGFloat = 26
    // top chrome height (search field + filter bar + tab bar) — the scroll
    // view must span from here to the window's bottom
    private var chromeBottom: CGFloat = 0
    private var monitors: [Any] = []
    private var focusRetries = 0

    // title shown in the editor drag header (set before show())
    public var chromeHeaderTitle: String? {
        didSet { chrome?.headerTitle = chromeHeaderTitle }
    }
    // glyph drawn left of the header title pill (app identity)
    public var headerIcon: NSImage? {
        didSet { chrome?.headerIcon = headerIcon }
    }
    // header button labels — set/refresh them as host state changes (active
    // tab, selection); nil leaves the framework default
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

    // live item count drawn dim on the left of the drag header
    public var itemCount: String? {
        didSet {
            chrome?.itemCount = itemCount
            chrome?.needsDisplay = true
        }
    }

    // live mic level 0-1 for the recording control bar; the bar itself is
    // only drawn while meterEnabled (host-driven: voice windows). Toggling it
    // also GROWS/SHRINKS the window by the bar height — the top edge stays
    // put so the editor keeps its size and the space is reclaimed instead of
    // left as a dead strip.
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
            // the meter strip owns the bottom: re-layout the editor, the
            // terminal drawer and the file browser so none of them hides
            // behind the bar (and the browser's [.width, .height] autoresize
            // can't stretch it into the bar's space)
            layoutEditorScroll()
            layoutTerminal()
            layoutFileBrowser()
        }
    }
    public var recordingState: Int = 0 {   // 0 idle, 1 recording, 2 paused, 3 transcribing
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
    // record-bar button clicks (record/stop and pause/resume)
    public var onMeterRecord: (() -> Void)?
    public var onMeterPause: (() -> Void)?

    // click (not drag) on the editor drag header — e.g. copy the file path.
    // Handled at the window level (PopupBaseWindow.sendEvent): the titlebar
    // would otherwise eat the events, and the chrome must not fire it too.
    public var onChromeHeaderClick: (() -> Void)?
    // click on the header's "config" button — e.g. copy the config file path
    public var onChromeConfigClick: (() -> Void)?
    // click on the top-left header app glyph — e.g. open the config file
    public var onChromeIconClick: (() -> Void)?

    // tabs (config.tabs): titles + selection; changing the selection fires
    // onTabChange so the host can swap the content
    public var tabTitles: [String] = [] {
        didSet {
            tabsBar?.titles = tabTitles
            tabsBar?.needsDisplay = true
            relayoutTabs()
        }
    }
    // per-tab status badges, parallel to tabTitles (nil = none)
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
        }
    }
    // dim metadata line in the tab strip under the selected file's pill
    // (e.g. "Last File Write: …") — drawn in the drag header, left side,
    // right after the live item count
    public var tabFooterText: String? {
        didSet {
            chrome?.footerText = tabFooterText
            chrome?.needsDisplay = true
        }
    }
    public var onTabChange: ((Int) -> Void)?
    // every tab click, including the already-selected one (e.g. copy path)
    public var onTabClick: ((Int) -> Void)?
    // "+" pill on the tab strip (config.tabsAddButton) — e.g. create a note
    public var onAddTab: (() -> Void)?
    // the small "✕" badge on a tab pill — host closes/removes that tab
    public var onCloseTab: ((Int) -> Void)?
    // right-click a note tab -> "Copy Path" (replaces the copy-path header
    // button); the host copies that tab's absolute path
    public var onTabCopyPath: ((Int) -> Void)?
    // host hook to open a specific file in this window (e.g. the Finder
    // "Open in Notes" service): the host adds it as a tab / makes it active
    public var onOpenExternalPath: ((String) -> Void)?
    // host prompt for the editor's "Open file at path…" context-menu item
    public var onOpenPathPrompt: (() -> Void)?
    // editor right-click "Copy File Path": copies the open note's absolute
    // path (replaces the dedicated "copy <name> path" header button)
    public var onCopyFilePath: (() -> Void)?
    // terminal drawer right-click "Open in Notes": the host receives the
    // terminal's current selection (a path) and opens it as a note tab
    public var onTerminalOpenInNotes: ((String) -> Void)?
    // terminal drawer right-click "Open in Default App" / "Reveal in Finder":
    // the host acts on the terminal's current selection (a path)
    public var onTerminalOpenDefault: ((String) -> Void)?
    public var onTerminalRevealInFinder: ((String) -> Void)?
    // file-browser right-click "Open in Notes": the host receives the row's
    // absolute path and opens it as a note tab
    public var onFileBrowserOpenInNotes: ((String) -> Void)?

    // filters (config.filters): labels + unique values per dimension (value
    // index 0 = "All"); changing a selection fires onFilterChange
    public var filterLabels: [String] = [] {
        didSet { filterBar?.labels = filterLabels; filterBar?.needsDisplay = true }
    }
    public var filterValues: [[String]] = [] {
        didSet { filterBar?.values = filterValues; filterBar?.needsDisplay = true }
    }
    // display titles for filter dropdown options (parallel to filterValues;
    // empty = show the raw value). Matching always uses filterValues.
    public var filterValueLabels: [[String]] = [] {
        didSet { filterBar?.valueLabels = filterValueLabels; filterBar?.needsDisplay = true }
    }
    // multi-select mode (see PopupFilterBar.onOpen): pill click → host
    // popover anchored at (bar, pill rect); summaries + active drive the look
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
            // a longer label ("release: 13.1 (2026-10-15)") widens its
            // segment; the window grows to fit rather than clip the text
            filterBar?.needsDisplay = true
            growWidthToContent()
            onFilterChange?(filterSelections)
        }
    }
    public var onFilterChange: (([Int]) -> Void)?

    // the search field's current text (for re-filtering after a filter change)
    public var currentQuery: String { field.stringValue }

    // UI zoom: scales fonts, row heights and chrome sizes proportionally to
    // the window. Ctrl/Cmd+± drives it; propagated live to every subview.
    public var zoom: CGFloat = 1.0 {
        didSet {
            guard oldValue != zoom else { return }
            config.zoom = zoom
            rowView.zoom = zoom
            chrome?.zoom = zoom
            tabsBar?.zoom = zoom
            filterBar?.zoom = zoom
            field.font = config.rowFont(config.inputFontSize * zoom)
            if let tv = editorView {
                tv.font = editorFont(config.fontName, zoom, size: config.editorFontSize)
            }
            vimView?.font = PopupWindow.vimFont(config)
            refreshVimImageRows()
            if let chrome {
                chrome.dragHeaderHeight = config.headerHeight * zoom
                chrome.needsDisplay = true
            }
            if let base = panel as? PopupBaseWindow {
                base.headerClickBand = config.headerHeight * zoom
            }
            rowView.needsDisplay = true
            tabsBar?.needsDisplay = true
            filterBar?.needsDisplay = true
            field.needsDisplay = true
            layoutForZoom()
        }
    }

    // Re-frame every subview for the current zoom: bars/pills draw at their
    // scaled size inside frames that must grow with them, and the content
    // (editor scroll / row scroll) must shift down past the taller header.
    private func layoutForZoom() {
        let z = zoom
        guard let backdrop = panel.contentView else { return }
        if config.editMode {
            if let bar = tabsBar {
                bar.frame = NSRect(x: 0, y: config.headerHeight * z + 2,
                                   width: backdrop.bounds.width,
                                   height: config.tabBarHeight * z)
            }
            layoutEditorScroll()
            layoutTerminal()
            layoutFileBrowser()
        } else {
            if config.scrollableRows {
                let headerOffset = (config.dragHeader) ? config.headerHeight * z + 4 : 0
                let fieldFrame = NSRect(x: config.padding + 10,
                                        y: headerOffset + config.padding + 2,
                                        width: config.width - 2 * (config.padding + 10),
                                        height: 24 * z)
                field.frame = fieldFrame
                var cb = fieldFrame.maxY + 4
                if let bar = filterBar {
                    bar.frame = NSRect(x: 0, y: cb, width: backdrop.bounds.width,
                                       height: config.filterBarHeight * z)
                    cb += config.filterBarHeight * z + 2
                }
                if let bar = tabsBar {
                    bar.frame = NSRect(x: 0, y: cb, width: backdrop.bounds.width,
                                       height: config.tabBarHeight * z)
                    cb += config.tabBarHeight * z + 2
                }
                chromeBottom = cb
                layoutScrollDocument()
            } else {
                let headerOffset = (config.dragHeader) ? config.headerHeight * z + 4 : 0
                let fieldFrame = NSRect(x: config.padding + 10,
                                        y: headerOffset + config.padding + 2,
                                        width: config.width - 2 * (config.padding + 10),
                                        height: 24 * z)
                field.frame = fieldFrame
                var cb = fieldFrame.maxY + 4
                if let bar = filterBar {
                    bar.frame = NSRect(x: 0, y: cb, width: backdrop.bounds.width,
                                       height: config.filterBarHeight * z)
                    cb += config.filterBarHeight * z + 2
                }
                if let bar = tabsBar {
                    bar.frame = NSRect(x: 0, y: cb, width: backdrop.bounds.width,
                                       height: config.tabBarHeight * z)
                    cb += config.tabBarHeight * z + 2
                }
                rowView.topInset = cb
            }
        }
        relayoutTabs()
    }


    public init(config: PopupConfig) {
        self.config = config

        // terminal drawer context menu: built in the terminal block below
        // (before super.init, so it can't capture self), then wired after
        // super.init where [weak self] is legal
        var terminalMenuView: LocalProcessTerminalView? = nil
        var terminalMenuTarget: TerminalMenuTarget? = nil
        var terminalMenu: NSMenu? = nil

        // Note/list windows get a (visually transparent) titlebar: AeroSpace's
        // isWindowHeuristic treats accessory apps without an AX close button
        // as "not a window", so they'd be invisible to focus commands and
        // Alt+hjkl. .titled + .closable exposes the AX close button; the
        // titlebar itself is hidden (fullSizeContentView + hidden title) and
        // only the red close button remains visible.
        let wantsTitlebar = config.editMode || config.enableDrag
        let initialHeight = config.padding * 2 + config.headerHeight * zoom + config.rowHeight * zoom
        if wantsTitlebar {
            // NO .nonactivatingPanel here: these windows must activate the app
            // when AeroSpace (or a click) focuses them, otherwise alt-j/k and
            // alt-shift-j/k raise the window but keyboard focus stays behind.
            // Resizing is NATIVE (.resizable): the window server's own live
            // resize from every edge + corner (incl. the top), with the right
            // cursors and no competing hand-rolled drag math. Content
            // re-lays out in windowDidResize.
            var mask: NSWindow.StyleMask = [.titled, .closable, .fullSizeContentView]
            if config.enableResize { mask.insert(.resizable) }
            panel = PopupPlainWindow(
                contentRect: NSRect(x: 0, y: 0, width: config.width, height: initialHeight),
                styleMask: mask,
                backing: .buffered, defer: false)
            panel.contentMinSize = NSSize(width: 320, height: 220)
            (panel as? PopupPlainWindow)?.cornerRadius = config.cornerRadius
            panel.titlebarAppearsTransparent = true
            panel.titleVisibility = .hidden
            // traffic lights hidden — the close button exists only so the
            // AeroSpace heuristic accepts the window; Esc closes it, not the X
            // (unless showCloseButton is set, e.g. for vim mode)
            let hideClose = !config.showCloseButton
            for type: NSWindow.ButtonType in [.miniaturizeButton, .zoomButton] {
                panel.standardWindowButton(type)?.isHidden = true
            }
            if hideClose {
                panel.standardWindowButton(.closeButton)?.isHidden = true
            }
        } else {
            panel = PopupPanel(
                contentRect: NSRect(x: 0, y: 0, width: config.width, height: initialHeight),
                styleMask: [.borderless, .nonactivatingPanel],
                backing: .buffered, defer: false)
        }
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = config.hasShadow
        panel.level = config.floating ? .popUpMenu : .normal
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        panel.isReleasedWhenClosed = false
        panel.title = config.name

        // Wire the close button when shown (e.g. vim mode): onClick calls
        // onCloseWindow if set, otherwise falls back to hide(restore: true).
        // The target is created here (before super.init) but the closure is
        // wired AFTER super.init.
        if config.showCloseButton, let closeBtn = panel.standardWindowButton(.closeButton) {
            let closeTarget = WindowCloseTarget()
            closeBtn.target = closeTarget
            closeBtn.action = #selector(WindowCloseTarget.close)
            windowCloseTarget = closeTarget
        }

        // Backdrop: rounded container that clips a blurred material + tint,
        // for a sleek translucent look with real see-through corners.
        let backdrop = PopupBackdrop(config: config,
                                     frame: NSRect(x: 0, y: 0, width: config.width, height: initialHeight))
        backdrop.autoresizingMask = [.width, .height]
        backdrop.wantsLayer = true
        backdrop.layer?.cornerRadius = config.cornerRadius
        backdrop.layer?.masksToBounds = true
        backdrop.layer?.borderWidth = 1
        backdrop.layer?.borderColor = config.colors.border.cgColor

        // allow mouse-moved for hover feedback (file-browser rows, resize
        // cursor feedback) — cheap, and the tracking areas scope the events
        panel.acceptsMouseMovedEvents = true
        if config.enableResize {
            // borderless: resize handled by dragging edges/corners on the
            // backdrop; just allow mouse-moved for cursor feedback
            panel.acceptsMouseMovedEvents = true
        }

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

        rowView = PopupRowView(config: config)
        rowView.frame = backdrop.bounds
        rowView.autoresizingMask = [.width, .height]
        field = NSTextField(frame: NSRect(x: config.padding + 10, y: config.padding + 2,
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
            // replacing the cell RESETS its editability/selectability (cell
            // defaults are non-editable) — restore them or the field can
            // never become first responder
            let fieldCell = PopupSearchFieldCell()
            fieldCell.hInset = 8
            field.cell = fieldCell
            field.isEditable = config.enableSearch
            field.isSelectable = config.enableSearch
            // the cell swap also resets focusRingType — kill the ring AGAIN
            // or the loud blue macOS ring returns on top of our hairline
            field.focusRingType = .none
            field.wantsLayer = true
            field.layer?.backgroundColor = ButtonStyle.inputFill(config.colors).cgColor
            field.layer?.cornerRadius = 6
            // themed hairline instead of the system focus ring (killed below
            // in controlTextDidBeginEditing) — the blue macOS ring reads as
            // an error state on the dark bar
            field.layer?.borderWidth = 1
            field.layer?.borderColor = ButtonStyle.inputStroke(config.colors).cgColor
            field.placeholderAttributedString = NSAttributedString(
                string: "search…",
                attributes: [
                    .font: config.rowFont(config.inputFontSize * zoom),
                    .foregroundColor: config.colors.dim,
                ])
        }

        if config.editMode {
            // plain-text editor: scrollable NSTextView below the drag header
            // (+ tab strip if enabled)
            let topY = config.headerHeight * zoom + (config.tabs ? config.tabBarHeight * zoom + 2 : 0)
            let scroll = NSScrollView(frame: NSRect(x: 0, y: topY,
                                                    width: backdrop.bounds.width,
                                                    height: backdrop.bounds.height - topY))
            scroll.autoresizingMask = [.width]
            scroll.hasVerticalScroller = true
            // photos are fitted to the editor width (see makeAttachment), so
            // the document never overflows horizontally — no horizontal bar
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
            // Force the selection highlight colors (Ctrl+A select-all, the
            // find bar's match jump). The system default follows the OS
            // appearance/accent, so the SAME binary renders differently per
            // machine — e.g. white text on a light-mode selection is
            // unreadable. Pin selection to the configured highlight + text
            // colors so it always contrasts, regardless of the machine.
            tv.selectedTextAttributes = ButtonStyle.selection(config.colors)
            tv.backgroundColor = .clear
            tv.drawsBackground = false
            tv.textContainerInset = NSSize(width: 10, height: 10)
            tv.autoresizingMask = [.width]
            if config.markdownImages {
                // text wraps at the window width, but the VIEW may grow wider
                // than the clip when a natural-size photo needs the room —
                // that growth is what the horizontal scroller scrolls
                tv.isHorizontallyResizable = true
                tv.maxSize = NSSize(width: 4096,
                                    height: CGFloat.greatestFiniteMagnitude)
                tv.textContainer?.widthTracksTextView = false
                tv.textContainer?.containerSize = NSSize(
                    width: max(120, scroll.bounds.width - 24),
                    height: CGFloat.greatestFiniteMagnitude)
            }
            scroll.documentView = tv
            backdrop.addSubview(scroll)
            editorView = tv
            editorScroll = scroll
            // focus indicator: bright 4-sided border around the editor scroll
            let efb = NSView(frame: scroll.frame)
            efb.autoresizingMask = [.width, .height]
            efb.wantsLayer = true
            efb.layer?.borderWidth = focusBorderWidth
            efb.layer?.borderColor = ButtonStyle.focusStroke(config.colors).cgColor
            efb.layer?.cornerRadius = 6
            efb.isHidden = true
            backdrop.addSubview(efb)
            editorFocusBorder = efb
            // vim mode: a chrome-less terminal running the editor sits exactly
            // over the text view (layoutEditorScroll keeps them in sync) and
            // blends into the notepad — no border, the card color shows
            // through. The process starts on first show, once the pane has
            // its real size.
            if config.vimEditorExecutable != nil {
                let vv = LocalProcessTerminalView(frame: scroll.frame)
                vv.font = PopupWindow.vimFont(config)
                vv.nativeBackgroundColor = .clear
                vv.nativeForegroundColor = config.colors.text
                vv.wantsLayer = true
                vv.layer?.backgroundColor = NSColor.clear.cgColor
                let vr = TerminalAutoRestart()
                vv.processDelegate = vr
                vimRestarter = vr
                // sits below the focus border so the ring stays visible
                backdrop.addSubview(vv, positioned: .below, relativeTo: efb)
                vimView = vv
                scroll.isHidden = true
                if config.vimImageFile != nil {
                    let ov = VimImageOverlay(frame: vv.frame)
                    backdrop.addSubview(ov, positioned: .above, relativeTo: vv)
                    vimImageOverlay = ov
                }
            }
            if config.tabs {
                let bar = PopupTabsBar(config: config)
                bar.frame = NSRect(x: 0, y: config.headerHeight * zoom + 2,
                                   width: backdrop.bounds.width, height: config.tabBarHeight * zoom)
                bar.autoresizingMask = [.width]
                backdrop.addSubview(bar)
                tabsBar = bar
            }
            // in-note find bar (Ctrl/Cmd+F): hidden until toggled; the query
            // field matches the search-field styling
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
            backdrop.addSubview(ff)
            findField = ff
            let fc = NSTextField(labelWithString: "")
            fc.font = config.rowFont(config.inputFontSize * zoom)
            fc.textColor = config.colors.dim
            fc.isHidden = true
            backdrop.addSubview(fc)
            findCountLabel = fc
            // bottom status strip (lazy: hidden until the host calls setStatus)
            let sb = PopupStatusBar(config: config)
            sb.frame = NSRect(x: 6, y: backdrop.bounds.height - statusBarHeight - 4,
                              width: max(0, backdrop.bounds.width - 12),
                              height: statusBarHeight)
            sb.autoresizingMask = [.width]
            sb.isHidden = true
            backdrop.addSubview(sb)
            statusBar = sb
            if config.terminal {
                drawerInsetNow = config.terminalHeight
                // embedded shell drawer at the bottom: the editor stops above
                // it (layoutEditorScroll), the session survives hide/show
                let term = LocalProcessTerminalView(frame: NSRect(
                    x: terminalInset,
                    y: backdrop.bounds.height - config.terminalHeight,
                    width: max(0, backdrop.bounds.width - 2 * terminalInset),
                    height: config.terminalHeight))
                term.autoresizingMask = [.width]
                if let tf = NSFont(name: config.terminalFont, size: config.terminalFontSize) {
                    term.font = tf
                }
                // softly rounded drawer corners (sits inset in the backdrop)
                term.wantsLayer = true
                term.layer?.cornerRadius = 8
                term.layer?.masksToBounds = true
                // translucent default background, matching the notepad: the
                // silvery-blue panel color (config.terminalBackground) carries
                // its own alpha — the color picker's opacity slider sets it
                term.nativeBackgroundColor = config.terminalBackground
                term.nativeForegroundColor = config.terminalForeground ?? config.colors.text
                backdrop.addSubview(term)
                terminalDrawer = term
                term.installColors(PopupWindow.ansiPalette(config.colors))
                currentTerminalHeight = config.terminalHeight
                // focus indicator: bright 4-sided border around the terminal
                let tfb = NSView(frame: term.frame)
                tfb.autoresizingMask = [.width]
                tfb.wantsLayer = true
                tfb.layer?.borderWidth = focusBorderWidth
                tfb.layer?.borderColor = ButtonStyle.focusStroke(config.colors).cgColor
                tfb.layer?.cornerRadius = 8
                tfb.isHidden = true
                backdrop.addSubview(tfb)
                terminalFocusBorder = tfb
                // auto-restart: if the shell exits (user typed exit/ctrl-d)
                // spawn it again after a beat so the drawer is never dead
                // (capture the shell locally — no self before super.init)
                let exec = config.shell
                let execArgs = config.shellArgs
                let terminalDir = config.terminalDir
                let restarter = TerminalAutoRestart()
                // onTerminated is wired AFTER super.init (see below) to avoid
                // capturing self before initialization completes
                term.processDelegate = restarter
                terminalRestarter = restarter
                term.processDelegate = restarter
                term.startProcess(executable: exec, args: execArgs,
                                  currentDirectory: terminalDir)
                // right-click menu: paste / copy / select-all straight to the
                // shell (SwiftTerm owns the clipboard read/write), plus
                // "Open in Notes" — the current selection (a path) becomes a
                // note tab via the host hook. The closures that capture self
                // are wired after super.init (see below).
                let tmenu = NSMenu(title: "Terminal")
                func titem(_ t: String, _ sel: Selector, _ key: String) -> NSMenuItem {
                    let i = NSMenuItem(title: t, action: sel, keyEquivalent: key)
                    i.target = term
                    return i
                }
                let menuTarget = TerminalMenuTarget()
                let copyItem = NSMenuItem(title: "Copy",
                                          action: #selector(TerminalMenuTarget.copySelection(_:)),
                                          keyEquivalent: "c")
                copyItem.target = menuTarget
                let openItem = NSMenuItem(title: "Open in Notes",
                                          action: #selector(TerminalMenuTarget.openSelectionInNotes(_:)),
                                          keyEquivalent: "")
                openItem.target = menuTarget
                let openDefaultItem = NSMenuItem(title: "Open in Default App",
                                                 action: #selector(TerminalMenuTarget.openSelectionInDefaultApp(_:)),
                                                 keyEquivalent: "")
                openDefaultItem.target = menuTarget
                let revealItem = NSMenuItem(title: "Reveal in Finder",
                                            action: #selector(TerminalMenuTarget.revealSelectionInFinder(_:)),
                                            keyEquivalent: "")
                revealItem.target = menuTarget
                tmenu.addItem(titem("Paste", #selector(LocalProcessTerminalView.paste(_:)), "v"))
                tmenu.addItem(copyItem)
                tmenu.addItem(NSMenuItem.separator())
                tmenu.addItem(openItem)
                tmenu.addItem(openDefaultItem)
                tmenu.addItem(revealItem)
                tmenu.addItem(NSMenuItem.separator())
                tmenu.addItem(titem("Select All", #selector(LocalProcessTerminalView.selectAll(_:)), "a"))
                terminalMenuView = term
                terminalMenuTarget = menuTarget
                terminalMenu = tmenu
                // safety net: poll the shell's state; restart once the old
                // session is fully wound down (running==false && windingDown==false)
                let cfgShell = config.shell
                let cfgShellArgs = config.shellArgs
                let cfgTerminalDir = config.terminalDir
                let poll = Timer(timeInterval: 1.5, repeats: true) { [weak term] _ in
                    guard let term, let p = term.process else { return }
                    if !p.running, !p.windingDown {
                        term.startProcess(executable: cfgShell, args: cfgShellArgs,
                                          currentDirectory: cfgTerminalDir)
                    }
                }
                RunLoop.main.add(poll, forMode: .common)
                terminalRestartTimer = poll
                if !config.terminalStartsOpen {
                    // session still spawns (ready on toggle), drawer closed
                    terminalShown = false
                    drawerInsetNow = 0
                }
            }
        }

        // search list: chrome (drag header, search field, filter bar, tab
        // strip) fixed on top; rows below — directly (default) or in a
        // scroll view. Non-scroll keeps chrome as rowView subviews so
        // clicks reach them; scroll puts them on the backdrop above the
        // scroll view.
        if !config.editMode {
            let fieldH: CGFloat = 24 * zoom
            let headerOffset = (config.editMode || config.dragHeader)
                ? config.headerHeight * zoom + 4 : 0
            let fieldFrame = NSRect(x: config.padding + 10, y: headerOffset + config.padding + 2,
                                    width: (config.width - 2 * (config.padding + 10))
                                        * config.searchWidthFraction,
                                    height: fieldH)
            var chromeBottom: CGFloat = fieldFrame.maxY + 4
            let scrollable = config.scrollableRows
            if scrollable {
                field.frame = fieldFrame
                field.autoresizingMask = [.width]
                backdrop.addSubview(field)
                if config.filters {
                    let bar = PopupFilterBar(config: config)
                    bar.frame = NSRect(x: 0, y: chromeBottom, width: config.width,
                                       height: config.filterBarHeight * zoom)
                    bar.autoresizingMask = [.width]
                    backdrop.addSubview(bar)
                    filterBar = bar
                    chromeBottom += config.filterBarHeight * zoom + 2
                }
                if config.tabs {
                    let bar = PopupTabsBar(config: config)
                    bar.frame = NSRect(x: 0, y: chromeBottom, width: config.width,
                                       height: config.tabBarHeight * zoom)
                    bar.autoresizingMask = [.width]
                    backdrop.addSubview(bar)
                    tabsBar = bar
                    chromeBottom += config.tabBarHeight * zoom + 2
                }
                // IMPORTANT: the scroll view's height must always equal
                // windowHeight - chromeBottom. The autoresizing mask alone
                // grows it by the FULL window delta, so once the window is
                // taller than the initial setup height the scroll view
                // overshoots the window bottom (scrollbar + last rows hang
                // off-screen). layoutScrollDocument re-pins the frame on
                // every resize/show.
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
                rowView.bottomInset = 16   // room for the pill border stroke
                if !config.tableColumns.isEmpty {
                    let header = PopupTableHeaderView(config: config)
                    header.frame = NSRect(x: 0, y: 0, width: backdrop.bounds.width,
                                          height: config.tableHeaderHeight * zoom)
                    header.autoresizingMask = [.width]
                    tableHeader = header
                    rowView.topInset = config.tableHeaderHeight * zoom + 2
                }
                // + breathing room below the last row (the scroll view now
                // ends exactly at the window bottom, so no huge inset needed)
scroll.documentView = rowView
                if let header = tableHeader {
                    // floating: pinned to the top of the visible rows,
                    // never scrolls away
                    scroll.addFloatingSubview(header, for: .vertical)
                }
                backdrop.addSubview(scroll)
                rowScroll = scroll
            } else {
                rowView.frame = backdrop.bounds
                rowView.autoresizingMask = [.width, .height]
                field.frame = fieldFrame
                field.autoresizingMask = [.width]
                rowView.addSubview(field)
                if config.filters {
                    let bar = PopupFilterBar(config: config)
                    bar.frame = NSRect(x: 0, y: chromeBottom, width: config.width,
                                       height: config.filterBarHeight * zoom)
                    bar.autoresizingMask = [.width]
                    rowView.addSubview(bar)
                    filterBar = bar
                    chromeBottom += config.filterBarHeight * zoom + 2
                }
                if config.tabs {
                    let bar = PopupTabsBar(config: config)
                    bar.frame = NSRect(x: 0, y: chromeBottom, width: config.width,
                                       height: config.tabBarHeight * zoom)
                    bar.autoresizingMask = [.width]
                    rowView.addSubview(bar)
                    tabsBar = bar
                    chromeBottom += config.tabBarHeight * zoom + 2
                }
                rowView.topInset = chromeBottom
                backdrop.addSubview(rowView)
            }
        }

        // Chrome overlay (drag header for editors / drag-anywhere for list
        // windows + resize edges). Must be the topmost subview so its hitTest
        // gets first shot; non-chrome areas fall through to the content.
        if config.editMode || config.enableDrag {
            let chrome = PopupChrome(config: config)
            // header clicks (incl. the ✕) route through PopupBaseWindow's
            // click band; borderless panels have none, so no glyph there
            if !(panel is PopupBaseWindow) { chrome.config.headerCloseButton = false }
            chrome.frame = backdrop.bounds
            chrome.autoresizingMask = [.width, .height]
            if config.editMode || config.dragHeader {
                chrome.dragHeaderHeight = config.headerHeight * zoom
                chrome.headerTitle = chromeHeaderTitle
            } else if config.scrollableRows {
                // the scroll view owns the row area (scrolling), so the
                // chrome only claims the resize edges; window drags use the
                // hidden titlebar natively
                chrome.dragAnywhere = false
            } else {
                chrome.dragAnywhere = true
                // pass-through for interactive controls: search field + tab
                // strip (everything above the first row)
                let top = config.padding + 2
                let bottom = top + 24 * zoom + 4 + (config.tabs ? config.tabBarHeight * zoom + 2 : 0)
                chrome.reservedRect = NSRect(x: 0, y: top, width: config.width,
                                             height: bottom - top)
            }
            backdrop.addSubview(chrome)
            self.chrome = chrome
        }

        panel.contentView = backdrop

        super.init()
        // theme-pinned appearance + text selection (needs the drawers built)
        applyThemeAppearance()

        // Wire the terminal restarter (self-safe now): auto-restart the shell.
        if let restarter = terminalRestarter {
            let terminalDir = config.terminalDir
            restarter.onTerminated = { [weak self] in
                guard let self else { return }
                terminalDrawer?.startProcess(executable: config.shell, args: config.shellArgs,
                                             currentDirectory: terminalDir)
            }
        }
        // vim pane: an exiting editor (`:q`) fires onVimExit and is relaunched
        // on the current note — the pane is never left dead. The delegate
        // callback may arrive off the main thread.
        if let vr = vimRestarter {
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
        }

        // Wire the close button (self-safe now)
        if config.showCloseButton, let _ = panel.standardWindowButton(.closeButton),
           let closeTarget = windowCloseTarget {
            closeTarget.onClose = { [weak self] in
                if let onCloseWindow = self?.onCloseWindow {
                    onCloseWindow()
                } else {
                    self?.hide(restore: true)
                }
            }
        }

        // "Open file at path…" editor context-menu item -> host prompt
        (editorView as? PopupTextView)?.onOpenFileAtPath = { [weak self] in
            self?.onOpenPathPrompt?()
        }
        (editorView as? PopupTextView)?.onCopyFilePath = { [weak self] in
            self?.onCopyFilePath?()
        }
        // terminal drawer right-click actions (self-safe only after super.init):
        // "Copy" copies the selection; "Open in Notes" / "Open in Default
        // App" / "Reveal in Finder" forward the selected text (a path) to the
        // host hooks.
        if let term = terminalMenuView, let menuTarget = terminalMenuTarget {
            menuTarget.copy = { [weak term] in
                term?.copy(NSNull())
            }
            // the selection must become a real path before acting on it —
            // copy to the general pasteboard, then read it back (never
            // clobber the clipboard on an empty right-click)
            let copySelectionToPasteboard: () -> String? = { [weak term] in
                guard let term, term.selectedRange().length > 0 else { return nil }
                term.copy(NSNull())
                return NSPasteboard.general.string(forType: .string)?
                    .trimmingCharacters(in: .whitespacesAndNewlines)
            }
            menuTarget.openInNotes = { [weak self] in
                guard let text = copySelectionToPasteboard(), !text.isEmpty else { return }
                self?.onTerminalOpenInNotes?(text)
            }
            menuTarget.openInDefault = { [weak self] in
                guard let text = copySelectionToPasteboard(), !text.isEmpty else { return }
                self?.onTerminalOpenDefault?(text)
            }
            menuTarget.revealInFinder = { [weak self] in
                guard let text = copySelectionToPasteboard(), !text.isEmpty else { return }
                self?.onTerminalRevealInFinder?(text)
            }
        }
        if let term = terminalMenuView, let tmenu = terminalMenu {
            term.menu = tmenu
        }
        if let bar = tabsBar {
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
            }
        }
        if let bar = filterBar {
            bar.onSelect = { [weak self] dim, vi in
                guard let self, self.filterSelections.indices.contains(dim) else { return }
                var sels = self.filterSelections
                sels[dim] = vi
                self.filterSelections = sels
            }
        }
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
            updateCopyRowsLabel()
        }
        tableHeader?.onSort = { [weak self] i in self?.onTableSort?(i) }
        tableHeader?.onFilter = { [weak self] i, r in
            guard let self, let h = self.tableHeader else { return }
            self.onTableFilter?(i, h, r)
        }
        rowView.onToggleStar = { [weak self] i in self?.onToggleStar?(i) }
        tableHeader?.onResize = { [weak self] pcts, final in
            guard let self else { return }
            var cols = self.config.tableColumns
            for i in cols.indices where i < pcts.count { cols[i].width = pcts[i] }
            self.setTableColumns(cols)
            self.onTableColumnsResized?(pcts, final)
        }
        chrome?.onMeterRecord = { [weak self] in self?.onMeterRecord?() }
        chrome?.onMeterPause = { [weak self] in self?.onMeterPause?() }
        // wired here (not at editor creation): the closure captures self, which
        // is unavailable until every stored property is initialized
        (editorView as? PopupTextView)?.onPasteImage = { [weak self] img in
            guard let self, self.config.markdownImages,
                  let rel = self.imageSaver?(img) else { return }
            self.insertImageAttachment(rel: rel, image: img)
        }
        (editorView as? PopupTextView)?.absolutePathAt = { [weak self] idx in
            guard let self, let tv = self.editorView,
                  let storage = tv.textStorage, idx < storage.length else { return nil }
            let attrs = storage.attributes(at: idx, effectiveRange: nil)
            guard let att = attrs[.attachment] as? NSTextAttachment,
                  let rel = self.attachmentPaths[att] else { return nil }
            return (self.imageBaseDir as NSString).appendingPathComponent(rel)
        }
        panel.delegate = self
        (panel as? EscapableWindow)?.onEscape = { [weak self] in
            self?.handleEscape()
        }
        if !config.editMode {
            field.delegate = self
        }
        // find bar delegate (edit mode only) — set after super.init like field
        findField?.delegate = self
        // header clicks never reach the chrome (the invisible titlebar eats
        // them) — intercept them at the window level instead; the click
        // position decides which header button was hit
        if (config.editMode || config.dragHeader), let base = panel as? PopupBaseWindow {
            base.headerClickBand = config.headerHeight * zoom
            base.onHeaderClick = { [weak self] point in
                guard let self else { return }
                // window coords (bottom-left) -> chrome coords (flipped)
                let p = NSPoint(x: point.x, y: self.panel.frame.height - point.y)
                if let chrome = self.chrome, chrome.closeButtonRect.insetBy(dx: -2, dy: -2).contains(p) {
                    // ✕ glyph: the host's close path (same as Esc)
                    if let onCloseWindow = self.onCloseWindow { onCloseWindow() } else { self.handleEscape() }
                } else if let chrome = self.chrome,
                   let hit = chrome.extraButtonRects.first(where: { $0.value.contains(p) }) {
                    self.onHeaderButton?(hit.key)
                    chrome.showCopiedFeedback(hit.key)
                } else if let chrome = self.chrome,
                   chrome.copyRowsLabel != nil,
                   chrome.copyRowsButtonRect.contains(p) {
                    self.performCopyRows()
                    chrome.showCopiedFeedback(3)
                } else if let chrome = self.chrome,
                   chrome.configButtonRect.contains(p) {
                    self.onChromeConfigClick?()
                    chrome.showCopiedFeedback(2)
                } else if let chrome = self.chrome,
                   chrome.copyButtonRect.contains(p) {
                    // only the actual "copy path" SEGMENT acts as the copy
                    // button — empty header space must not trigger anything
                    self.onChromeHeaderClick?()
                    chrome.showCopiedFeedback(1)
                } else if let chrome = self.chrome,
                   chrome.headerIcon != nil, chrome.iconButtonRect.insetBy(dx: -4, dy: -4).contains(p) {
                    // click on the top-left app glyph (drawn at x=10..26) —
                    // the host e.g. opens the config file in the viewer
                    self.onChromeIconClick?()
                }
            }
        }
        panel.orderOut(nil)
    }

    // MARK: Lifecycle

    public func start() {
        if config.enableToggle {
            startToggleServer()
        }
    }

    public func show() {
        presentList()
    }

    private func presentList() {
        // let the host refresh its data (e.g. re-query window state) before
        // the rows are rebuilt — so re-shows never render stale entries
        onShow?()
        if config.editMode {
            if let attr = editorAttributed {
                editorView?.textStorage?.setAttributedString(attr)
            } else {
                editorView?.string = editorText
            }
            editorView?.isEditable = !editorReadOnly
            wireEditorTextChange()
            let h = min(config.height, maxPanelHeight())
            let origin = centeredOrigin(width: config.width, height: h)
            panel.setContentSize(NSSize(width: config.width, height: h))
            panel.setFrameOrigin(origin)
            // the window was just resized to its real height — re-frame the
            // editor, terminal and file-browser drawers to that final size
            // (otherwise the browser stretches to fill the window on first
            // paint and the editor is left at its tiny init frame)
            layoutForZoom()
            isShown = true
            installMonitors()
            focusRetries = 0
            // vim pane: start the editor NOW that it has its final size
            startVimIfNeeded()
            // initial focus: editor (or the vim pane) gets the highlight
            if let ed = primaryEditor {
                panel.makeFirstResponder(ed)
                focusedPane = .editor
            }
            updateFocusIndicator()
            takeFocus()
            return
        }
        let initial = onFilter?("") ?? []
        setRows(initial)
        field.stringValue = ""
        rowView.highlightQuery = ""

        // scrollable windows keep a fixed height (config.height, else their
        // current content height) — rows scroll inside instead of growing it
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
        // clamp after initial placement: the frame must never extend past the
        // screen (title bar, drag header, or any chrome can push the window
        // geometry off the visible area)
        panel.setFrame(clampToScreen(panel.frame), display: true)
        if config.scrollableRows {
            // document view holds the FULL content height; the scroll view
            // clips and scrolls it
            rowView.frame = NSRect(x: 0, y: 0, width: config.width, height: contentH)
        } else {
            rowView.frame = NSRect(x: 0, y: 0, width: config.width, height: height)
        }
        rowView.sizingRowCount = rows.count
        rowView.needsDisplay = true

        isShown = true
        installMonitors()
        focusRetries = 0
        relayoutTabs()          // wrap the tab strip at the final window width
        layoutSearchField()     // search field ~80% width, centered
        growWidthToContent()    // never show a clipped label on first paint
        takeFocus()
    }

    public func hide(restore: Bool) {
        guard isShown else { return }
        isShown = false
        // who hid it? (diagnosing windows vanishing on TCC permission prompts)
        let front = NSWorkspace.shared.frontmostApplication?.bundleIdentifier ?? "?"
        let caller = Thread.callStackSymbols.dropFirst().prefix(4)
            .map { $0.split(separator: " ", omittingEmptySubsequences: true).dropFirst(3).prefix(1).joined() }
            .joined(separator: " < ")
        let line = "ws: hide '\(config.name)' restore=\(restore) front=\(front) via \(caller)\n"
        if let fh = try? FileHandle(forWritingTo: URL(fileURLWithPath: "/tmp/ws-debug.log")) {
            fh.seekToEndOfFile()
            fh.write(Data(line.utf8))
            try? fh.close()
        }
        removeMonitors()
        panel.orderOut(nil)
        if config.editMode, editorView != nil {
            onEditorClose?(currentEditorText)
        }
        onHideVoiceStop?()
        onHide?(restore)
    }

    // Re-show a persistent (hidden-but-alive) window. The panel was orderOut'd
    // on hide but the PopupWindow itself survived (the host keeps the notes
    // window as a singleton), so we only re-assert visibility and re-install
    // the monitors — editor text, scroll position and the embedded terminal
    // session are untouched.
    public func showPersistent() {
        guard !isShown else { return }
        onShow?()
        isShown = true
        installMonitors()
        focusRetries = 0
        relayoutTabs()
        layoutEditorScroll()
        // an editor that died while hidden comes back on the current note
        startVimIfNeeded()
        panel.makeKeyAndOrderFront(nil)
        takeFocus()
    }

    // Break the host-hook retain cycles so a hidden sub-window (and whatever
    // its closures captured — e.g. a VoiceRecorder holding the mic) can be
    // deallocated. Hosts store strong closures ON the window (w.onMeterRecord
    // captures w itself), so without this a closed window leaks forever and a
    // leaked AVAudioEngine keeps the microphone busy.
    public func releaseHooks() {
        onShow = nil
        onFilter = nil
        onTableFilter = nil
        onToggleStar = nil
        onFilterOpen = nil
        onAccept = nil
        onRowClick = nil
        onRowDoubleClick = nil
        onEscape = nil
        onHide = nil
        onHideVoiceStop = nil
        onEditorCommit = nil
        onEditorClose = nil
        onDrawRow = nil          // didSet also clears rowView.onDrawRow
        onCopyRows = nil
        onCommandK = nil
        onCommandF = nil
        closeActionPicker()
        onHeaderButton = nil
        onMeterRecord = nil
        onMeterPause = nil
        onChromeHeaderClick = nil
        onChromeConfigClick = nil
        onTabChange = nil
        onTabClick = nil
        onAddTab = nil
        onFilterChange = nil
    }

    public func toggle() {
        if isShown {
            hide(restore: true)
        } else {
            show()
        }
    }

    // Pop up a menu anchored at the top-left header icon position. The host
    // builds the menu (with checkmarks, actions, submenus) and calls this from
    // onChromeIconClick — the menu appears right under the app glyph.
    public func showHeaderMenu(_ menu: NSMenu) {
        // Suppress the active observer while the menu is open: when the menu
        // pops up the app briefly deactivates/reactivates, and the observer's
        // makeKeyAndOrderFront + makeFirstResponder would fight the menu,
        // causing the window to jump in size/position and mess up zoom.
        isShowingMenu = true
        chrome?.iconMenuOpen = true
        defer { chrome?.iconMenuOpen = false }
        // icon sits at x=10, y=top of window; pop down 4pts below the header
        let pt = NSPoint(x: (chrome?.iconButtonRect.minX ?? 6) + 4, y: panel.frame.height - config.headerHeight * zoom - 4)
        let screenPt = panel.convertPoint(toScreen: pt)
        menu.popUp(positioning: nil, at: screenPt, in: nil)
        isShowingMenu = false
    }

    // Reset the window back to its configured default size and re-layout
    // all sub-panes (editor, terminal, browser). Called from the app menu.
    public func resetToDefaultSize() {
        zoom = 1
        var h = config.height
        if config.editMode {
            // config.height folds in the drawer that opens at launch; swap it
            // for the drawers open NOW at their preferred heights
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

    // Reset all theme colors back to their defaults (browser panel, terminal
    // drawer, notepad background, header tint). Called from the app menu.
    public func resetToDefaultColors() {
        let base = PopupConfig(name: "")
        setThemeColor(base.fileBrowserBackground, for: .browser)
        setThemeColor(base.terminalBackground, for: .terminal)
        // notepad: default background with default tintAlpha
        let notepadDefault = base.colors.background.withAlphaComponent(base.tintAlpha)
        setThemeColor(notepadDefault, for: .notepad)
        // header: nil = use window background (no custom tint)
        config.headerColor = nil
        chrome?.headerColorOverride = config.colors.background
        panel.contentView?.needsDisplay = true
    }

    public func setRows(_ newRows: [PopupRow], resetScroll: Bool = true) {
        rows = newRows
        if selection >= rows.count {
            selection = max(0, rows.count - 1)
        }
        if config.selectableRows {
            // a re-filter can drop ticked rows; keep indices valid
            rowView.selected = rowView.selected.filter { rows.indices.contains($0) }
            updateCopyRowsLabel()
        }
        rowView.rows = rows
        rowView.selection = selection
        rowView.needsDisplay = true
        if config.scrollableRows {
            // never shrink the window — the scroll view absorbs overflow;
            // the document tracks the current width so rows widen on resize
            layoutScrollDocument()
            rowView.sizingRowCount = rows.count
            // new content (tab switch / new filter): reset the scroll
            // position. On-disk reloads pass resetScroll=false so the view
            // keeps its place instead of yanking back to the top.
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

    // Scrollable rows: KEYBOARD navigation keeps the selected row at a FIXED
// relative position (~45% down) — the list scrolls behind it and the
// selection stays put (the model fzf/editors use), clamped at the ends.
// Clicks just keep the row visible (no jump).
private func scrollSelectionIntoView() {
        guard config.scrollableRows, isShown,
              let scroll = rowScroll, selection >= 0, selection < rows.count,
              let rv = rowScroll?.documentView as? PopupRowView else { return }
        let r = rv.rect(for: selection)
        let vis = scroll.documentVisibleRect
        let inset: CGFloat = 8
        // minimal edge-scrolling: only move when the selection is OFF-SCREEN,
        // and only just enough to bring it to the nearest edge — no
        // re-anchoring of visible rows (that caused the jarring "jump")
        let targetY: CGFloat
        if r.minY < vis.minY {
            targetY = r.minY - inset                       // above -> top edge
        } else if r.maxY > vis.maxY {
            targetY = r.maxY - vis.height + inset          // below -> bottom edge
        } else {
            return
        }
        let maxTarget = max(0, (scroll.documentView?.frame.height ?? 0) - vis.height)
        let clamped = min(max(0, targetY), maxTarget)
        guard abs(clamped - vis.minY) > 2 else { return }
        if abs(clamped - vis.minY) > 48 {
            // long keyboard jumps glide instead of teleporting (wheel/
            // trackpad scrolling stays fully native)
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

    // editor text accessors (edit mode)
    public var currentEditorText: String {
        config.markdownImages ? editorMarkdown : (editorView?.string ?? editorText)
    }

    // host-supplied: persist a pasted/dropped image next to the note and
    // return its path RELATIVE to the note (nil = ignore the image)
    public var imageSaver: ((NSImage) -> String?)?
    // note directory: resolves attachment rel paths to ABSOLUTE paths for the
    // right-click "copy image path" menu
    public var imageBaseDir: String = ""
    // attachment identity -> relative path, so saves round-trip images back
    // to `![](rel)` markdown instead of dropping them
    public var attachmentPaths: [NSTextAttachment: String] = [:]

    // last attributed content set via setEditorAttributedText, kept so re-shows
    // (presentList) restore colors instead of flattening to plain text
    private var editorAttributed: NSAttributedString?

    public func setEditorText(_ s: String) {
        editorAttributed = nil
        editorText = s
        editorView?.string = s
        restyleEditor()
    }

    // route the editor view's textDidChange into the public hook (programmatic
    // setEditorText calls do NOT post textDidChange, so no feedback loop)
    private func wireEditorTextChange() {
        (editorView as? PopupTextView)?.onTextChange = onEditorTextChange
    }

    // assigning tv.string (or inserting plain strings) resets every run to the
    // DEFAULT typing attributes — black system font — which is what made
    // dictated text render black. Re-apply the editor's font/color everywhere
    // (attachments keep their own run) and fix future typing attributes.
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

    // replace everything from `offset` to the end with `s` (the live draft
    // region). Range-based on purpose: a stale anchor can never wipe the
    // committed text above it (prefix-matching drafts did exactly that).
    public func replaceTail(from offset: Int, with s: String) {
        guard let tv = editorView, let storage = tv.textStorage else { return }
        let loc = max(0, min(offset, storage.length))
        storage.replaceCharacters(
            in: NSRange(location: loc, length: storage.length - loc), with: s)
        editorText = tv.string
        restyleEditor()
        tv.scrollRangeToVisible(NSRange(location: storage.length, length: 0))
    }

    // the editor's caret / selection (UTF-16 offsets into the display string)
    public var editorSelection: NSRange {
        editorView?.selectedRange() ?? NSRange(location: (editorText as NSString).length, length: 0)
    }

    // replace `range` (clamped) with `s`, keep the styling, put the caret
    // after the new text minus `caretBack` characters and keep it visible.
    // Voice dictation's live region at the cursor. Returns the new length.
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

    public func tailText(from offset: Int) -> String {
        guard let tv = editorView else { return "" }
        let s = tv.string as NSString
        let loc = max(0, min(offset, s.length))
        return s.substring(from: loc)
    }

    // MARK: Markdown images

    private func makeAttachment(_ img: NSImage, rel: String) -> NSTextAttachment {
        let att = NSTextAttachment()
        att.image = img
        // fit the photo to the EDITOR width: a wider attachment can never be
        // scrolled to (TextKit keeps the line fragment at the container width,
        // so an oversized image is just clipped, and the text view's frame
        // follows the container — the horizontal scroller has no extent).
        // Scaling keeps the proportions and makes the FULL image visible,
        // like Apple Notes. Smaller images keep their natural size.
        let maxW = editorScroll?.bounds.width ?? config.width
        var size = img.size
        if size.width > maxW {
            size = NSSize(width: maxW, height: size.height * maxW / size.width)
        }
        att.bounds = NSRect(origin: .zero, size: size)
        attachmentPaths[att] = rel
        return att
    }

    // insert a rendered image at the caret (paste / drop path)
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

    // load note markdown: `![alt](rel)` becomes an inline image attachment
    // (scaled to the editor width); missing files fall back to literal text
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

    // MARK: Binary file preview (PDF / image)

    // render a non-editable file (PDF or image) as a READ-ONLY preview in the
    // editor: every PDF page is drawn inline (scaled to the editor width), so
    // the whole document is visible instead of a blank/garble. The host must
    // NOT save the editor text back to such a file (see the note host's save
    // guards). Returns true when the editor switched to a preview.
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
            // RTF is rich text — render it like the finder preview (white
            // background, formatted) instead of dumping raw escape chars
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

    // serialize the editor back to markdown (attachments -> `![](rel)`)
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

    // a pasted photo should be fully reachable: the document view grows to
    // the widest attachment so the horizontal scroller has real extent
    // (NSTextView will NOT widen itself for an attachment that overflows its
    // text container — the line fragment stays container-wide and the image
    // just gets clipped, which is exactly the bug this fixes)
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

    // Attributed variant: sets rich text (colors, bold) on the editor.
    public func setEditorAttributedText(_ s: NSAttributedString) {
        editorAttributed = s
        editorText = s.string
        editorView?.textStorage?.setAttributedString(s)
    }

    // Scroll the editor so its newest text is visible. With ifAtBottom, only
    // follow when the user is already reading near the bottom (live streaming
    // shouldn't yank the scroll position away from a user reading above).
    // scrollRangeToVisible alone can no-op right after a string replacement
    // (layout hasn't caught up), so pin the clip view to the document end.
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

    // ANSI SGR-escaped text (e.g. the doctor's colored PASS/FAIL/WARN output)
    // rendered as an attributed string on the editor's theme + font.
    public func setEditorANSI(_ s: String) {
        let font = editorFont(config.fontName, config.zoom, size: config.editorFontSize)
        setEditorAttributedText(parseANSI(s, baseFont: font, defaultColor: config.colors.text))
    }

    // prettyprint-style syntax highlighting: re-render `text` with JSON/XML
    // token colors using the editor's own font. Fixes the typing attributes so
    // edits after highlighting keep the theme instead of snapping to black.
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

    // MARK: Focus

    private func takeFocus() {
        guard isShown else { return }
        panel.makeKeyAndOrderFront(nil)
        if !panel.isKeyWindow, !NSApp.isActive {
            // NSWindow-based popups (note/list) can't become key while the
            // app is inactive — hiding the switcher deactivated us. The
            // borderless NSPanel (switcher itself) CAN be key when inactive,
            // so we only activate when the key attempt actually failed.
            NSApp.activate(ignoringOtherApps: true)
        }
        // keep focus on whichever pane already owns it (a drawer the user
        // moved to) — only claim it when nothing inside this window has it
        if !paneHoldsFocus() {
            if let ed = primaryEditor {
                panel.makeFirstResponder(ed)
                focusedPane = .editor
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

    // MARK: Event monitors

    private var activeObserver: NSObjectProtocol?
    private var responderObserver: NSObjectProtocol?

    private func installMonitors() {
        // AeroSpace focuses these windows by activating the app + AX-raising
        // the window; activation alone leaves the key window wherever it was,
        // so claim key/first-responder as soon as the app becomes active.
        if activeObserver == nil {
            activeObserver = NotificationCenter.default.addObserver(
                forName: NSApplication.didBecomeActiveNotification, object: nil,
                queue: .main) { [weak self] _ in
                guard let self, self.isShown, !self.isShowingMenu else { return }
                self.panel.makeKeyAndOrderFront(nil)
                // don't steal focus from the embedded terminal: if the shell
                // has it, leave it there (else focus the notes editor)
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
        // (the vim pane is routed inside handleKey: everything but the app's
        // chrome/edit shortcuts passes straight through to the editor)
        if let m = NSEvent.addLocalMonitorForEvents(matching: .keyDown, handler: {
            [weak self] event in
            // local monitors see EVERY key event in the app — only act when
            // THIS window is the key window, so two open popups (notes +
            // jira) never steal each other's shortcuts
            guard let self, self.isShown, self.panel.isKeyWindow
                // a sheet steals the key-window flag from the panel, but its
                // text field still needs our Ctrl+V / Cmd+V routing
                || self.panel.attachedSheet != nil else { return event }
            if self.handleKey(event.keyCode, event.modifierFlags) {
                return nil  // consumed
            }
            return event   // pass through (text input)
        }) {
            monitors.append(m)
        }
        if config.sticky {
            // Sticky windows stay visible when another app takes focus. They
            // are ONLY dismissed by Esc while THIS window is focused (the
            // local monitor above) — a global Esc hook would fire for every
            // app's Escape (e.g. vim's normal-mode Esc) and wrongly close the
            // popup while you're working elsewhere. Click the popup to focus
            // it, then Esc, to dismiss.
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
        // Track first-responder changes so the focus border follows the
        // user's mouse clicks between panes (editor / browser / terminal).
        responderObserver = NotificationCenter.default.addObserver(
            forName: NSWindow.didBecomeKeyNotification, object: panel,
            queue: .main) { [weak self] _ in
            self?.updateFocusedPane()
        }
        // Also track mouse clicks within the window — didBecomeKey only fires
        // when the window becomes key, not when clicking between panes inside
        // an already-key window.
        if let m = NSEvent.addLocalMonitorForEvents(matching: .leftMouseDown, handler: {
            [weak self] event in
            guard let self, self.isShown, self.panel.isKeyWindow else { return event }
            // short delay so first responder has updated
            DispatchQueue.main.async { self.updateFocusedPane() }
            return event
        }) {
            monitors.append(m)
        }
    }

    // Re-evaluate which pane currently has first responder and update the
    // bright focus border accordingly. Called on key-window activation and
    // after Ctrl+J/K cycles.
    private func updateFocusedPane() {
        let fr = panel.firstResponder
        // Evaluate every pane independently — the browser branch must not
        // gate the terminal check. The notes window opens with the file
        // browser drawer (start-drawer = browser), so the old else-if chain
        // entered the browser branch on EVERY click and never reached the
        // terminal branch: clicking the shell drawer focused the shell (keys
        // worked) but the focus border stayed on whatever pane was last.
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

    // MARK: Keys

    // the NSTextView / NSTextField field editor actively editing inside the
    // sheet / panel — an NSTextField's first responder is the FIELD itself,
    // so its editing shortcut routing must go through currentEditor()
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

    private func handleKey(_ code: UInt16, _ mods: NSEvent.ModifierFlags) -> Bool {
        // an open action picker owns the keyboard (its Esc never counts
        // toward the window's Esc-streak close)
        if actionPicker != nil { return actionPickerKey(code, mods) }
        // an Esc streak (N rapid Esc close the window) only counts
        // CONSECUTIVE presses — any other key starts it over
        if code != 53 { escStreak = 0 }
        // Cmd + plus/minus (main "="/"+" and "-", plus the keypad): grow or
        // shrink the window; rows stretch to fill from then on
        let cmd = mods.contains(.command)
        // Cmd+Opt+= / Cmd+Opt+- : editor + terminal font size step
        if cmd, mods.contains(.option), panel.attachedSheet == nil,
           let step = onFontSizeStep {
            switch code {
            case 24, 69: step(1); return true
            case 27, 78: step(-1); return true
            default: break
            }
        }
        // resize shortcuts never fire while a sheet's text field is up —
        // Cmd+= / Cmd+- are typing/editing context, not window chrome
        if cmd, panel.attachedSheet == nil {
            switch code {
            case 24: resizeBy(80); return true          // = / + (Cmd+Shift+=)
            case 27: resizeBy(-80); return true         // -
            case 69: resizeBy(80); return true          // keypad +
            case 78: resizeBy(-80); return true         // keypad -
            default: break
            }
        }
        // Editing shortcuts (select-all / copy / paste / cut / undo) must be
        // intercepted explicitly: system key equivalents don't fire reliably
        // for nonactivating accessory-app windows (Cmd/Ctrl + A/C/V/X/Z).
        // Terminal policy: when the shell holds focus it owns EVERY shortcut
        // except copy (Cmd+C) and paste (Cmd+V / Ctrl+V) — Ctrl+C must reach
        // the shell as SIGINT, Ctrl+A/Z/X stay readline/suspend, etc.
        let ctrl = mods.contains(.control)
        if cmd || ctrl {
            // When a sheet is up (e.g. the New Note / Open Existing dialog),
            // its own text field must own the edit shortcuts — don't hijack
            // Cmd+V/C/X/A into the editor behind the sheet. Paste (Cmd+V AND
            // Ctrl+V, the Linux-style shortcut the terminal also honors) is
            // routed straight to the sheet's field editor so it ALWAYS works,
            // even though the app has no Edit menu / key equivalents.
            if panel.attachedSheet != nil {
                // The sheet's text field owns the standard edit shortcuts —
                // routed straight to its field editor so they ALWAYS work,
                // even though the app has no Edit menu / key equivalents.
                // (Cmd+V AND Ctrl+V paste; Cmd+A/C/X/Z select/copy/cut/undo.)
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
            // Ctrl+J / Ctrl+K: move keyboard focus up/down across the panes
            // (notes editor -> file browser drawer -> terminal drawer), only
            // over the ones that are open. J = down, K = up. Handled BEFORE
            // the terminal branch so it works from any pane.
            // Ctrl+Shift+J/K is reserved for pane resize — don't intercept it.
            if ctrl && !mods.contains(.shift) && (code == 38 || code == 40), config.editMode {
                var panes: [NSResponder] = []
                var paneTypes: [FocusedPane] = []
                if let ed = primaryEditor { panes.append(ed); paneTypes.append(.editor) }
                if fileBrowserShown, let fb = fileBrowser { panes.append(fb.listView); paneTypes.append(.browser) }
                if terminalShown, let term = terminalDrawer { panes.append(term); paneTypes.append(.terminal) }
                if panes.count > 1 {
                    let current = panel.firstResponder
                    var curIdx = panes.firstIndex { p in
                        if let current, current === p { return true }
                        if let v = current as? NSView, let pv = p as? NSView {
                            return v.isDescendant(of: pv)
                        }
                        return false
                    }
                    // the filter bar (its field editor), pills and preview
                    // sit OUTSIDE the list view but are still the browser
                    // pane — without this, Ctrl+J/K from the filter bar saw
                    // "no pane focused" and jumped to the first/last pane
                    // instead of the neighbour, needing extra presses
                    var inFilterBar = false
                    if curIdx == nil, fileBrowserShown, let fb = fileBrowser,
                       browserHasFocus(fb), let bi = paneTypes.firstIndex(of: .browser) {
                        curIdx = bi
                        inFilterBar = fb.searchView.currentEditor() != nil
                    }
                    if let curIdx {
                        let target = code == 38
                            ? min(curIdx + 1, panes.count - 1)
                            : max(curIdx - 1, 0)
                        // at the edge from the filter bar: drop into the list
                        // (the browser is still the pane, focus still moves)
                        if target != curIdx || inFilterBar { panel.makeFirstResponder(panes[target]) }
                        focusedPane = paneTypes[target]
                    } else {
                        let target = code == 38 ? panes[0] : panes[panes.count - 1]
                        panel.makeFirstResponder(target)
                        focusedPane = code == 38 ? paneTypes[0] : paneTypes[panes.count - 1]
                    }
                    updateFocusIndicator()
                    return true
                }
                // single pane (editor only) — fall through so the emacs
                // bindings (Ctrl+J newline, Ctrl+K kill-line) reach the text view
            }
            // Ctrl+Tab / Ctrl+Shift+Tab: next / previous tab (open notes),
            // wrapping at either end. Before the vim/terminal branches so it
            // works from every pane.
            if ctrl && !cmd && code == 48, config.tabs, tabTitles.count > 1 {
                let n = tabTitles.count
                selectedTab = (selectedTab + (mods.contains(.shift) ? -1 : 1) + n) % n
                return true
            }
            // Ctrl+Shift+HJKL: resize window like tmux pane resize
            // H = shrink width, L = grow width, J = shrink height, K = grow height
            if ctrl && mods.contains(.shift), panel.attachedSheet == nil {
                let step: CGFloat = 20
                switch code {
                case 4:  // H — shrink width
                    var f = panel.frame
                    f.size.width = max(120, f.width - step)
                    panel.setFrame(clampToScreen(f), display: true)
                    return true
                case 37: // L — grow width
                    var f = panel.frame
                    f.size.width = min(maxPanelWidth(), f.width + step)
                    panel.setFrame(clampToScreen(f), display: true)
                    return true
                case 40: // K — grow focused pane height
                    if terminalShown && focusedPane == .terminal {
                        currentTerminalHeight = min(600, currentTerminalHeight + step)
                        preferredTerminalHeight = currentTerminalHeight
                    } else if fileBrowserShown && focusedPane == .browser {
                        currentBrowserHeight = min(600, currentBrowserHeight + step)
                        preferredBrowserHeight = currentBrowserHeight
                    } else {
                        // editor: grow window height
                        var f = panel.frame
                        f.size.height = min(maxPanelHeight(), f.height + step)
                        panel.setFrame(clampToScreen(f), display: true)
                        return true
                    }
                    syncDrawerLayout()
                    updateFocusIndicator()
                    return true
                case 38: // J — shrink focused pane height
                    if terminalShown && focusedPane == .terminal {
                        currentTerminalHeight = max(minTerminalH, currentTerminalHeight - step)
                        preferredTerminalHeight = currentTerminalHeight
                    } else if fileBrowserShown && focusedPane == .browser {
                        currentBrowserHeight = max(minBrowserH, currentBrowserHeight - step)
                        preferredBrowserHeight = currentBrowserHeight
                    } else {
                        // editor: shrink window height
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
            // vim pane: every Cmd/Ctrl key belongs to the editor except the
            // app's edit shortcuts (rule 1), mapped onto vim actions.
            // Ctrl+C / Cmd+C only copy while a Visual selection exists —
            // otherwise Ctrl+C stays vim's own (cancel) key.
            if let vv = focusedVim() {
                switch code {
                case 8 where cmd || ctrl:           // C — copy
                    if vimCopySelection(cut: false) { return true }
                    if cmd, vv.selectedRange().length > 0 { vv.copy(self); return true }
                    return cmd
                case 9 where cmd || ctrl:           // V — paste
                    vimPaste(); return true
                case 7 where cmd:                   // X — cut the selection
                    _ = vimCopySelection(cut: true); return true
                case 0 where cmd:                   // A — select all
                    vimRemote("<C-\\><C-N>ggVG"); return true
                case 6 where cmd:                   // Z — undo
                    vimRemote("<C-\\><C-N>u"); return true
                case 1 where cmd:                   // S — save
                    vimCommand("silent! wall")
                    onEditorCommit?(currentEditorText)
                    return true
                case 3 where cmd:                   // F — vim search
                    vimRemote("<C-\\><C-N>/"); return true
                case 13 where cmd:                  // W — close the window
                    handleEscape(); return true
                case 31 where cmd:                  // O — open file at path
                    onOpenPathPrompt?(); return true
                default:
                    return false                    // Ctrl+* etc. -> vim
                }
            }
            if let term = focusedTerm() {
                switch code {
                case 8 where cmd: term.copy(self); return true    // Cmd+C copy
                case 9 where cmd || ctrl: term.paste(self); return true  // Cmd+V / Ctrl+V paste
                default: return false   // every other Cmd/Ctrl key goes to the shell
                }
            }
            // Cmd+K: the host's action picker (the file browser keeps its
            // own Cmd+K = copy path while it has focus)
            if cmd && code == 40, let hook = onCommandK,
               !(fileBrowser.map { browserActive() && browserHasFocus($0) } ?? false) {
                hook()
                return true
            }
            if cmd && code == 3, !config.editMode, let hook = onCommandF {
                hook()
                return true
            }
            // Cmd+L: focus the browser's filter bar (address-bar shortcut),
            // selecting the current path so typing replaces it
            if cmd && code == 37, let fb = fileBrowser, browserActive() {
                panel.makeFirstResponder(fb.searchView)
                fb.searchView.currentEditor()?.selectAll(nil)
                return true
            }
            // The file browser's search field owns the standard editing
            // shortcuts while it (or its list) has focus — otherwise Cmd+V
            // pastes into the notes editor / hidden window field instead of
            // the filter bar.
            if let fb = fileBrowser, browserActive(), browserHasFocus(fb) {
                switch code {
                case 45 where ctrl, 35 where ctrl:   // Ctrl+N / Ctrl+P — next / prev result
                    fb.listView.moveSelection(code == 45 ? 1 : -1)
                    return true
                case 40 where cmd:  // Cmd+K — copy the selected row's absolute path
                    if let p = fb.copyRowPath(fb.listView.selection), !config.copyToast.isEmpty {
                        let shown = (p as NSString).abbreviatingWithTildeInPath
                        showToast(config.copyToast.replacingOccurrences(of: "{}", with: shown),
                                  symbol: "doc.on.clipboard")
                    }
                    return true
                case 0:   // A — select all in the filter bar
                    fb.searchView.selectText(nil)
                    return true
                case 8:   // C — copy the search selection / the list row path
                    if let ed = fb.searchView.currentEditor() {
                        ed.copy(nil)
                    } else {
                        fb.copyRowPath(fb.listView.selection)
                    }
                    return true
                case 9:   // V — paste into the filter bar
                    if let ed = fb.searchView.currentEditor() {
                        ed.paste(nil)
                    } else if panel.makeFirstResponder(fb.searchView),
                              let ed = fb.searchView.currentEditor() {
                        ed.paste(nil)
                    }
                    return true
                case 7:   // X — cut from the filter bar
                    fb.searchView.currentEditor()?.cut(nil)
                    return true
                case 6:   // Z — undo in the filter bar
                    fb.searchView.currentEditor()?.undoManager?.undo()
                    return true
                default:
                    break
                }
            }
            switch code {
            case 0:   // A — select all
                // find bar first if it's visible and focused
                if let ff = findField, !ff.isHidden, panel.firstResponder === ff || ff.currentEditor() != nil {
                    ff.selectText(nil)
                    ff.currentEditor()?.selectAll(nil)
                } else if let tv = editorView {
                    tv.selectAll(nil)
                } else {
                    field.selectText(nil)
                }
                return true
            case 8:   // C — copy
                if let ff = findField, !ff.isHidden, let ed = ff.currentEditor() {
                    ed.copy(nil)
                } else if let tv = editorView {
                    tv.copy(nil)
                } else if let ed = field.currentEditor() {
                    ed.copy(nil)
                }
                return true
            case 9:   // V — paste
                if let ff = findField, !ff.isHidden, let ed = ff.currentEditor() {
                    ed.paste(nil)
                } else if let tv = editorView {
                    tv.paste(nil)
                } else if let ed = field.currentEditor() {
                    ed.paste(nil)
                }
                return true
            case 7:   // X — cut
                if let ff = findField, !ff.isHidden, let ed = ff.currentEditor() {
                    ed.cut(nil)
                } else if let tv = editorView {
                    tv.cut(nil)
                } else if let ed = field.currentEditor() {
                    ed.cut(nil)
                }
                return true
            case 6:   // Z — undo
                if let ff = findField, !ff.isHidden, let ed = ff.currentEditor() {
                    ed.undoManager?.undo()
                } else if let tv = editorView {
                    tv.undoManager?.undo()
                } else if let ed = field.currentEditor() {
                    ed.undoManager?.undo()
                }
                return true
            case 3:   // F — find in the note (Cmd+F or Ctrl+F)
                if config.editMode {
                    toggleFindBar()
                    return true
                }
                return false
            default:
                break
            }
        }
        if config.editMode {
            // text editor: only Esc (dismiss; host saves on close) and Cmd+S
            // (explicit save) are consumed — everything else goes to the text
            // view (typing, arrows, etc.)
            if let term = terminalDrawer, terminalShown, terminalFocused(term) {
                // the embedded terminal has keyboard focus: let SwiftTerm see
                // EVERYTHING (including Esc — the shell's, not the window's)
                // except the Nth rapid Esc, which closes the window
                if code == 53, escStreakCloses() {
                    handleEscape()
                    return true
                }
                return false
            }
            if let vv = focusedVim() {
                // the vim pane owns Esc (normal mode) and every plain key.
                // Esc is written to the pty directly: a stray modifier flag
                // (e.g. .function left over from an arrow key event) makes
                // the terminal view drop it, stranding vim in Insert mode.
                if code == 53, mods.intersection([.command, .control, .option]).isEmpty {
                    // the Nth rapid Esc closes the window — but only when vim
                    // is ALREADY in Normal mode (the earlier presses got it
                    // there), so leaving Insert/Visual never closes anything
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
                // find bar owns Esc (close) and Return/Shift+Return (cycle)
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
            // Cmd+O: open a file at an exact path (same prompt as the editor's
            // "Open file at path…" context menu)
            if code == 31, mods.contains(.command) {
                onOpenPathPrompt?()
                return true
            }
            return false
        }
        if config.selectableRows, mods.contains(.control), code == 49 {
            toggleRowSelection(selection)   // Ctrl+Space ticks the current row
            return true
        }
        if config.enableNavigation {
            let ctrl = mods.contains(.control)
            switch (code, ctrl) {
            case (125, _): moveSelection(1); return true                          // Down
            case (126, _): moveSelection(-1); return true                         // Up
            case (48, _): moveSelection(mods.contains(.shift) ? -1 : 1); return true  // Tab
            case (45, true): moveSelection(1); return true                        // C-n
            case (35, true): moveSelection(-1); return true                       // C-p
            case (36, _), (38, true): acceptSelection(); return true              // Return / C-j
            default: break
            }
        }
        if config.enableEscape && code == 53 {
            if escStreakCloses() { handleEscape() }
            return true
        }
        return false
    }

    // MARK: Action picker (Cmd+K)

    // A small centered list of actions over the window: Up/Down, Ctrl+N/P
    // or Tab move, Return / click picks, Esc closes just the picker.
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
            // a digit picks directly (1…9)
            if let n = [18: 1, 19: 2, 20: 3, 21: 4, 23: 5, 22: 6, 26: 7, 28: 8, 25: 9][Int(code)],
               n <= actionItems.count {
                let pick = actionPick
                closeActionPicker()
                pick?(n - 1)
            }
            return true   // everything else is swallowed while the picker is up
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

    // MARK: Toast

    // Raycast-style confirmation pill: pops in at the bottom-center of the
    // window (fade + small rise/scale), holds, then fades. A new toast
    // replaces the one on screen.
    private weak var toastView: NSView?
    func showToast(_ text: String, symbol: String? = nil) {
        guard let root = panel.contentView else { return }
        toastView?.removeFromSuperview()
        let c = config.colors
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
        label.font = .systemFont(ofSize: 12.5 * zoom, weight: .medium)
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
        // explicit frames (no stack view): symmetric side padding and the
        // icon + text group centered on both axes of the pill
        label.sizeToFit()
        let padX = 16 * zoom, gap = 8 * zoom
        let iconSize = icon?.fittingSize ?? .zero
        let groupExtra = icon == nil ? 0 : iconSize.width + gap
        let h = 32 * zoom
        let w = min(ceil(label.frame.width + groupExtra + padX * 2), root.bounds.width - 32)
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
        // bottom-center, clear of the footer strip; the backdrop is flipped
        let inset = 30 * zoom
        let flipped = root.isFlipped
        pill.frame = NSRect(x: (root.bounds.width - w) / 2,
                            y: flipped ? root.bounds.height - h - inset : inset,
                            width: w, height: h)
        pill.autoresizingMask = [.minXMargin, .maxXMargin, flipped ? .minYMargin : .maxYMargin]
        pill.layer?.cornerRadius = h / 2
        root.addSubview(pill, positioned: .above, relativeTo: nil)
        toastView = pill

        // pop in: rise 6pt + fade, then hold and fade out
        let final = pill.frame
        pill.alphaValue = 0
        pill.setFrameOrigin(NSPoint(x: final.minX, y: final.minY + (flipped ? 6 : -6) * zoom))
        NSAnimationContext.runAnimationGroup { ctx in
            ctx.duration = 0.18
            ctx.timingFunction = CAMediaTimingFunction(name: .easeOut)
            pill.animator().alphaValue = 1
            pill.animator().setFrameOrigin(final.origin)
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.4) { [weak pill] in
            guard let pill, pill.superview != nil else { return }
            NSAnimationContext.runAnimationGroup({ ctx in
                ctx.duration = 0.25
                ctx.timingFunction = CAMediaTimingFunction(name: .easeIn)
                pill.animator().alphaValue = 0
            }, completionHandler: { pill.removeFromSuperview() })
        }
    }

    // counts one Esc press toward config.escCloseCount; true (and the streak
    // resets) on the Nth press within 0.6 s of the previous one
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

    // Cmd+±: resize the window by a fixed delta; rows stretch to fill the new
    // space (scrollable lists expand their document to the visible height).
    // The frame is clamped to the screen so growing in place can never push
    // the bottom (scrollbar + last rows) off-screen.
    private func resizeBy(_ delta: CGFloat) {
        rowView.stretchToFill = true
        var f = panel.frame
        f.size.width = max(120, f.width + delta)
        f.size.height = max(140, f.height + delta)
        panel.setFrame(clampToScreen(f), display: true)
        // scale the UI proportionally with the resize: zoom tracks the
        // width relative to the config's base width
        zoom = min(3, max(0.6, f.width / config.width))
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

    // MARK: NSTextFieldDelegate

    public func controlTextDidBeginEditing(_ obj: Notification) {
        // the shared field editor draws its own system focus ring; the search
        // bar carries a themed hairline border instead
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

    // MARK: NSWindowDelegate

    public func windowDidResignKey(_ notification: Notification) {
        // hide the focus border when another app takes focus so it's obvious
        // this window no longer owns the keyboard
        editorFocusBorder?.isHidden = true
        browserFocusBorder?.isHidden = true
        terminalFocusBorder?.isHidden = true
        // sticky windows stay visible when another app takes focus (the user
        // dismisses them with Esc); everything else hides on focus loss —
        // unless the global hide-on-focus-loss setting is disabled, in which
        // case no window hides on focus loss (only Esc dismisses)
        // Checked on the next turn, once the new key window is known: our own
        // sheets / alerts / color panel / open panel / header menus take key
        // without the user leaving the window, so they never count as a loss.
        guard isShown && !config.sticky && settings.hideOnFocusLoss else { return }
        DispatchQueue.main.async { [weak self] in
            guard let self, self.isShown, !self.config.sticky, settings.hideOnFocusLoss,
                  !self.isShowingMenu, !self.panel.isKeyWindow,
                  self.panel.attachedSheet == nil else { return }
            if let k = NSApp.keyWindow, k is NSPanel, !(k.delegate is PopupWindow) { return }
            self.hide(restore: false)
        }
    }

    // User resized the window (e.g. dragged a corner): from now on rows fill
    // the current space, so remember the row count this size was chosen for.
    // Also clamp the frame to the screen so drag-resizing can never push the
    // bottom (scrollbar + last pill) off-screen.
    // Smart resize: when shrinking, terminal/browser drawers shrink first
    // (down to a minimum) before the editor is touched. When growing, the
    // editor expands while drawers stay at their configured height.
    public func windowDidResize(_ notification: Notification) {
        // programmatic resizes may overshoot the screen — pull them back.
        // NEVER during a live (mouse) resize: re-framing mid-drag fights the
        // window server and slides the window instead of moving the edge.
        if isShown, !panel.inLiveResize {
            let clamped = clampToScreen(panel.frame)
            if clamped != panel.frame {
                panel.setFrame(clamped, display: true)
                return   // re-enters with the clamped frame
            }
        }
        fitDrawersToWindow()
        drawerInsetNow = (terminalShown ? currentTerminalHeight : 0) + (fileBrowserShown ? currentBrowserHeight : 0)
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

    // User finished dragging a resize edge (or Cmd+±): from now on rows fill
    // the window.
    // Drawer heights for the CURRENT window height: preferred heights when
    // they fit; when the editor would drop below minEditorH, the terminal and
    // then the browser give up space (down to their minimums).
    private func fitDrawersToWindow() {
        guard config.editMode, let backdrop = panel.contentView else { return }
        let tabH = tabsBar?.frame.height ?? config.tabBarHeight * zoom
        let topY = config.headerHeight * zoom + tabH + 2 + findBarHeight()
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

    // scrollable rows: document height = content, or the visible space when
    // the user resized the window (rows then stretch to fill it); document
    // WIDTH always tracks the window so rows widen after a resize. Bottom
    // inset is ALWAYS reserved so the last pill can be fully visible.
    private func scrollDocumentHeight() -> CGFloat {
        let content = rowView.contentHeight()
        guard rowView.stretchToFill, let scroll = rowScroll else { return content }
        return max(content, scroll.bounds.height) + rowView.bottomInset
    }

    private func layoutScrollDocument() {
        guard config.scrollableRows, let scroll = rowScroll else { return }
        let w = scroll.bounds.width > 0 ? scroll.bounds.width : config.width
        // re-pin the scroll view: top edge at the chrome, bottom edge at the
        // window's bottom. Autoresizing alone overshoots the window bottom
        // (the scrollbar + last rows would hang off-screen).
        if let backdrop = panel.contentView {
            let cb = chromeBottom > 0 ? chromeBottom : 0
            let h = max(40, backdrop.bounds.height - cb)
            if scroll.frame.origin.y != cb || abs(scroll.frame.height - h) > 0.5 {
                scroll.frame = NSRect(x: 0, y: cb, width: backdrop.bounds.width, height: h)
            }
        }
        // apply the WIDTH first: contentHeight measures at the live width, so
        // a stale width would make the document too short and clip the last
        // row's pill after a resize
        rowView.frame.size.width = w
        if let header = tableHeader {
            header.zoom = zoom
            header.frame = NSRect(x: 0, y: 0, width: w, height: config.tableHeaderHeight * zoom)
            rowView.topInset = config.tableHeaderHeight * zoom + 2
            header.needsDisplay = true
        }
        rowView.frame.size.height = scrollDocumentHeight()
    }

    // new column widths/titles for a table-mode list (live divider drags,
    // config reloads): header + rows re-measure and redraw together
    // "Fit columns": size every column to its widest cell (header title
    // included, sampled over the first rows) and widen the window so the
    // whole table fits — capped at the visible screen, where the widest
    // columns give up room first. Returns the new percent widths (the host
    // persists them) or nil when nothing could be measured.
    @discardableResult
    public func fitTableColumns(sample: Int = 400) -> [CGFloat]? {
        let cols = config.tableColumns
        guard !cols.isEmpty else { return nil }
        let font = config.rowFont(config.rowFontSize * zoom)
        let bold = NSFontManager.shared.convert(font, toHaveTrait: .boldFontMask)
        let hfont = NSFontManager.shared.convert(config.rowFont(config.rowFontSize * zoom * 0.86),
                                                 toHaveTrait: .boldFontMask)
        let rowsToMeasure = rows.filter { !$0.loadMore }.prefix(sample)
        var need: [CGFloat] = cols.enumerated().map { i, col in
            // header: small caps + kerning + sort arrow + the ▾ slot
            var w = (col.title.uppercased() + " ↑" as NSString)
                .size(withAttributes: [.font: hfont, .kern: 0.6]).width + 12
            if col.filterable { w += 22 }
            for r in rowsToMeasure {
                guard let t = r.cellText(col.field), !t.isEmpty else { continue }
                let flat = t.replacingOccurrences(of: "\n", with: " ")
                let f = i == 0 ? bold : font
                // + the status dot some cells draw
                w = max(w, (flat as NSString).size(withAttributes: [.font: f]).width + 11 + 11 * zoom)
            }
            return ceil(w)
        }
        // one runaway cell (a huge title) must not make a 4000pt window
        let screenW = (panel.screen ?? NSScreen.main)?.visibleFrame.width ?? 1400
        let lead = config.rowLeadInset, trail = config.padding + 10 + 16   // + scroller
        let cap = screenW - 40 - lead - trail
        need = need.map { min($0, max(160, cap * 0.45)) }
        var total = need.reduce(0, +)
        if total > cap {
            // trim the widest columns down toward each other until it fits
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

    // Tab bar height is dynamic: many tabs WRAP to extra rows instead of
    // hiding. After titles change (or the window resizes), re-measure and
    // shift the content below so it never overlaps the wrapped pills.
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
                // the strip's top edge, from its height BEFORE this change
                let base = self.chromeBottom - (oldH + 2)
                let cb = base + h + 2
                self.chromeBottom = cb
                scroll.frame = NSRect(x: 0, y: cb, width: backdrop.bounds.width,
                                      height: max(40, backdrop.bounds.height - cb))
                layoutScrollDocument()
            }
        }
    }

    // Pin the note editor's scroll view below the drag header + (wrapped) tab
    // strip — autoresizing alone overshoots the window bottom.
    // Embedded terminal drawer: grows/shrinks with it, and the editor stops
    // above it while shown.
    public func toggleTerminalDrawer() {
        guard let drawer = terminalDrawer else { return }
        // manual recreate: if the drawer is coming back up with a dead shell
        // (exit/ctrl-d left it hung), respawn it so the user never gets stuck
        if !terminalShown, let p = drawer.process, !p.running, !p.windingDown {
            drawer.startProcess(executable: config.shell, args: config.shellArgs)
        }
        terminalShown.toggle()
        if terminalShown { currentTerminalHeight = preferredTerminalHeight }
        syncDrawerLayout()
        if terminalShown {
            panel.makeFirstResponder(drawer)
            focusedPane = .terminal
        } else if let ed = primaryEditor {
            panel.makeFirstResponder(ed)
            focusedPane = .editor
        }
        updateFocusIndicator()
    }

    // MARK: Vim pane

    // the pane that owns "editing" right now: the vim terminal while it is
    // active, else the native text view
    private var primaryEditor: NSView? {
        if let vv = vimView, vimPaneActive { return vv }
        return editorView
    }

    // the vim pane, but only while it is active AND holds keyboard focus
    private func focusedVim() -> LocalProcessTerminalView? {
        guard let vv = vimView, vimPaneActive else { return nil }
        let fr = panel.firstResponder
        if fr === vv { return vv }
        if let v = fr as? NSView, v.isDescendant(of: vv) { return vv }
        return nil
    }

    // does a real pane (editor / vim / browser / terminal / find bar) hold
    // focus? takeFocus leaves such a choice alone instead of stealing it
    private func paneHoldsFocus() -> Bool {
        guard let v = panel.firstResponder as? NSView else { return false }
        if let vv = vimView, vimPaneActive, v === vv || v.isDescendant(of: vv) { return true }
        if let ed = editorView, !(editorScroll?.isHidden ?? true),
           v === ed || v.isDescendant(of: ed) { return true }
        if let term = terminalDrawer, terminalShown, v === term || v.isDescendant(of: term) { return true }
        if let fb = fileBrowser, fileBrowserShown, v.isDescendant(of: fb) { return true }
        if let ff = findField, !ff.isHidden, v === ff || ff.currentEditor() === v { return true }
        return false
    }

    // monospace font for the vim pane: the window font when it is fixed
    // pitch (a proportional font would garble the terminal grid), else the
    // terminal font, else the system mono
    static func vimFont(_ c: PopupConfig) -> NSFont {
        let size = c.editorFontSize * c.zoom
        if let n = c.fontName, let f = NSFont(name: n, size: size), f.isFixedPitch { return f }
        if let f = NSFont(name: c.terminalFont, size: size) { return f }
        return NSFont.monospacedSystemFont(ofSize: size, weight: .regular)
    }

    // Environment for the editor process. The terminal's default env has no
    // PATH, so nvim could not find pbcopy/pbpaste — every yank/delete with
    // clipboard=unnamedplus raised a blocking "Press ENTER" error. Inherit
    // the app's env, guarantee the system + Homebrew dirs, and advertise a
    // truecolor UTF-8 terminal.
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
        // never inherit a parent nvim's server address (would redirect RPC)
        env.removeValue(forKey: "NVIM")
        env.removeValue(forKey: "NVIM_LISTEN_ADDRESS")
        return env.map { "\($0.key)=\($0.value)" }
    }

    // whether this window edits through the embedded vim pane
    public var isVimEditor: Bool { vimView != nil }

    // PID-free liveness check of the editor process
    public var vimRunning: Bool { vimView?.process?.running ?? false }

    // Show the vim pane (text notes) or the native read-only preview
    // (PDF/image tabs, which a terminal editor can't display).
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

    // Start the editor unless it is already running. Stale sockets from a
    // previous (crashed) editor are removed first so --listen can bind.
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
            // cell height lets the editor reserve just enough rows per image
            let ch = Int(PopupWindow.cellSize(PopupWindow.vimFont(config)).height)
            args.insert(contentsOf: ["--cmd", "let g:ws_cell_h=\(ch)"], at: 0)
        }
        vv.startProcess(executable: exec, args: args,
                        environment: PopupWindow.vimEnvironment(),
                        currentDirectory: config.terminalDir)
    }

    // Stop the editor for good (window teardown): save every buffer, quit,
    // and never relaunch.
    public func shutdownVim() {
        guard vimView != nil else { return }
        vimShuttingDown = true
        if vimEval("execute('silent! wall')") == nil {
            vimRemote("<C-\\><C-N>:silent! wall<CR>")
        }
        vimRemote("<C-\\><C-N>:qa!<CR>")
    }

    // Run `nvim --server <socket> <flag> <arg>` (short timeout). Returns the
    // client's stdout, or nil when the socket/editor is unavailable.
    @discardableResult
    private func vimClient(_ flag: String, _ arg: String) -> String? {
        guard let exec = config.vimEditorExecutable,
              let sock = config.vimEditorSocket,
              FileManager.default.fileExists(atPath: sock),
              vimRunning else { return nil }
        let p = Process()
        p.executableURL = URL(fileURLWithPath: exec)
        p.arguments = ["--headless", "--clean", "--server", sock, flag, arg]
        let out = Pipe()
        p.standardOutput = out
        p.standardError = FileHandle.nullDevice
        p.standardInput = FileHandle.nullDevice
        do { try p.run() } catch { return nil }
        let deadline = Date().addingTimeInterval(1.5)
        while p.isRunning && Date() < deadline { usleep(5_000) }
        if p.isRunning { p.terminate(); return nil }
        let data = out.fileHandleForReading.readDataToEndOfFile()
        guard p.terminationStatus == 0 else { return nil }
        return String(data: data, encoding: .utf8) ?? ""
    }

    // Send keys to the editor as if typed (vim key notation: <CR>, <Esc>…).
    // Falls back to writing raw keystrokes to the terminal when no RPC
    // socket is available (plain vim).
    public func vimRemote(_ keys: String) {
        if vimClient("--remote-send", keys) != nil { return }
        guard let vv = vimView, vimRunning else { return }
        let raw = keys
            .replacingOccurrences(of: "<C-\\><C-N>", with: "\u{1c}\u{0e}")
            .replacingOccurrences(of: "<CR>", with: "\r")
            .replacingOccurrences(of: "<Esc>", with: "\u{1b}")
        vv.send(txt: raw)
    }

    // Evaluate a Vimscript expression in the editor (nvim only). Returns the
    // result as text, or nil when it could not be evaluated.
    public func vimEval(_ expr: String) -> String? {
        vimClient("--remote-expr", expr)
    }

    // Run an Ex command immediately (works in any mode, no keystrokes).
    // Falls back to typed keys when RPC is unavailable.
    public func vimCommand(_ ex: String) {
        let quoted = "'" + ex.replacingOccurrences(of: "'", with: "''") + "'"
        if vimEval("execute(\(quoted))") != nil { return }
        // typed fallback: `echo ''` wipes the echoed command line afterwards
        // (it used to linger as ":silent! checktime" under the note)
        vimRemote("<C-\\><C-N>:\(ex) | echo ''<CR>")
    }

    // Vim single-quoted string literal for arbitrary text
    public static func vimString(_ s: String) -> String {
        "'" + s.replacingOccurrences(of: "'", with: "''") + "'"
    }

    // Save the editor's buffers to disk (tab switch / hide / host writes).
    public func vimFlush() {
        vimCommand("silent! wall")
    }

    // Switch the editor to `path` (saving first). Leaves insert mode so the
    // new note opens in Normal mode like a fresh editor.
    public func vimOpen(_ path: String) {
        let lit = PopupWindow.vimString(path)
        // redraw! clears the previous buffer's leftover message line
        let ex = "silent! wall | stopinsert | execute 'edit ' .. fnameescape(\(lit)) | redraw!"
        if vimEval("execute(\(PopupWindow.vimString(ex)))") != nil { return }
        // no RPC: typed fallback (path escaped for the command line)
        let esc = path.replacingOccurrences(of: " ", with: "\\ ")
        vimRemote("<C-\\><C-N>:silent! wall | edit \(esc)<CR>")
    }

    // Append lines to the end of `path`'s buffer and save (voice dictation):
    // edits happen IN the editor, so nothing races the user's typing.
    @discardableResult
    public func vimAppend(_ text: String, to path: String) -> Bool {
        let lines = text.components(separatedBy: "\n").map { PopupWindow.vimString($0) }
        let list = "[" + lines.joined(separator: ",") + "]"
        let buf = "bufnr(\(PopupWindow.vimString(path)))"
        let expr = "\(buf) > 0 ? [appendbufline(\(buf), '$', \(list)), execute('silent! wall')][0] : -1"
        guard let r = vimEval(expr) else { return false }
        return r.trimmingCharacters(in: .whitespacesAndNewlines) == "0"
    }

    // Voice dictation in the vim pane: a live region AT THE CURSOR, tracked
    // by two extmarks (left / right gravity) so typing elsewhere never
    // shifts it. Begin anchors it (Normal mode: after the character under
    // the cursor, like `a`; Insert mode: at the caret) and remembers
    // whether a space is needed before / after; update replaces the
    // region's text and moves the cursor to its end; end drops the marks
    // and saves. All false when the editor has no RPC socket (plain vim).
    private static func vimLua(_ lines: [String]) -> String {
        lines.map { $0.trimmingCharacters(in: .whitespaces) }.joined(separator: " ")
    }
    // Vim double-quoted string literal (newlines survive as \n)
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

    // Visual-mode copy/cut into the system clipboard. Returns false when no
    // Visual selection exists (nothing to copy).
    private func vimCopySelection(cut: Bool) -> Bool {
        guard let m = vimEval("mode()")?.trimmingCharacters(in: .whitespacesAndNewlines),
              ["v", "V", "\u{16}"].contains(m) else { return false }
        vimRemote(cut ? "\"+d" : "\"+y")
        return true
    }

    // right-click menu for the vim pane (the host builds it: it knows the
    // current note path for Copy File Path / Reveal in Finder)
    public var vimMenu: NSMenu? {
        get { vimView?.menu }
        set { vimView?.menu = newValue }
    }

    // right-click Copy: the Visual selection, else the terminal selection
    public func vimCopy() {
        if vimCopySelection(cut: false) { return }
        if let vv = vimView, vv.selectedRange().length > 0 { vv.copy(self) }
    }

    // Paste the clipboard into the editor through nvim's own paste API
    // (mode-correct: inserts in Insert mode, puts in Normal, types into the
    // command line). NOT SwiftTerm's paste(): that leaves the view's text
    // input state such that every later Esc is swallowed — vim would be
    // stuck in Insert mode after the first Cmd+V.
    public func vimPaste() {
        guard let vv = vimView else { return }
        // an image on the clipboard: saved next to the note (assets/…) by the
        // host, exactly like the native editor, and linked as markdown —
        // the inline-image overlay then renders it under the link
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
        // no RPC (plain vim): bracketed paste written straight to the pty
        vv.send(txt: "\u{1b}[200~" + text + "\u{1b}[201~")
    }

    // Terminal cell size for a font — the same metrics the terminal view
    // lays its grid out with (line height = ceil(ascent + descent + leading),
    // width = advance of "W")
    static func cellSize(_ f: NSFont) -> NSSize {
        let h = ceil(CTFontGetAscent(f) + CTFontGetDescent(f) + CTFontGetLeading(f))
        var glyph = CTFontGetGlyphWithName(f, "W" as CFString)
        var adv = CGSize.zero
        CTFontGetAdvancesForGlyphs(f, .horizontal, &glyph, &adv, 1)
        return NSSize(width: adv.width, height: h)
    }

    // Watch the editor's image-placement file and redraw the overlay on every
    // write (scroll / edit / resize in vim) — no polling.
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
                // replaced on disk: re-arm on the new file
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

    // new font size -> new cell height -> images need a different number of
    // reserved rows
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
        ov.textRows = max(0, lines - 1)   // the bottom row is vim's command line
        ov.items = items
    }

    // Live font change (Font menu): editor text view, terminal drawer and
    // vim pane. nil leaves that part unchanged.
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
        if let vv = vimView { vv.font = PopupWindow.vimFont(config) }
        vimImageOverlay?.cell = PopupWindow.cellSize(PopupWindow.vimFont(config))
        refreshVimImageRows()
        layoutForZoom()
    }

    // themed color roles the paint-brush picker can style. Only the per-window
// BACKGROUNDS are interactive (card colors stay in [theme] in commands.conf):
// `browser` = the file-explorer panel, `terminal` = the shell drawer,
// `notepad` = the editor / window card fill, `header` = the drag-header tint.
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

    // Apply a picked color to ONE role in THIS window only. Each role touches
    // only its own surfaces — updating the terminal never restyles the
    // notepad or the file explorer, and text colors are never touched. The
    // color's ALPHA is the surface's opacity (the picker's opacity slider).
    public func setThemeColor(_ c: NSColor, for role: ThemeRole) {
        switch role {
        case .browser:
            config.fileBrowserBackground = c
            fileBrowser?.setBackground(c)
        case .terminal:
            config.terminalBackground = c
            if let term = terminalDrawer {
                term.nativeBackgroundColor = c
                // terminal text color stays the window's text color unless a
                // preset gave the drawer its own
                term.nativeForegroundColor = config.terminalForeground ?? config.colors.text
            }
        case .notepad:
            // the card fill only — the editor's text/selection colors are
            // never re-applied by the picker. The picked alpha becomes the
            // card's opacity (tintAlpha); the hue stays opaque so it never
            // fights the translucency.
            let cc = c.usingColorSpace(.sRGB) ?? c
            config.colors.background = cc.withAlphaComponent(1)
            config.tintAlpha = cc.alphaComponent
            tintView?.layer?.backgroundColor =
                config.colors.background.withAlphaComponent(config.tintAlpha).cgColor
            if config.opaqueTabs { tabsBar?.fill = config.colors.background }
            // keep the drag-header fill consistent when it falls back to the
            // card background
            chrome?.headerColorOverride = config.headerColor ?? config.colors.background
            applyThemeAppearance()
            // the deeper tones (tab strip, table header, wells) derive from
            // the card: re-push them
            pushColors()
        case .header:
            config.headerColor = c
            chrome?.headerColorOverride = c
        }
        panel.contentView?.needsDisplay = true
    }

    // Window-wide text palette (Theme ▸ presets, light <-> dark): the editor,
    // the vim pane, the shell drawer and every themed subview pick it up live.
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
            // the bundled init re-applies its highlights on ColorScheme
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

    // hand the window palette to every themed subview + the layer-drawn
    // chrome (card outline, focus rings, input wells)
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
        // only the fields drawn as wells (the list search bar, the find
        // bar): editor windows keep an invisible `field` over the editor
        // and painting it would cover the text
        for f in [field, findField].compactMap({ $0 }) where (f.layer?.borderWidth ?? 0) > 0 {
            f.layer?.backgroundColor = ButtonStyle.inputFill(c).cgColor
            f.layer?.borderColor = f === findField
                ? ButtonStyle.focusStroke(c).withAlphaComponent(0.6).cgColor
                : ButtonStyle.inputStroke(c).cgColor
        }
        panel.contentView?.needsDisplay = true
    }

    // Pin the window to the THEME's appearance instead of the Mac's: the
    // blur material, scrollers, menus and selection all follow the window
    // appearance, so a light-mode Mac used to wash dark presets out (and a
    // dark-mode one muddied light presets). Also re-pins text selection.
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

    // host fallback for `term` when this window has no shell drawer (files
    // window): open the configured terminal app in `dir`
    public var onOpenExternalTerminal: ((String) -> Void)?

    // cd the embedded shell drawer to `dir` (opening + focusing it), or hand
    // off to an external terminal when there's no drawer
    public func openTerminalHere(_ dir: String) {
        guard let term = terminalDrawer else {
            onOpenExternalTerminal?(dir)
            return
        }
        if !terminalShown { toggleTerminalDrawer() }
        let quoted = "'" + dir.replacingOccurrences(of: "'", with: "'\\''") + "'"
        // Ctrl+U clears whatever is half-typed at the prompt first
        term.send(txt: "\u{15}cd -- \(quoted)\r")
        panel.makeFirstResponder(term)
        focusedPane = .terminal
        updateFocusIndicator()
    }

    // live float toggle (header icon menu ▸ Float Above Other Windows)
    public func setFloating(_ on: Bool) {
        config.floating = on
        panel.level = on ? .popUpMenu : .normal
    }

    // `let g:ws_…` for the vim pane's palette roles (vim/notes-init.vim)
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

    // The shell drawer's 16 ANSI colors from the theme (ls, git, prompts):
    // red/green/yellow/cyan = danger/success/warning/info; blue + magenta
    // come from accent / accent2 (whichever sits nearer the hue), shifted to
    // the right hue when the theme has no such color; blacks/whites from the
    // surfaces. Bright = nudged toward the text color.
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

    // the shell drawer's own text color (nil = follow the window text)
    public func setTerminalForeground(_ c: NSColor?) {
        config.terminalForeground = c
        terminalDrawer?.nativeForegroundColor = c ?? config.colors.text
    }

    public var hasTerminalDrawer: Bool { terminalDrawer != nil }
    public var hasFileBrowser: Bool { fileBrowser != nil }

    // the current window's drag-header rect (for popping the theme menu under
    // the paint-brush button)
    public func headerButtonRect(_ id: Int) -> NSRect? {
        chrome?.extraButtonRects[id]
    }

    // Host installs a file browser. `drawer` = true makes it a bottom drawer
    // toggled like the terminal (notes); false makes it fill the content area
    // below the chrome (the standalone "files" window).
    func installFileBrowser(_ fb: PopupFileBrowser, drawer: Bool) {
        fileBrowser = fb
        fileBrowserDrawerMode = drawer
        guard let backdrop = panel.contentView else { return }
        fb.autoresizingMask = [.width, .height]
        currentBrowserHeight = preferredBrowserHeight
        // right-click "Open in Notes" -> host hook
        fb.onOpenInNotes = { [weak self] p in
            self?.onFileBrowserOpenInNotes?(p)
        }
        // `term` in the filter bar / right-click "Open Terminal Here"
        fb.onOpenTerminal = { [weak self] dir in
            self?.openTerminalHere(dir)
        }
        if let chrome = self.chrome {
            backdrop.addSubview(fb, positioned: .below, relativeTo: chrome)
        } else {
            backdrop.addSubview(fb)
        }
        if drawer && config.fileBrowserDefault {
            // the file browser is the default pane: start open, terminal closed
            fileBrowserShown = true
            terminalShown = false
            drawerInsetNow = config.fileBrowserHeight
        }
        // drawer mode starts hidden unless it's the default; content mode is
        // always visible
        fb.isHidden = drawer ? !fileBrowserShown : false
        // focus indicator: bright 4-sided border around the browser
        let bfb = NSView(frame: fb.frame)
        bfb.autoresizingMask = [.width, .height]
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

    public func toggleFileBrowser() {
        guard fileBrowser != nil, fileBrowserDrawerMode else { return }
        fileBrowserShown.toggle()
        fileBrowser?.isHidden = !fileBrowserShown
        if fileBrowserShown {
            currentBrowserHeight = preferredBrowserHeight
        }
        syncDrawerLayout()
        if fileBrowserShown, let lp = fileBrowser?.listView {
            panel.makeFirstResponder(lp)
            focusedPane = .browser
        } else if let ed = primaryEditor {
            panel.makeFirstResponder(ed)
            focusedPane = .editor
        }
        updateFocusIndicator()
    }

    // Both drawers can be open at once (terminal + file browser stacked); the
    // window grows so the editor never overlaps them. The total drawer height
    // is folded into the window frame and the panes laid out accordingly.
    private func drawerInsetTotal() -> CGFloat {
        (terminalShown ? currentTerminalHeight : 0)
            + (fileBrowserShown ? currentBrowserHeight : 0)
    }
    private func syncDrawerLayout() {
        let want = drawerInsetTotal()
        if abs(want - drawerInsetNow) > 0.5 {
            let f = panel.frame
            panel.setFrame(NSRect(x: f.origin.x, y: f.origin.y,
                                  width: f.width, height: f.height + (want - drawerInsetNow)),
                           display: true)
            drawerInsetNow = want
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
            // stack the browser ABOVE the terminal drawer (terminal keeps the
            // very bottom), so both can be visible at once
            let termH = terminalShown ? currentTerminalHeight : 0
            let y = max(0, backdrop.bounds.height - meter - termH - h)
            fb.frame = NSRect(x: terminalInset, y: y,
                              width: max(0, backdrop.bounds.width - 2 * terminalInset),
                              height: h)
        } else {
            // fill the content area below the drag header / tabs
            let tabH = (tabsBar?.frame.height ?? 0)
            let topY = config.headerHeight * zoom + tabH + 2
            fb.frame = NSRect(x: 0, y: topY, width: backdrop.bounds.width,
                              height: max(40, backdrop.bounds.height - topY))
        }
        fb.needsLayout = true
        fb.layoutSubtreeIfNeeded()
    }

    // Update the bright focus border so it highlights whichever pane (editor /
    // browser / terminal) currently owns first responder.
    private func updateFocusIndicator() {
        fileBrowser?.updatePartFocus()
        editorFocusBorder?.isHidden = true
        browserFocusBorder?.isHidden = true
        terminalFocusBorder?.isHidden = true
        switch focusedPane {
        case .editor:
            if let scroll = editorScroll, let eb = editorFocusBorder {
                eb.frame = scroll.frame
                eb.isHidden = false
            }
        case .browser:
            if let fb = fileBrowser, let bb = browserFocusBorder, fileBrowserShown {
                bb.frame = fb.frame
                bb.isHidden = false
            }
        case .terminal:
            if let term = terminalDrawer, let tb = terminalFocusBorder, terminalShown {
                tb.frame = term.frame
                tb.isHidden = false
            }
        case nil:
            break
        }
    }

    // does the embedded terminal hold keyboard focus? (keyboard routing: let
    // SwiftTerm see everything while the shell is focused)
    private func terminalFocused(_ term: LocalProcessTerminalView) -> Bool {
        let fr = panel.firstResponder
        if fr === term { return true }
        if let v = fr as? NSView { return v.isDescendant(of: term) }
        return false
    }

    // the terminal view, but only when the drawer is shown AND it has focus —
    // edit shortcuts (copy/paste/select-all) route to it instead of the editor
    private func focusedTerm() -> LocalProcessTerminalView? {
        guard let term = terminalDrawer, terminalShown, terminalFocused(term) else { return nil }
        return term
    }

    // does the file browser own keyboard focus? (search field editing, or the
    // first responder is inside the browser — list, pills, etc.)
    private func browserHasFocus(_ fb: PopupFileBrowser) -> Bool {
        if fb.searchView.currentEditor() != nil { return true }
        let fr = panel.firstResponder
        if let v = fr as? NSView { return v.isDescendant(of: fb) }
        return false
    }
    // is the browser on screen at all? drawer mode = shown; content mode =
    // (floating files window) always visible
    private func browserActive() -> Bool {
        guard let fb = fileBrowser else { return false }
        _ = fb
        return fileBrowserDrawerMode ? fileBrowserShown : true
    }

    // MARK: Find in note

    private var findBarShown: Bool {
        guard let ff = findField else { return false }
        return !ff.isHidden
    }

    // Ctrl/Cmd+F: show the find bar (prefilled with the current selection),
    // or close it if it is already open
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

    // re-run the find across the note's plain text, jump to the first match
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
        let tabH = tabsBar?.frame.height ?? config.tabBarHeight * zoom
        let y = config.headerHeight * zoom + tabH + 4
        let h: CGFloat = 24
        if ff.isHidden {
            ff.frame = .zero
            findCountLabel?.frame = .zero
            return
        }
        ff.frame = NSRect(x: 12, y: y,
                          width: max(120, backdrop.bounds.width - 24 - 56),
                          height: h)
        findCountLabel?.frame = NSRect(x: backdrop.bounds.width - 50, y: y,
                                       width: 42, height: h)
    }

    private func findBarHeight() -> CGFloat {
        findBarShown ? 24 + 6 : 0
    }

    // The drawer's height is FIXED (config.terminalHeight) — the window is
    // created with exactly that much extra, and toggling adds/removes the same
    // amount, so show/hide never changes the window's proportions (a
    // proportional height recomputed from the window height caused the toggle
    // to ratchet the size smaller each time).
    private func terminalDrawerHeight() -> CGFloat {
        terminalShown ? currentTerminalHeight : 0
    }

    private func layoutTerminal() {
        guard let drawer = terminalDrawer, let backdrop = panel.contentView else { return }
        let h = terminalDrawerHeight()
        // the voice meter/record strip overlays the bottom of the window:
        // stop the terminal ABOVE it so the cursor/typed line is never hidden
        // behind the record button
        let meter = (chrome?.meterEnabled ?? false) ? chrome!.meterBarHeight : 0
        let y = max(0, backdrop.bounds.height - meter - h)
        // bottom-anchored drawer: the editor (and its scroll bar) stop above
        // it, so content can never scroll behind the terminal. A tiny side
        // inset keeps the shell's first/last columns off the window edges.
        drawer.frame = NSRect(x: terminalInset, y: y,
                              width: max(0, backdrop.bounds.width - 2 * terminalInset),
                              height: h)
    }

    private func layoutEditorScroll() {
        guard config.editMode, let scroll = editorScroll,
              let backdrop = panel.contentView else { return }
        let tabH = tabsBar?.frame.height ?? config.tabBarHeight * zoom
        let topY = config.headerHeight * zoom + tabH + 2 + findBarHeight()
        // the voice meter/record strip owns the bottom of the window: stop the
        // editor above it so the caret (and the last dictated line) is never
        // hidden behind the record button
        let meter = (chrome?.meterEnabled ?? false) ? chrome!.meterBarHeight + 4 : 0
        // the drawer (terminal or file browser) owns the bottom: stop the editor
        // above it while one is shown — use actual current heights, not cached
        let drawerH = (terminalShown ? currentTerminalHeight : 0)
                    + (fileBrowserShown ? currentBrowserHeight : 0)
        let drawer = drawerH + 4
        // the transient status strip (prettyprint errors) reserves its band
        // above the meter; the editor shrinks to make room
        let statusVisible = !(statusBar?.isHidden ?? true)
        let status = statusVisible ? statusBarHeight + 4 : 0
        scroll.frame.origin.y = topY
        scroll.frame.size.height = max(40, backdrop.bounds.height - topY - meter - drawer - status)
        // the vim pane mirrors the text view's frame (inset like its text)
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
            // keep text wrapping at the (possibly resized) window width while
            // wide photos overflow into the horizontal scroller
            editorView?.textContainer?.containerSize = NSSize(
                width: max(120, scroll.bounds.width - 24),
                height: CGFloat.greatestFiniteMagnitude)
            syncEditorDocWidth()
        }
    }

    // Labels are NEVER clipped: when the filter bar or the header cluster
    // needs more room than the window has, grow the window (capped at the
    // visible screen) instead of ellipsizing any label.
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

    // Search bar shares the rows'/filter bar's left inset and takes
    // searchWidthFraction of the INNER width, so the whole column lines up
    // instead of the bar floating centered above a left-aligned pill row.
    private func layoutSearchField() {
        guard !config.editMode, let backdrop = panel.contentView else { return }
        let w = backdrop.bounds.width
        let inset = config.padding + 10
        let fieldW = max(50, (w - 2 * inset) * config.searchWidthFraction)
        let x = inset
        let headerOffset = (config.dragHeader) ? config.headerHeight * zoom + 4 : 0
        let y = headerOffset + config.padding + 2
        field.frame = NSRect(x: x, y: y, width: fieldW, height: 24)
    }

    // MARK: Placement

    // Cap the popup height so huge lists never overflow the screen: explicit
    // maxHeight from config, else 60% of the visible screen height.
    private func maxPanelHeight() -> CGFloat {
        let screenH = NSScreen.main?.visibleFrame.height ?? 800
        let cap = config.maxHeight > 0 ? config.maxHeight : screenH * 0.6
        return max(100, min(cap, screenH - 40))
    }

    // Cap auto-grown widths (filter bar growth) to the visible screen.
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

    // MARK: Toggle server (name-scoped messages)

    private func startToggleServer() {
        let socketPath = popupTmpDir() + config.name + ".sock"
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            unlink(socketPath)
            let fd = socket(AF_UNIX, SOCK_STREAM, 0)
            guard fd >= 0 else { return }
            var addr = makeUnixSockAddr(socketPath)
            let bound = withUnsafePointer(to: &addr) { ptr -> Bool in
                ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                    Darwin.bind(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size)) == 0
                }
            }
            guard bound else { close(fd); return }
            listen(fd, 4)
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
                        // debounce: rapid double-presses collapse into one
                        // toggle instead of show->hide churn
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

// MARK: - Toggle client

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

// MARK: - PopupStack (breadcrumb trail of open popups)

// LIFO stack of PopupWindows: push() hides the current top and shows the new
// one; pop() hides the top and restores the previous — so nested popups
// remember where they came from (breadcrumbs), e.g. /command -> sub-window ->
// Esc -> back to the main switcher.
public final class PopupStack {
    private var stack: [PopupWindow] = []

    public var top: PopupWindow? { stack.last }
    public var depth: Int { stack.count }
    public var names: [String] { stack.map { $0.config.name } }

    public init() {}

    public func push(_ w: PopupWindow) {
        if let top = stack.last, top.isShown {
            top.hide(restore: false)
        }
        stack.append(w)
        w.show()
        FileHandle.standardError.write(
            Data("popup-stack: push '\(w.config.name)' -> \(names.joined(separator: " / "))\n".utf8))
    }

    @discardableResult
    public func pop() -> PopupWindow? {
        guard let top = stack.last else { return nil }
        top.hide(restore: false)
        stack.removeLast()
        if let prev = stack.last {
            prev.show()
        }
        FileHandle.standardError.write(
            Data("popup-stack: pop '\(top.config.name)' -> \(names.joined(separator: " / "))\n".utf8))
        return top
    }

    public func popAll() {
        while stack.count > 0 {
            stack.last?.hide(restore: false)
            stack.removeLast()
        }
        FileHandle.standardError.write(Data("popup-stack: popped all\n".utf8))
    }
}
