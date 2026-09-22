import AppKit
import PDFKit
import SwiftTerm
import Foundation
import Darwin

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
func editorFont(_ name: String?, _ zoom: CGFloat) -> NSFont {
    name.flatMap { NSFont(name: $0, size: 13 * zoom) }
        ?? NSFont.monospacedSystemFont(ofSize: 13 * zoom, weight: .regular)
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
    public var accent: NSColor       // active segment fill in joined button bars

    public init(background: NSColor = NSColor(srgbRed: 36/255, green: 39/255, blue: 58/255, alpha: 1),
                border: NSColor = NSColor(srgbRed: 159/255, green: 200/255, blue: 232/255, alpha: 1),
                text: NSColor = NSColor(srgbRed: 202/255, green: 211/255, blue: 245/255, alpha: 1),
                dim: NSColor = NSColor(srgbRed: 147/255, green: 154/255, blue: 183/255, alpha: 1),
                highlight: NSColor = NSColor(srgbRed: 63/255, green: 74/255, blue: 90/255, alpha: 1),
                accent: NSColor = NSColor(srgbRed: 85/255, green: 104/255, blue: 130/255, alpha: 1)) {
        self.background = background
        self.border = border
        self.text = text
        self.dim = dim
        self.highlight = highlight
        self.accent = accent
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

    // standardized button theme (pills, tabs, header segments, filter chips)
    // — one look across every window: accent action fill, border stroke,
    // consistent radius, hover/pressed feedback driven by the same colors
    public var buttonRadius: CGFloat = 6
    public var buttonFontSize: CGFloat = 10.5
    // hover/pressed shading applied over the normal fill (0 = none)
    public var buttonHoverAlpha: CGFloat = 0.18
    public var buttonPressedAlpha: CGFloat = 0.32

    // UI zoom: scales fonts, row heights and chrome sizes proportionally
    // (Ctrl/Cmd+± drives it alongside the window resize)
    public var zoom: CGFloat = 1.0

    // appearance
    public var tintAlpha: CGFloat = 0.78            // card fill opacity over the blur
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
    // when a file-browser drawer is installed, open it (and close the
    // terminal) from the start instead of the terminal being the default
    public var fileBrowserDefault = false
    // shell the terminal drawer (and the host's command runner) spawn
    public var shell = "/opt/homebrew/bin/bash"
    // font for the terminal drawer (a Nerd Font so glyphs/powerline render)
    public var terminalFont = "Hack Nerd Font"
    // args passed to that shell: --login -i makes it read the profile AND
    // rc files (~/.bash_profile + ~/.bashrc), so aliases/functions/zoxide etc.
    // defined there work in the embedded terminal
    public var shellArgs: [String] = ["--login", "-i"]
    // optional override: use a different executable than the shell (e.g. nvim
    // for vim mode). When set, the terminal launches this instead of shell.
    public var terminalExecutable: String?
    // optional args for the terminal executable (default: [])
    public var terminalExecArgs: [String] = []
    // show the standard window close button (red traffic light). By default
    // it's hidden on titled windows (Esc closes instead); set true to show it
    // and wire it to onCloseWindow.
    public var showCloseButton: Bool = false

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
    public var tabBarHeight: CGFloat = 26
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

    // cap on how much a single row may stretch when the window is resized
    // larger than its content: filling a tall window with few rows would
    // otherwise leave huge empty gaps between the pills
    public var maxRowStretch: CGFloat = 26

    // highlight the query's matched characters in row titles/content
    // (fzf-style), using the current search text
    public var highlightMatches: Bool = false

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
}

public extension PopupRow {
    var icons: [NSImage] { [] }
    var trailing: String? { nil }
    var content: String? { nil }
    var detail: String? { nil }
    var body: String? { nil }
    var loadMore: Bool { false }
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
public final class PopupPlainWindow: PopupBaseWindow {}

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
        guard config.enableResize else { return }
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
        if config.enableResize && !e.isEmpty, let win = window {
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
final class PopupTabsBar: NSView {
    let config: PopupConfig
    var zoom: CGFloat = 1.0
    var titles: [String] = [] {
        didSet { needsDisplay = true }
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
    private let gap: CGFloat = 6
    private var addW: CGFloat { 30 * zoom }
    // small ✕ badge drawn at each tab pill's top-left corner (hover only)
    private var closeSize: CGFloat { 12 * zoom }
    // index (into pillRects) of the tab whose ✕ is under the cursor
    private var hoverCloseIndex: Int?
    private var trackingArea: NSTrackingArea?

    override var isFlipped: Bool { true }

    init(config: PopupConfig) {
        self.config = config
        super.init(frame: .zero)
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
        if hoverCloseIndex != nil {
            hoverCloseIndex = nil
            needsDisplay = true
        }
    }
    override func mouseMoved(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        var hover: Int?
        for (i, (_, title, close)) in pillRects().enumerated() {
            if title != "+", let close,
               close.insetBy(dx: -3, dy: -3).contains(p) {
                hover = i
                break
            }
        }
        if hover != hoverCloseIndex {
            hoverCloseIndex = hover
            needsDisplay = true
        }
    }

    private func tabWidth(_ title: String) -> CGFloat {
        let attrs: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: config.buttonFontSize * zoom, weight: .semibold),
        ]
        // extra padding on the left so the centered title clears the ✕ badge
        return (title as NSString).size(withAttributes: attrs).width + 32 * zoom
    }

    // Number of wrapped rows the pills occupy at `width` (the "+" first).
    private func rowCount(forWidth width: CGFloat) -> Int {
        var x: CGFloat = config.padding + 4 + (config.tabsAddButton ? addW + gap : 0)
        var rows = 1
        for t in titles {
            let tw = tabWidth(t) + gap
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
        CGFloat(rowCount(forWidth: width)) * (tabH + gap) - gap
    }

    // Wrapped layout of every pill; the "+" add button is first. Each tab pill
    // carries its ✕ badge rect (top-left corner), nil for the "+" button.
    private func pillRects() -> [(rect: NSRect, title: String, close: NSRect?)] {
        var out: [(NSRect, String, NSRect?)] = []
        var x: CGFloat = config.padding + 4
        var y: CGFloat = 0
        if config.tabsAddButton {
            out.append((NSRect(x: x, y: y, width: addW, height: tabH), "+", nil))
            x += addW + gap
        }
        for t in titles {
            let tw = tabWidth(t)
            if x + tw + gap > bounds.width - config.padding {
                y += tabH + gap
                x = config.padding + 4
            }
            let rect = NSRect(x: x, y: y, width: tw, height: tabH)
            let close = NSRect(x: x + 2, y: y + 2, width: closeSize, height: closeSize)
            out.append((rect, t, close))
            x += tw + gap
        }
        return out
    }

    override func draw(_ dirtyRect: NSRect) {
        for (i, (rect, title, close)) in pillRects().enumerated() {
            let path = NSBezierPath(roundedRect: rect, xRadius: config.buttonRadius,
                                    yRadius: config.buttonRadius)
            let isSelectedTab = title != "+" && titles.firstIndex(of: title) == selected
            if title == "+" {
                config.colors.accent.withAlphaComponent(0.5).setFill()
                path.fill()
            } else if isSelectedTab {
                config.colors.accent.withAlphaComponent(0.7).setFill()
                path.fill()
                config.colors.border.withAlphaComponent(0.8).setStroke()
                path.lineWidth = 1
                path.stroke()
            } else {
                config.colors.highlight.withAlphaComponent(0.3).setFill()
                path.fill()
            }
            let attrs: [NSAttributedString.Key: Any] = [
                .font: NSFont.systemFont(ofSize: (title == "+" ? 13 : config.buttonFontSize) * zoom,
                                         weight: title == "+" ? .medium : .semibold),
                .foregroundColor: isSelectedTab ? config.colors.text : config.colors.dim,
            ]
            let s = title as NSString
            let sz = s.size(withAttributes: attrs)
            s.draw(at: NSPoint(x: rect.midX - sz.width / 2, y: rect.midY - sz.height / 2),
                   withAttributes: attrs)
            // ✕ close badge — only pops in over the pill's top-left corner
            if i == hoverCloseIndex, let close {
                let badge = NSBezierPath(roundedRect: close, xRadius: 3, yRadius: 3)
                (isSelectedTab ? config.colors.highlight
                               : config.colors.background).withAlphaComponent(0.85).setFill()
                badge.fill()
                config.colors.dim.withAlphaComponent(0.6).setStroke()
                badge.lineWidth = 0.8
                badge.stroke()
                let xAttrs: [NSAttributedString.Key: Any] = [
                    .font: NSFont.systemFont(ofSize: 7.5 * zoom, weight: .semibold),
                    .foregroundColor: config.colors.text.withAlphaComponent(0.75),
                ]
                let xmark = "✕" as NSString
                let xsz = xmark.size(withAttributes: xAttrs)
                xmark.draw(at: NSPoint(x: close.midX - xsz.width / 2,
                                       y: close.midY - xsz.height / 2),
                           withAttributes: xAttrs)
            }
        }
    }

    override func mouseDown(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        // the ✕ badge has priority over selecting the tab
        for (rect, title, close) in pillRects() where rect.contains(p) {
            if title == "+" {
                onAddTab?()
            } else if let close, close.contains(p), let i = titles.firstIndex(of: title) {
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
    let config: PopupConfig
    var zoom: CGFloat = 1.0
    var labels: [String] = []
    var values: [[String]] = []      // per dimension; index 0 = "All"
    // display titles parallel to `values` (empty = show the raw value) — lets
    // a dropdown show "13.1 (2026-10-15)" while matching on the raw "13.1"
    var valueLabels: [[String]] = []
    var selections: [Int] = []       // selected value index per dimension
    var onSelect: ((Int, Int) -> Void)?
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
        let sel = selections.indices.contains(dim) ? selections[dim] : 0
        return "\(label): \(optionTitle(dim, sel)) ▾"
    }

    // Total width the bar needs with FULL (untruncated) titles — used to
    // decide whether segments must shrink to fit the window.
    func naturalWidth() -> CGFloat {
        var w: CGFloat = 0
        for i in 0..<labels.count {
            w += (currentTitle(i) as NSString).size(withAttributes: fontAttrs).width + segPad * 2
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
            let w = (t as NSString).size(withAttributes: fontAttrs).width + segPad * 2
            out.append((NSRect(x: x, y: (bounds.height - pillH) / 2,
                               width: w, height: pillH), i, t))
            x += w + sepW
        }
        return out
    }

    override func draw(_ dirtyRect: NSRect) {
        let rects = pillRects()
        guard let first = rects.first, let last = rects.last else { return }
        // one long bar across all the dropdown segments
        let bar = NSRect(x: first.0.minX, y: first.0.minY,
                         width: last.0.maxX - first.0.minX, height: pillH)
        let bp = NSBezierPath(roundedRect: bar, xRadius: config.buttonRadius,
                              yRadius: config.buttonRadius)
        config.colors.accent.withAlphaComponent(0.45).setFill()
        bp.fill()
        config.colors.border.withAlphaComponent(0.5).setStroke()
        bp.lineWidth = 1
        bp.stroke()
        for (i, (rect, dim, title)) in rects.enumerated() {
            let active = selections.indices.contains(dim) && selections[dim] > 0
            let flashing = flashDim == dim
            if i > 0 {
                // thin | divider — hidden when either neighbor is active/
                // flashing so the accent fill reads as one solid segment
                let prev = rects[i - 1]
                let prevActive = selections.indices.contains(prev.1)
                    && selections[prev.1] > 0
                let prevFlash = flashDim == prev.1
                if !prevActive && !active && !prevFlash && !flashing {
                    config.colors.border.withAlphaComponent(0.35).setStroke()
                    let d = NSBezierPath()
                    d.lineWidth = 1
                    d.move(to: NSPoint(x: rect.minX, y: bar.minY + 5))
                    d.line(to: NSPoint(x: rect.minX, y: bar.maxY - 5))
                    d.stroke()
                }
            }
            if flashing || active {
                // selected chip: accent fill + border hairline — clearly the
                // active segment of the bar (brighter than the idle accent)
                let chip = NSBezierPath(roundedRect: rect.insetBy(dx: 2, dy: 3),
                                        xRadius: config.buttonRadius - 1,
                                        yRadius: config.buttonRadius - 1)
                config.colors.accent.withAlphaComponent(flashing ? 0.6 : 0.35).setFill()
                chip.fill()
                config.colors.border.withAlphaComponent(0.85).setStroke()
                chip.lineWidth = 1
                chip.stroke()
            }
            let attrs: [NSAttributedString.Key: Any] = [
                .font: NSFont.systemFont(ofSize: config.buttonFontSize * zoom,
                                         weight: .semibold),
                .foregroundColor: (active || flashing)
                    ? config.colors.text : config.colors.dim,
            ]
            let s = title as NSString
            let sz = s.size(withAttributes: attrs)
            s.draw(at: NSPoint(x: rect.midX - sz.width / 2,
                               y: rect.midY - sz.height / 2),
                   withAttributes: attrs)
        }
    }

    override func mouseDown(with event: NSEvent) {
        let p = convert(event.locationInWindow, from: nil)
        for (rect, dim, _) in pillRects() where rect.contains(p) {
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
    // The search bar is taller than the cell's natural height, and AppKit pins
    // a bezel-less cell's text/field-editor rect near the TOP of the control —
    // the caret and typed text then sit visibly high in the bar. Center every
    // text rect vertically so the caret lands on the bar's midline.
    private func centeredTextRect(in r: NSRect) -> NSRect {
        let h = cellSize.height
        return NSRect(x: r.minX, y: r.midY - h / 2, width: r.width, height: h)
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
    let config: PopupConfig
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

    override var isFlipped: Bool { true }

    init(config: PopupConfig) {
        self.config = config
        super.init(frame: .zero)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) not supported") }

    // left inset of row text: the checkbox column is reserved when present,
    // so height measurement and drawing always agree
    private var contentX: CGFloat {
        config.padding + 10 + (config.selectableRows ? 22 : 0)
    }

    private func checkBoxRect(in band: NSRect) -> NSRect {
        let s: CGFloat = 13
        return NSRect(x: config.padding + 2, y: band.midY - s / 2, width: s, height: s)
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
            config.colors.border.withAlphaComponent(0.8).setStroke()
            p.lineWidth = 1.5
            p.stroke()
        }
        if config.selectableRows {
            drawCheckBox(checkBoxRect(in: band), on: selected.contains(index))
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
            let accent = config.colors.border
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

    // Rounded checkbox: dim outline when unticked, filled + check mark when in
    // the copy selection.
    private func drawCheckBox(_ r: NSRect, on: Bool) {
        let p = NSBezierPath(roundedRect: r, xRadius: 3, yRadius: 3)
        if on {
            config.colors.border.setFill()
            p.fill()
            let tick = NSBezierPath()
            tick.lineWidth = 1.6
            tick.move(to: NSPoint(x: r.minX + 3, y: r.midY - 0.5))
            tick.line(to: NSPoint(x: r.midX - 0.5, y: r.maxY - 3.5))
            tick.line(to: NSPoint(x: r.maxX - 2.5, y: r.minY + 3))
            config.colors.background.setStroke()
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
                          wrap: Bool, highlight: Bool) {
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
    private let config: PopupConfig
    var title: String { didSet { needsDisplay = true } }
    // "on" state (e.g. ★ pinned): rendered with the solid highlight fill so
    // an active toggle reads clearly against the idle accent buttons
    var isOn = false { didSet { needsDisplay = true } }
    var onClick: (() -> Void)?
    private var down = false
    private var hover = false
    private var trackingArea: NSTrackingArea?

    init(config: PopupConfig, title: String) {
        self.config = config
        self.title = title
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

    override func draw(_ dirty: NSRect) {
        let c = config.colors
        // accent fill, shaded by hover/pressed; selected-style "on" buttons
        // (★ pinned) use the highlight fill instead so state reads clearly
        let fill: NSColor
        if down {
            fill = c.highlight.withAlphaComponent(config.buttonPressedAlpha)
        } else if isOn {
            fill = c.highlight.withAlphaComponent(0.9)
        } else if hover {
            fill = c.accent.blended(withFraction: 0.25, of: c.text)
                ?? c.accent.withAlphaComponent(0.75)
        } else {
            fill = c.accent.withAlphaComponent(0.55)
        }
        let r = NSBezierPath(roundedRect: bounds.insetBy(dx: 0.5, dy: 0.5),
                             xRadius: config.buttonRadius, yRadius: config.buttonRadius)
        fill.setFill()
        r.fill()
        if isOn {
            c.border.withAlphaComponent(0.9).setStroke()
            r.lineWidth = 1.5
        } else {
            c.border.withAlphaComponent(0.5).setStroke()
            r.lineWidth = 1
        }
        r.stroke()
        let attrs: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: config.buttonFontSize, weight: .semibold),
            .foregroundColor: c.text,
        ]
        let s = title as NSString
        let sz = s.size(withAttributes: attrs)
        s.draw(at: NSPoint(x: bounds.midX - sz.width / 2,
                           y: bounds.midY - sz.height / 2), withAttributes: attrs)
    }
    override func mouseDown(with e: NSEvent) { down = true; needsDisplay = true }
    override func mouseUp(with e: NSEvent) {
        if bounds.contains(convert(e.locationInWindow, from: nil)) { onClick?() }
        down = false
        needsDisplay = true
    }
}

// The browser's file list: custom-drawn rows (icon + name + size), click to
// select (preview), double-click/Return to open, arrows to move. Printable
// keys hand focus to the search field.
final class FileListPane: NSView {
    private let config: PopupConfig
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

    private let rowH: CGFloat = 22
    private static let iconSize: CGFloat = 16
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
        for (i, e) in rows.enumerated() {
            let r = rowRect(i)
            if i == selection {
                config.colors.highlight.setFill()
                r.fill()
            } else if let h = hover, h == i {
                config.colors.highlight.withAlphaComponent(0.4).setFill()
                r.fill()
            }
            let icon = e.icon ?? NSWorkspace.shared.icon(forFile: e.path)
            var ir = r
            ir.origin.x += 6
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
            let avail = w - tx - 8 - (e.trailingWidth > 0 ? e.trailingWidth + 10 : 0)
            name.draw(with: NSRect(x: tx, y: r.minY + (rowH - 15) / 2, width: max(20, avail), height: 15),
                      options: [.truncatesLastVisibleLine, .usesLineFragmentOrigin],
                      attributes: nameAttrs)
            if e.isDir && e.name != ".." {
                // small folder glyph after the name (dim)
                let dirAttrs: [NSAttributedString.Key: Any] = [
                    .font: NSFont.systemFont(ofSize: 10, weight: .regular),
                    .foregroundColor: config.colors.dim,
                ]
                let d = (e.name as NSString).size(withAttributes: nameAttrs)
                let g = "/" as NSString
                g.draw(at: NSPoint(x: tx + d.width + 2, y: r.minY + (rowH - 12) / 2), withAttributes: dirAttrs)
            }
            if e.trailingWidth > 0 {
                let szAttrs: [NSAttributedString.Key: Any] = [
                    .font: NSFont.systemFont(ofSize: 10, weight: .regular),
                    .foregroundColor: config.colors.dim,
                ]
                let s = e.trailingText as NSString
                s.draw(at: NSPoint(x: w - e.trailingWidth - 8, y: r.minY + (rowH - 12) / 2),
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
final class BrowserSearchField: NSTextField {}

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
    let config: PopupConfig
    var onOpen: ((String) -> Void)?          // open a FILE in its default app
    var onDirChange: ((String) -> Void)?     // cwd changed (host labels)
    var onCopyDir: ((String) -> Void)?       // copy current dir path
    var onCopyPath: ((String) -> Void)?      // copy an arbitrary path (hover/right-click)
    var onOpenInNotes: ((String) -> Void)?   // right-click "Open in Notes"
    var onStatus: ((String) -> Void)?        // transient feedback line

    struct Entry {
        let name: String
        let path: String
        let isDir: Bool
        let size: Int
        var icon: NSImage?
        var trailingText: String = ""
        var trailingWidth: CGFloat = 0
    }

    private static var iconCache: [String: NSImage] = [:]

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

    // listPane needs to be focusable from the window (drawer toggle)
    var listView: FileListPane { listPane }
    // the search field is exposed so the window can route Cmd+V/C/A etc. to
    // the filter bar (otherwise they land in the notes editor / hidden field)
    var searchView: NSTextField { searchField }

    // live restyle from the color picker: swap the panel background without
    // rebuilding the browser (the color's own alpha sets the translucency)
    func setBackground(_ c: NSColor) {
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
        searchField.layer?.backgroundColor = c.highlight.withAlphaComponent(0.4).cgColor
        searchField.layer?.borderColor = c.border.withAlphaComponent(0.45).cgColor
        previewText.textColor = c.text
        previewHint.textColor = c.dim
        splitter.layer?.backgroundColor = c.border.withAlphaComponent(0.35).cgColor
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
        self.parentButton = ThemeButton(config: config, title: "← up")
        self.starButton = ThemeButton(config: config, title: "★ pin")
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
        searchField.layer?.backgroundColor = config.colors.highlight.withAlphaComponent(0.4).cgColor
        searchField.layer?.cornerRadius = 6
        searchField.layer?.borderWidth = 1
        searchField.layer?.borderColor = config.colors.border.withAlphaComponent(0.45).cgColor

        parentButton.onClick = { [weak self] in self?.cdParent() }
        starButton.onClick = { [weak self] in self?.toggleStar() }

        previewText.isEditable = false
        previewText.isSelectable = true
        previewText.drawsBackground = false
        previewText.textContainerInset = NSSize(width: 8, height: 8)
        previewText.font = NSFont.monospacedSystemFont(ofSize: 12, weight: .regular)
        previewText.textColor = config.colors.text
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
        splitter.layer?.backgroundColor = config.colors.border.withAlphaComponent(0.35).cgColor
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

        addSubview(parentButton)
        addSubview(searchField)
        addSubview(starButton)
        addSubview(listScroll)
        addSubview(splitter)
        addSubview(previewScroll)
        addSubview(previewImage)
        addSubview(previewHint)
        addSubview(previewListScroll)

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
    }
    private func layoutPanes() {
        let w = bounds.width
        let toolbarY: CGFloat = 4
        let toolbarH: CGFloat = 24
        // pin first (left), then parent, then the filter bar fills the rest
        starButton.frame = NSRect(x: 6, y: toolbarY, width: 74, height: toolbarH)
        parentButton.frame = NSRect(x: starButton.frame.maxX + 4, y: toolbarY,
                                    width: 44, height: toolbarH)
        searchField.frame = NSRect(x: parentButton.frame.maxX + 6, y: toolbarY,
                                   width: max(60, w - parentButton.frame.maxX - 12),
                                   height: toolbarH)
        // favorites wrap to as many lines as their paths need
        let favY = toolbarY + toolbarH + 5
        let favH = layoutFavorites(from: favY)
        let listY = favY + favH + 4
        let bottomH: CGFloat = 0
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
        let pillH: CGFloat = 20
        let gap: CGFloat = 4
        var x: CGFloat = 6
        var y = y0
        for p in favPills {
            let pw = pillWidth(p.title)
            if x + pw > bounds.width - 6, x > 6 {
                x = 6
                y += pillH + gap
            }
            p.frame = NSRect(x: x, y: y, width: pw, height: pillH)
            x += pw + gap
        }
        return (y - y0) + pillH
    }
    private func pillWidth(_ t: String) -> CGFloat {
        let attrs: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: config.buttonFontSize, weight: .semibold)]
        return (t as NSString).size(withAttributes: attrs).width + 18
    }
    // ~/notes instead of /Users/me/notes for pinned/config paths under home
    private func displayPath(_ p: String) -> String {
        let home = NSHomeDirectory()
        if p.hasPrefix(home + "/") { return "~" + p.dropFirst(home.count) }
        return p
    }

    // MARK: data

    private func listDir(_ dir: String) -> [Entry] {
        let fm = FileManager.default
        guard let names = try? fm.contentsOfDirectory(atPath: dir) else { return [] }
        var out: [Entry] = []
        if (dir as NSString).pathComponents.count > 1 {
            let parent = (dir as NSString).deletingLastPathComponent
            var up = Entry(name: "..", path: parent, isDir: true, size: 0)
            up.icon = Self.iconCache[parent] ?? NSWorkspace.shared.icon(forFile: parent)
            out.append(up)
        }
        for n in names where !n.hasPrefix(".") {
            let p = (dir as NSString).appendingPathComponent(n)
            var isDir: ObjCBool = false
            guard fm.fileExists(atPath: p, isDirectory: &isDir) else { continue }
            let d = isDir.boolValue
            var e = Entry(name: n, path: p, isDir: d,
                          size: d ? 0 : (((try? fm.attributesOfItem(atPath: p))?[.size] as? NSNumber)?.intValue ?? 0))
            if let cached = Self.iconCache[p] {
                e.icon = cached
            } else {
                let img = NSWorkspace.shared.icon(forFile: p)
                e.icon = img
                Self.iconCache[p] = img
            }
            if !d {
                e.trailingText = Self.humanSize(e.size)
                e.trailingWidth = (e.trailingText as NSString)
                    .size(withAttributes: [.font: NSFont.systemFont(ofSize: 10)]).width
            }
            out.append(e)
        }
        out.sort {
            if $0.isDir != $1.isDir { return $0.isDir }
            return $0.name.localizedStandardCompare($1.name) == .orderedAscending
        }
        return out
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
        refilter()
    }
    private func refilter() {
        let q = query
        if q.isEmpty {
            rows = all
        } else if q.contains("*") || q.contains("?") {
            // glob matching (case-insensitive): * = any run, ? = one char.
            // A path-like glob (~/… or /…) also matches the FULL path, so
            // "/Users/me/notes/assets/img*" filters this listing like Finder.
            let nameRE = Self.globRegex(q)
            let pathRE = Self.globRegex(Self.expandTilde(q))
            rows = all.filter { e in
                Self.globMatch(nameRE, e.name) || Self.globMatch(pathRE, e.path)
            }
        } else if q.contains("/") || q.hasPrefix("~") {
            // address-bar filter: match the full path, not just the bare name
            let target = Self.expandTilde(q).lowercased()
            rows = all.filter { $0.path.lowercased().contains(target) }
        } else {
            rows = all.filter { $0.name.lowercased().contains(q.lowercased()) }
        }
        if selection >= rows.count { selection = max(0, rows.count - 1) }
        listPane.rows = rows
        listPane.selection = selection
        layoutListDocument()
        scrollListToTop()
        previewSelection()
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
    func copyRowPath(_ i: Int) {
        guard rows.indices.contains(i) else { return }
        let p = rows[i].path
        onCopyPath?(p)
        onStatus?("copied \(p)")
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

    // Windows-explorer style: when the filter bar holds something that looks
    // like a path (~/…, /…, …/…, or an existing entry) and the user hits
    // Enter, jump straight to it (cd into a dir / open a file). Returns true
    // when a jump happened, so the Enter is consumed and not treated as a
    // list selection.
    @discardableResult
    private func jumpToQueryPath() -> Bool {
        let q = searchField.stringValue.trimmingCharacters(in: .whitespaces)
        guard !q.isEmpty else { return false }
        // only a real path: absolute/~/or a slash-bearing entry (a bare name
        // like "notes" keeps filtering the current dir instead)
        guard q.contains("/") || q.hasPrefix("~") else { return false }
        let p = (q as NSString).expandingTildeInPath

        // Wildcard path (~/notes/assets/img*): switch to the longest existing
        // directory prefix, keep the glob as the active filter so the listing
        // shows every match, then open the top one (like Enter on a bare-name
        // filter opens the selection).
        if q.contains("*") || q.contains("?") {
            var dir = "/"
            var found = false
            for comp in (p as NSString).pathComponents {
                if comp == "/" { continue }
                let cand = (dir as NSString).appendingPathComponent(comp)
                var isDir: ObjCBool = false
                if FileManager.default.fileExists(atPath: cand, isDirectory: &isDir),
                   isDir.boolValue {
                    dir = cand
                    found = true
                } else {
                    break
                }
            }
            if found, dir != cwd {
                cd(dir)
                query = q
                searchField.stringValue = q
                refilter()
            }
            if let w = window { w.makeFirstResponder(listPane) }
            if let target = rows.firstIndex(where: { $0.name != ".." }) {
                openIndex(target)
            }
            return true
        }

        var isDir: ObjCBool = false
        guard FileManager.default.fileExists(atPath: p, isDirectory: &isDir) else {
            onStatus?("no such path: \(q)")
            return true
        }
        if isDir.boolValue {
            cd(p)
            if let w = window { w.makeFirstResponder(listPane) }
        } else {
            onOpen?(p)
            onStatus?("opened \(p)")
        }
        return true
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
        starButton.title = on ? "★ pinned" : "★ pin"
    }
    private func rebuildPills() {
        for p in favPills { p.removeFromSuperview() }
        favPills = []
        shownFavorites = mergedFavorites()
        for fav in shownFavorites {
            let p = ThemeButton(config: config, title: displayPath(fav))
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
        refilter()
        onStatus?(statusText())
    }
    // transient feedback for the filter bar: an exact existing path shows
    // "↵ open …" / "↵ cd …" (Enter will jump there) instead of "0 matches"
    private func statusText() -> String {
        let q = searchField.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        if !q.isEmpty {
            var isDir: ObjCBool = false
            let p = (q as NSString).expandingTildeInPath
            if FileManager.default.fileExists(atPath: p, isDirectory: &isDir) {
                return isDir.boolValue ? "↵ cd \(displayPath(p))" : "↵ open \(displayPath(p))"
            }
        }
        return query.isEmpty ? "" : "\(rows.count) match\(rows.count == 1 ? "" : "es")"
    }

    // Field-editor commands for the filter bar: the field editor owns
    // Return/Up/Down while editing, so this delegate hook (the only reliable
    // interception) routes them — Return jumps to a typed path or opens the
    // list selection; Up/Down move the list selection.
    func control(_ control: NSControl, textView: NSTextView,
                 doCommandBy commandSelector: Selector) -> Bool {
        guard control === searchField else { return false }
        switch commandSelector {
        case #selector(NSResponder.insertNewline(_:)):
            if jumpToQueryPath() { return true }
            if let w = window { w.makeFirstResponder(listPane) }
            openIndex(listPane.selection)
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
    let config: PopupConfig
    var text: String = "" { didSet { needsDisplay = true } }
    var isError = false { didSet { needsDisplay = true } }

    override var isFlipped: Bool { true }

    init(config: PopupConfig) {
        self.config = config
        super.init(frame: .zero)
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) not supported") }

    override func draw(_ dirty: NSRect) {
        let r = NSRect(x: 1, y: 1, width: bounds.width - 2, height: bounds.height - 2)
        let p = NSBezierPath(roundedRect: r, xRadius: config.buttonRadius,
                             yRadius: config.buttonRadius)
        (isError ? NSColor.systemRed.withAlphaComponent(0.2)
                 : config.colors.highlight.withAlphaComponent(0.5)).setFill()
        p.fill()
        (isError ? NSColor.systemRed.withAlphaComponent(0.8)
                 : config.colors.border.withAlphaComponent(0.4)).setStroke()
        p.lineWidth = 1
        p.stroke()
        guard !text.isEmpty else { return }
        let para = NSMutableParagraphStyle()
        para.lineBreakMode = .byTruncatingTail
        let attrs: [NSAttributedString.Key: Any] = [
            .font: NSFont.monospacedSystemFont(ofSize: 10.5, weight: .regular),
            .foregroundColor: isError
                ? NSColor.systemRed.withAlphaComponent(0.95)
                : config.colors.text,
            .paragraphStyle: para,
        ]
        let s = text as NSString
        let lineH = s.size(withAttributes: attrs).height
        s.draw(with: NSRect(x: r.minX + 10, y: r.midY - lineH / 2,
                            width: max(20, r.width - 20), height: lineH),
               options: [.usesLineFragmentOrigin], attributes: attrs)
    }
}

// MARK: - Chrome (drag + resize overlay)

// Transparent overlay above the content that owns the window chrome: resize
// edges (when enableResize) and a drag area (a top header strip for editors,
// or drag-anywhere for list windows). Non-chrome areas return nil from
// hitTest so the content below keeps its own events (text selection, typing).
final class PopupChrome: NSView {
    let config: PopupConfig
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
        if config.enableResize && !e.isEmpty { return self }
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
        if config.enableResize && !resizeEdges(at: p).isEmpty {
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
        config.colors.dim.withAlphaComponent(0.4).setStroke()
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
            ? (headerIcon != nil ? 34 : 10) + (meta.isEmpty ? 0 : metaWidth + 8)
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
        if let icon = headerIcon {
            let isz: CGFloat = 16
            popupDrawImage(icon, in: NSRect(x: 10,
                                            y: (dragHeaderHeight - isz) / 2,
                                            width: isz, height: isz))
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
        if !segs.isEmpty {
            let bp = NSBezierPath(roundedRect: barRect, xRadius: config.buttonRadius,
                                  yRadius: config.buttonRadius)
            config.colors.accent.withAlphaComponent(0.45).setFill()
            bp.fill()
            config.colors.border.withAlphaComponent(0.5).setStroke()
            bp.lineWidth = 1
            bp.stroke()
        }
        // stretch mode shares the leftover width evenly across the segments
        let perSegExtra = stretch ? max(0, barW - naturalBarW) / CGFloat(segs.count) : 0
        headerSegRects = []
        var sx = barRect.minX
        for (i, seg) in segs.enumerated() {
            let segRect = NSRect(x: sx, y: barRect.minY,
                                 width: seg.w + perSegExtra, height: barH)
            let active = feedback == seg.fb
            let persistentOn = seg.fb >= 10 && activeButtonIDs.contains(seg.fb)
            headerSegRects.append((seg.fb, segRect))
            if i > 0 {
                // thin | divider between the segments — hidden when either
                // neighbor is active so the accent fill reads as one solid
                // selected segment
                let prevActive = feedback == segs[i - 1].fb
                if !prevActive && !active {
                    config.colors.border.withAlphaComponent(0.35).setStroke()
                    let d = NSBezierPath()
                    d.lineWidth = 1
                    d.move(to: NSPoint(x: segRect.minX, y: barRect.minY + 5))
                    d.line(to: NSPoint(x: segRect.minX, y: barRect.maxY - 5))
                    d.stroke()
                }
            }
            if active {
                config.colors.accent.setFill()
                segRect.fill()
                config.colors.border.withAlphaComponent(0.9).setStroke()
                let ring = NSBezierPath(roundedRect: segRect.insetBy(dx: 1, dy: 1),
                                        xRadius: config.buttonRadius - 1,
                                        yRadius: config.buttonRadius - 1)
                ring.lineWidth = 1.5
                ring.stroke()
            } else if persistentOn {
                // persistent "on" state (drawer open): SOLID accent fill + a
                // bright outline — unmistakable against the dark idle bar
                config.colors.accent.setFill()
                segRect.fill()
                config.colors.border.withAlphaComponent(0.9).setStroke()
                let ring = NSBezierPath(roundedRect: segRect.insetBy(dx: 1, dy: 1),
                                        xRadius: config.buttonRadius - 1,
                                        yRadius: config.buttonRadius - 1)
                ring.lineWidth = 1.5
                ring.stroke()
            } else if hoveredSegment == seg.fb {
                // hover feedback: lift the segment off the bar with a brighter
                // accent fill
                let hovered = config.colors.accent.blended(withFraction: 0.25, of: config.colors.text)
                    ?? config.colors.accent.withAlphaComponent(0.7)
                hovered.setFill()
                NSBezierPath(roundedRect: segRect.insetBy(dx: 1.5, dy: 2.5),
                             xRadius: config.buttonRadius - 1.5,
                             yRadius: config.buttonRadius - 1.5).fill()
            }
            if seg.fb == 1 { copyButtonRect = segRect }
            if seg.fb == 2 { configButtonRect = segRect }
            if seg.fb == 3 { copyRowsButtonRect = segRect }
            if seg.fb >= 10 { extraButtonRects[seg.fb] = segRect }
            let lit = active || persistentOn || hoveredSegment == seg.fb
            let attrs: [NSAttributedString.Key: Any] = [
                .font: headerButtonFont(seg.text),
                .foregroundColor: lit
                    ? config.colors.text : config.colors.text.withAlphaComponent(0.72),
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
            let x0: CGFloat = headerIcon != nil ? 34 : 10
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

// Transparent view that captures resize drags on window edges/corners.
// Sits on top of all content so resize works regardless of what fills the window.
final class ResizeEdgeView: NSView {
    let cursor: NSCursor
    let edges: PopupBackdrop.Edge
    var onResize: ((NSRect, NSPoint) -> Void)?
    var onResizeDrag: (() -> Void)?
    var resizeMonitor: Any?

    init(frame: NSRect, cursor: NSCursor, edges: PopupBackdrop.Edge) {
        self.cursor = cursor
        self.edges = edges
        super.init(frame: frame)
        wantsLayer = true
        layer?.backgroundColor = NSColor.clear.cgColor
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) not supported") }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        trackingAreas.forEach { removeTrackingArea($0) }
        addTrackingArea(NSTrackingArea(rect: bounds, options: [.activeAlways, .mouseMoved], owner: self))
    }
    override func mouseMoved(with event: NSEvent) { cursor.set() }
    override func mouseDown(with event: NSEvent) {
        onResize?(window?.frame ?? .zero, NSEvent.mouseLocation)
        // capture ALL mouse events globally until mouse up so drag works
        // even when the cursor leaves the narrow edge strip
        resizeMonitor = NSEvent.addLocalMonitorForEvents(matching: [.leftMouseDragged, .leftMouseUp], handler: { [weak self] ev in
            guard let self else { return ev }
            if ev.type == .leftMouseDragged {
                self.onResizeDrag?()
                return nil  // consume
            }
            // mouse up: remove the monitor
            if let m = self.resizeMonitor { NSEvent.removeMonitor(m) }
            self.resizeMonitor = nil
            return ev
        })
    }
    override func mouseDragged(with event: NSEvent) { onResizeDrag?() }
    override func viewWillMove(toWindow newWindow: NSWindow?) {
        if let m = resizeMonitor { NSEvent.removeMonitor(m) }
        resizeMonitor = nil
    }
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
    // fired when the terminal process exits (e.g. Vim quits). The exit code
    // is passed along; hosts use this to close the window or relaunch.
    public var onTerminalExit: ((Int32?) -> Void)?
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
        guard config.selectableRows else {
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
    private let focusBorderColor = NSColor(srgbRed: 100/255, green: 180/255, blue: 255/255, alpha: 1.0)
    private let focusBorderWidth: CGFloat = 3

    // total drawer height currently folded into the window frame (baseline =
    // no drawers); the terminal is on at init when config.terminal is set
    private var drawerInsetNow: CGFloat = 0
    // track window height during resize so we know if it shrank/grew
    private var lastResizeHeight: CGFloat?
    // current drawer heights (may differ from config during resize)
    private var currentTerminalHeight: CGFloat = 0
    private var currentBrowserHeight: CGFloat = 0
    private let minTerminalH: CGFloat = 40
    private let minBrowserH: CGFloat = 50
    private var editorScroll: NSScrollView?
    private var tabsBar: PopupTabsBar?
    private var filterBar: PopupFilterBar?
    // close button target (retained so it survives menu/window dismiss)
    private var windowCloseTarget: WindowCloseTarget?
    // retained restarter so we can wire onTerminated after super.init
    private var terminalRestarter: TerminalAutoRestart?
    // whether a custom terminal executable is set (captured before super.init,
    // used to skip auto-restart and fire onTerminalExit instead)
    private var isCustomExecForExit = false
    private var rowScroll: NSScrollView?
    private var chrome: PopupChrome?
    // transparent resize edge views that sit ON TOP of all content so drag
    // resize works even when the editor/terminal/browser fills the window
    private var resizeEdgeViews: [NSView] = []
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
                tv.font = editorFont(config.fontName, zoom)
            }
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
        } else if config.scrollableRows {
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
            panel = PopupPlainWindow(
                contentRect: NSRect(x: 0, y: 0, width: config.width, height: initialHeight),
                styleMask: [.titled, .closable, .fullSizeContentView],
                backing: .buffered, defer: false)
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
        panel.level = .popUpMenu
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
            field.cell = PopupSearchFieldCell()
            field.isEditable = config.enableSearch
            field.isSelectable = config.enableSearch
            // the cell swap also resets focusRingType — kill the ring AGAIN
            // or the loud blue macOS ring returns on top of our hairline
            field.focusRingType = .none
            field.wantsLayer = true
            field.layer?.backgroundColor = config.colors.highlight.withAlphaComponent(0.4).cgColor
            field.layer?.cornerRadius = 6
            // themed hairline instead of the system focus ring (killed below
            // in controlTextDidBeginEditing) — the blue macOS ring reads as
            // an error state on the dark bar
            field.layer?.borderWidth = 1
            field.layer?.borderColor =
                config.colors.border.withAlphaComponent(0.45).cgColor
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
            tv.font = editorFont(config.fontName, zoom)
            tv.textColor = config.colors.text
            // Force the selection highlight colors (Ctrl+A select-all, the
            // find bar's match jump). The system default follows the OS
            // appearance/accent, so the SAME binary renders differently per
            // machine — e.g. white text on a light-mode selection is
            // unreadable. Pin selection to the configured highlight + text
            // colors so it always contrasts, regardless of the machine.
            tv.selectedTextAttributes = [
                .backgroundColor: config.colors.highlight,
                .foregroundColor: config.colors.text,
            ]
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
            efb.layer?.borderColor = focusBorderColor.cgColor
            efb.layer?.cornerRadius = 6
            efb.isHidden = true
            backdrop.addSubview(efb)
            editorFocusBorder = efb
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
            ff.cell = PopupSearchFieldCell()
            ff.isEditable = true
            ff.isSelectable = true
            ff.focusRingType = .none
            ff.wantsLayer = true
            ff.layer?.backgroundColor = config.colors.highlight.withAlphaComponent(0.4).cgColor
            ff.layer?.cornerRadius = 6
            ff.layer?.borderWidth = 1
            ff.layer?.borderColor = config.colors.border.withAlphaComponent(0.45).cgColor
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
                if let tf = NSFont(name: config.terminalFont, size: 13) {
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
                term.nativeForegroundColor = config.colors.text
                backdrop.addSubview(term)
                terminalDrawer = term
                currentTerminalHeight = config.terminalHeight
                // focus indicator: bright 4-sided border around the terminal
                let tfb = NSView(frame: term.frame)
                tfb.autoresizingMask = [.width]
                tfb.wantsLayer = true
                tfb.layer?.borderWidth = focusBorderWidth
                tfb.layer?.borderColor = focusBorderColor.cgColor
                tfb.layer?.cornerRadius = 8
                tfb.isHidden = true
                backdrop.addSubview(tfb)
                terminalFocusBorder = tfb
                // auto-restart: if the shell exits (user typed exit/ctrl-d)
                // spawn it again after a beat so the drawer is never dead
                // (capture the shell locally — no self before super.init)
                // When a custom terminalExecutable is set (e.g. nvim for vim
                // mode), skip auto-restart and fire onTerminalExit instead.
                let exec = config.terminalExecutable ?? config.shell
                let execArgs = config.terminalExecArgs.isEmpty ? config.shellArgs : config.terminalExecArgs
                let terminalDir = config.terminalDir
                isCustomExecForExit = config.terminalExecutable != nil
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
                // Skip auto-restart for custom executables (e.g. Vim) — those
                // are managed by the host via onTerminalExit.
                let customExec = isCustomExecForExit  // capture before super.init
                let cfgShell = config.shell
                let cfgShellArgs = config.shellArgs
                let cfgTerminalDir = config.terminalDir
                let poll = Timer(timeInterval: 1.5, repeats: true) { [weak term] _ in
                    guard let term, let p = term.process else { return }
                    if !p.running, !p.windingDown, !customExec {
                        term.startProcess(executable: cfgShell, args: cfgShellArgs,
                                          currentDirectory: cfgTerminalDir)
                    }
                }
                RunLoop.main.add(poll, forMode: .common)
                terminalRestartTimer = poll
            }
        } else {
            // search list: chrome (drag header, search field, filter bar, tab
            // strip) fixed on top; rows below — directly (default) or in a
            // scroll view. Non-scroll keeps chrome as rowView subviews so
            // clicks reach them; scroll puts them on the backdrop above the
            // scroll view.
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
                // + breathing room below the last row (the scroll view now
                // ends exactly at the window bottom, so no huge inset needed)
scroll.documentView = rowView
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

        // Wire the terminal restarter (self-safe now): fire onTerminalExit for
        // custom executables (Vim), auto-restart for the shell.
        if let restarter = terminalRestarter {
            let terminalDir = config.terminalDir
            restarter.onTerminated = { [weak self] in
                guard let self else { return }
                if isCustomExecForExit {
                    onTerminalExit?(nil)
                } else {
                    terminalDrawer?.startProcess(executable: config.shell, args: config.shellArgs,
                                                 currentDirectory: terminalDir)
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
                if let chrome = self.chrome,
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
                   chrome.headerIcon != nil, p.x <= 36 {
                    // click on the top-left app glyph (drawn at x=10..26) —
                    // the host e.g. opens the config file in the viewer
                    self.onChromeIconClick?()
                }
            }
        }
        // transparent resize edge views: sit on top of ALL content so drag
        // resize works even when editor/terminal/browser fills the window
        if config.enableResize {
            installResizeEdges()
        }
        panel.orderOut(nil)
    }

    // Create transparent edge/corner views that capture resize drags.
    // These sit on top of all content (editor, terminal, browser) so the
    // user can grab the window border regardless of what's underneath.
    private let resizeHitSize: CGFloat = 16
    private var resizeDragEdges: PopupBackdrop.Edge = []
    private var resizeStartFrame: NSRect = .zero
    private var resizeStartPoint: NSPoint = .zero

    private func installResizeEdges() {
        guard let backdrop = panel.contentView else { return }
        for edgeView in resizeEdgeViews { edgeView.removeFromSuperview() }
        resizeEdgeViews = []

        let hit = resizeHitSize
        let w = backdrop.bounds.width
        let h = backdrop.bounds.height

        // top edge
        addResizeEdge(NSRect(x: hit, y: h - hit, width: w - 2 * hit, height: hit), cursor: .resizeUpDown, edges: [.top])
        // bottom edge
        addResizeEdge(NSRect(x: hit, y: 0, width: w - 2 * hit, height: hit), cursor: .resizeUpDown, edges: [.bottom])
        // left edge
        addResizeEdge(NSRect(x: 0, y: hit, width: hit, height: h - 2 * hit), cursor: .resizeLeftRight, edges: [.left])
        // right edge
        addResizeEdge(NSRect(x: w - hit, y: hit, width: hit, height: h - 2 * hit), cursor: .resizeLeftRight, edges: [.right])
        // corners
        addResizeEdge(NSRect(x: 0, y: h - hit, width: hit, height: hit), cursor: .resizeLeftRight, edges: [.top, .left])
        addResizeEdge(NSRect(x: w - hit, y: h - hit, width: hit, height: hit), cursor: .resizeLeftRight, edges: [.top, .right])
        addResizeEdge(NSRect(x: 0, y: 0, width: hit, height: hit), cursor: .resizeLeftRight, edges: [.bottom, .left])
        addResizeEdge(NSRect(x: w - hit, y: 0, width: hit, height: hit), cursor: .resizeLeftRight, edges: [.bottom, .right])
    }

    private func addResizeEdge(_ frame: NSRect, cursor: NSCursor, edges: PopupBackdrop.Edge) {
        let v = ResizeEdgeView(frame: frame, cursor: cursor, edges: edges)
        v.onResize = { [weak self] startFrame, startPoint in
            guard let self else { return }
            self.resizeDragEdges = edges
            self.resizeStartFrame = startFrame
            self.resizeStartPoint = startPoint
        }
        v.onResizeDrag = { [weak self] in
            self?.performResizeDrag()
        }
        if let backdrop = panel.contentView {
            backdrop.addSubview(v)
        }
        resizeEdgeViews.append(v)
    }

    private func performResizeDrag() {
        let m = NSEvent.mouseLocation
        let dx = m.x - resizeStartPoint.x
        let dy = m.y - resizeStartPoint.y
        var f = resizeStartFrame
        var w = f.width
        var h = f.height
        if resizeDragEdges.contains(.right) { w += dx }
        if resizeDragEdges.contains(.left) { w -= dx; f.origin.x += dx }
        if resizeDragEdges.contains(.top) { h += dy; f.origin.y -= dy }
        if resizeDragEdges.contains(.bottom) { h -= dy }
        w = max(120, w)
        h = max(140, h)
        f.size.width = w
        f.size.height = h
        panel.setFrame(clampToScreen(f), display: true)
        zoom = min(3, max(0.6, w / config.width))
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
            // initial focus: editor gets the highlight by default
            if let ed = editorView {
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
        // icon sits at x=10, y=top of window; pop down 4pts below the header
        let pt = NSPoint(x: 10, y: panel.frame.height - config.headerHeight * zoom - 4)
        let screenPt = panel.convertPoint(toScreen: pt)
        menu.popUp(positioning: nil, at: screenPt, in: nil)
        isShowingMenu = false
    }

    // Reset the window back to its configured default size and re-layout
    // all sub-panes (editor, terminal, browser). Called from the app menu.
    public func resetToDefaultSize() {
        let h = min(config.height, maxPanelHeight())
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
        let font = editorFont(config.fontName, zoom)
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
            .font: editorFont(config.fontName, zoom),
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
        let font = editorFont(config.fontName, config.zoom)
        setEditorAttributedText(parseANSI(s, baseFont: font, defaultColor: config.colors.text))
    }

    // prettyprint-style syntax highlighting: re-render `text` with JSON/XML
    // token colors using the editor's own font. Fixes the typing attributes so
    // edits after highlighting keep the theme instead of snapping to black.
    public func setEditorSyntaxHighlighted(_ text: String) {
        let font = editorFont(config.fontName, zoom)
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
        if let tv = editorView {
            panel.makeFirstResponder(tv)
        } else {
            panel.makeFirstResponder(field)
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
                if let tv = self.editorView {
                    self.panel.makeFirstResponder(tv)
                } else {
                    self.panel.makeFirstResponder(self.field)
                }
            }
        }
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
        if let ed = editorView, fr === ed || (fr as? NSTextView)?.isDescendant(of: ed) == true {
            focusedPane = .editor
        } else if fileBrowserShown, let fb = fileBrowser {
            let lv = fb.listView
            if fr === lv || (fr as? NSView)?.isDescendant(of: lv) == true {
                focusedPane = .browser
            }
        } else if terminalShown, let term = terminalDrawer,
                  (fr === term || (fr as? NSView)?.isDescendant(of: term) == true) {
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
        // Cmd + plus/minus (main "="/"+" and "-", plus the keypad): grow or
        // shrink the window; rows stretch to fill from then on
        let cmd = mods.contains(.command)
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
                if let ed = editorView { panes.append(ed); paneTypes.append(.editor) }
                if fileBrowserShown, let fb = fileBrowser { panes.append(fb.listView); paneTypes.append(.browser) }
                if terminalShown, let term = terminalDrawer { panes.append(term); paneTypes.append(.terminal) }
                if panes.count > 1 {
                    let current = panel.firstResponder
                    let curIdx = panes.firstIndex { p in
                        if let current, current === p { return true }
                        if let v = current as? NSView, let pv = p as? NSView {
                            return v.isDescendant(of: pv)
                        }
                        return false
                    }
                    if let curIdx {
                        let target = code == 38
                            ? min(curIdx + 1, panes.count - 1)
                            : max(curIdx - 1, 0)
                        if target != curIdx { panel.makeFirstResponder(panes[target]) }
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
                    } else if fileBrowserShown && focusedPane == .browser {
                        currentBrowserHeight = min(600, currentBrowserHeight + step)
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
                    } else if fileBrowserShown && focusedPane == .browser {
                        currentBrowserHeight = max(minBrowserH, currentBrowserHeight - step)
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
            if let term = focusedTerm() {
                switch code {
                case 8 where cmd: term.copy(self); return true    // Cmd+C copy
                case 9 where cmd || ctrl: term.paste(self); return true  // Cmd+V / Ctrl+V paste
                default: return false   // every other Cmd/Ctrl key goes to the shell
                }
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
                handleEscape()
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
            handleEscape()
            return true
        }
        return false
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
        if isShown && !config.sticky && settings.hideOnFocusLoss {
            hide(restore: false)
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
        if isShown {
            panel.setFrame(clampToScreen(panel.frame), display: true)
        }
        // smart drawer resize: if the window shrank, reduce drawers before
        // touching the editor
        let newH = panel.frame.height
        if let prevH = lastResizeHeight {
            let delta = newH - prevH
            if delta < 0 {
                // shrinking: take space from drawers first
                if terminalShown && currentTerminalHeight > minTerminalH {
                    let take = min(-delta, currentTerminalHeight - minTerminalH)
                    currentTerminalHeight -= take
                }
                let used = min(-delta, terminalShown ? currentTerminalHeight - minTerminalH + (currentTerminalHeight > minTerminalH ? 0 : 0) : 0)
                let remaining = -delta - used
                if remaining > 0 && fileBrowserShown && currentBrowserHeight > minBrowserH {
                    let take = min(remaining, currentBrowserHeight - minBrowserH)
                    currentBrowserHeight -= take
                }
            } else if delta > 0 {
                // growing: restore drawers toward config height
                if terminalShown && currentTerminalHeight < config.terminalHeight {
                    let give = min(delta, config.terminalHeight - currentTerminalHeight)
                    currentTerminalHeight += give
                }
                if fileBrowserShown && currentBrowserHeight < config.fileBrowserHeight {
                    let remaining = delta - (terminalShown ? min(delta, config.terminalHeight - currentTerminalHeight) : 0)
                    if remaining > 0 {
                        let give = min(remaining, config.fileBrowserHeight - currentBrowserHeight)
                        currentBrowserHeight += give
                    }
                }
            }
        } else {
            // first resize event: initialize from config
            currentTerminalHeight = config.terminalHeight
            currentBrowserHeight = config.fileBrowserHeight
        }
        lastResizeHeight = newH
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
        rowView.frame.size.height = scrollDocumentHeight()
    }

    // Tab bar height is dynamic: many tabs WRAP to extra rows instead of
    // hiding. After titles change (or the window resizes), re-measure and
    // shift the content below so it never overlaps the wrapped pills.
    private func relayoutTabs() {
        guard let bar = tabsBar else { return }
        let w = bar.bounds.width > 0 ? bar.bounds.width : config.width
        let h = bar.heightNeeded(forWidth: w)
        if abs(bar.frame.height - h) > 0.5 {
            bar.frame.size.height = h
            bar.needsDisplay = true
            if config.editMode {
                layoutEditorScroll()
            } else if config.scrollableRows, let scroll = rowScroll,
                      let backdrop = panel.contentView {
                let base = self.chromeBottom - (config.tabBarHeight * zoom + 2)
                let cb = base + h
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
        syncDrawerLayout()
        if terminalShown {
            panel.makeFirstResponder(drawer)
            focusedPane = .terminal
        } else if let ed = editorView {
            panel.makeFirstResponder(ed)
            focusedPane = .editor
        }
        updateFocusIndicator()
    }

    // Send raw text to the terminal's stdin (e.g. ":wq\r" to save and quit Vim).
    // Returns true if the terminal is available and the text was sent.
    public func sendToTerminal(_ text: String) -> Bool {
        guard let drawer = terminalDrawer, terminalShown,
              let process = drawer.process, process.running else { return false }
        drawer.send(txt: text)
        return true
    }

    // Whether this window is running a custom terminal executable (e.g. Vim).
    public var isCustomTerminal: Bool { config.terminalExecutable != nil }

    // Relaunch the terminal process with the current config executable/args.
    // Used when switching notes in vim mode — the Vim process exited, and we
    // need to start a fresh one with a different file argument.
    public func relaunchTerminal() {
        guard let drawer = terminalDrawer else { return }
        let exec = config.terminalExecutable ?? config.shell
        let args = config.terminalExecArgs.isEmpty ? config.shellArgs : config.terminalExecArgs
        drawer.startProcess(executable: exec, args: args,
                            currentDirectory: config.terminalDir)
        if terminalShown {
            panel.makeFirstResponder(drawer)
        }
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
                // terminal text color stays the theme's fixed color
                term.nativeForegroundColor = config.colors.text
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
            // keep the drag-header fill consistent when it falls back to the
            // card background
            chrome?.headerColorOverride = config.headerColor ?? config.colors.background
        case .header:
            config.headerColor = c
            chrome?.headerColorOverride = c
        }
        panel.contentView?.needsDisplay = true
    }

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
        currentBrowserHeight = config.fileBrowserHeight
        // right-click "Open in Notes" -> host hook
        fb.onOpenInNotes = { [weak self] p in
            self?.onFileBrowserOpenInNotes?(p)
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
        bfb.layer?.borderColor = focusBorderColor.cgColor
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
            currentBrowserHeight = config.fileBrowserHeight
        }
        syncDrawerLayout()
        if fileBrowserShown, let lp = fileBrowser?.listView {
            panel.makeFirstResponder(lp)
            focusedPane = .browser
        } else if let ed = editorView {
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
