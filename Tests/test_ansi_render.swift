// sources: AnsiRender.swift PaneShot.swift PythonHelper.swift ConfigText.swift ProcessRun.swift
import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

var passed = 0
var failed = 0

func check(_ condition: Bool, _ message: String, line: Int = #line) {
    if condition {
        passed += 1
    } else {
        failed += 1
        print("  FAIL (line \(line)): \(message)")
    }
}

let esc = "\u{1B}"

@main
struct AnsiRenderTests {
    static func main() {
        sgrTests()
        gridTests()
        widthTests()
        themeTests()
        renderTests()
        argTests()
        configTests()
        herdrTests()
        renderFileForALook()
        print("ansi: \(passed) passed, \(failed) failed")
        exit(failed == 0 ? 0 : 1)
    }

    static func style(_ params: String, from s: AnsiStyle = AnsiStyle()) -> AnsiStyle {
        var x = s
        AnsiGrid.applySGR(params, &x)
        return x
    }

    static func sgrTests() {
        check(style("38;5;12").fg == .index(12), "256-color fg")
        check(style("48;5;8").bg == .index(8), "256-color bg")
        check(style("38;2;10;20;30").fg == .rgb(AnsiRGB(10, 20, 30)), "truecolor fg")
        check(style("38:2:10:20:30").fg == .rgb(AnsiRGB(10, 20, 30)), "colon-separated truecolor")
        check(style("31").fg == .index(1) && style("94").fg == .index(12), "basic + bright fg")
        check(style("41").bg == .index(1) && style("103").bg == .index(11), "basic + bright bg")
        let all = style("1;2;3;4;7;9")
        check(all.bold && all.dim && all.italic && all.underline && all.inverse && all.strike, "attributes on")
        check(style("0", from: all) == AnsiStyle(), "0 resets")
        check(style("", from: all) == AnsiStyle(), "empty = reset")
        let off = style("22;23;24;27;29", from: all)
        check(!off.bold && !off.dim && !off.italic && !off.underline && !off.inverse && !off.strike, "attributes off")
        check(style("39", from: style("31")).fg == .none, "39 = default fg")
        check(style("1;38;5;200;4").fg == .index(200) && style("1;38;5;200;4").underline, "params after an extended color")
        check(style("38;5").fg == .none, "truncated extended color ignored")
    }

    static func text(_ row: [AnsiCell]) -> String { row.map(\.text).joined() }

    static func gridTests() {
        let g = AnsiGrid.parse("\(esc)[0m\(esc)[38;5;7mab\(esc)[0m\r\n\r\ncd\(esc)[1me\r\n")
        check(g.rows.count == 3, "rows: \(g.rows.count)")
        check(text(g.rows[0]) == "ab" && g.rows[0][0].style.fg == .index(7), "styled row")
        check(g.rows[1].isEmpty, "empty row kept")
        check(g.rows[2][2].style.bold && !g.rows[2][1].style.bold, "style starts mid-row")
        check(g.columns == 3, "columns = widest row")

        let trail = AnsiGrid.parse("x\n   \n\n")
        check(trail.rows.count == 1, "trailing blank rows trimmed")
        let kept = AnsiGrid.parse("x\n\(esc)[41m  \(esc)[0m\n")
        check(kept.rows.count == 2, "a blank row with a background is content")

        let cr = AnsiGrid.parse("hello\rJ\n")
        check(text(cr.rows[0]) == "Jello", "lone CR overwrites from column 0")
        let tab = AnsiGrid.parse("a\tb")
        check(text(tab.rows[0]) == "a       b", "tab to column 8")
        let osc = AnsiGrid.parse("\(esc)]8;;https://x.y\u{07}link\(esc)]8;;\(esc)\\ done")
        check(text(osc.rows[0]) == "link done", "OSC 8 links dropped")
        let csi = AnsiGrid.parse("a\(esc)[2Kb\(esc)[?25lc\(esc)(Bd")
        check(text(csi.rows[0]) == "abcd", "other CSI / charset escapes dropped")
        let ctl = AnsiGrid.parse("a\u{07}b\u{08}c")
        check(text(ctl.rows[0]) == "abc", "control characters dropped")
    }

    static func widthTests() {
        check(AnsiGrid.cellWidth("a") == 1 && AnsiGrid.cellWidth("─") == 1, "narrow")
        check(AnsiGrid.cellWidth("中") == 2 && AnsiGrid.cellWidth("한") == 2, "CJK wide")
        check(AnsiGrid.cellWidth("😀") == 2, "emoji wide")
        check(AnsiGrid.cellWidth("❤\u{FE0F}") == 2, "VS16 makes it wide")
        check(AnsiGrid.cellWidth("\u{F121}") == 1, "nerd font icon (private use) narrow")
        let g = AnsiGrid.parse("中x")
        check(g.rows[0].count == 3 && g.rows[0][1].text.isEmpty && g.rows[0][2].text == "x", "wide cell + its right half")
        check(AnsiGrid.parse("e\u{301}x").rows[0].count == 2, "combining mark rides in its grapheme")
    }

    static func themeTests() {
        let x = AnsiTheme.xterm256()
        check(x.count == 256, "256 colors")
        check(x[16] == AnsiRGB(0, 0, 0) && x[21] == AnsiRGB(0, 0, 255) && x[196] == AnsiRGB(255, 0, 0), "cube")
        check(x[232] == AnsiRGB(8, 8, 8) && x[255] == AnsiRGB(238, 238, 238), "grays")

        let t = AnsiTheme.ghostty("""
        font-family = JetBrainsMono Nerd Font
        font-family = Fallback Font
        font-size = 15
        background = #1a1b26
        foreground = #c0caf5
        palette = 4=#7aa2f7
        palette = 200=#123456
        bold-is-bright = true
        window-colorspace = display-p3
        junk line
        """)
        check(t.fontName == "JetBrainsMono Nerd Font", "first font-family wins: \(t.fontName)")
        check(t.fontSize == 15, "font-size")
        check(t.background == AnsiRGB(0x1a, 0x1b, 0x26) && t.foreground == AnsiRGB(0xc0, 0xca, 0xf5), "fg / bg")
        check(t.palette[4] == AnsiRGB(0x7a, 0xa2, 0xf7) && t.palette[200] == AnsiRGB(0x12, 0x34, 0x56), "palette")
        check(t.boldIsBright && t.displayP3, "bold-is-bright + colorspace")

        var s = AnsiStyle()
        check(t.colors(s).fg == t.foreground && t.colors(s).bg == nil, "defaults")
        s.fg = .index(1); s.bold = true
        check(t.colors(s).fg == t.palette[9], "bold-is-bright")
        var inv = AnsiStyle(); inv.inverse = true
        check(t.colors(inv).fg == t.background && t.colors(inv).bg == t.foreground, "inverse swaps")
    }

    static func renderTests() {
        var t = AnsiTheme()
        t.fontName = "Menlo"
        let f = AnsiRender.font(t)
        let m = AnsiRender.metrics(f)
        check(m.cellWidth > 5 && m.cellWidth < 10 && m.lineHeight > 12 && m.lineHeight < 20, "Menlo 13 metrics: \(m)")
        t.fontName = "No Such Font 123"
        check(AnsiRender.metrics(AnsiRender.font(t)) == m, "unknown font → Menlo")

        let g = AnsiGrid.parse("ab\ncde\n")
        let sz = AnsiRender.size(g, m, padding: 10)
        check(sz.height == 2 * m.lineHeight + 20, "height = rows × line + padding")
        check(sz.width == ceil(3 * m.cellWidth + 20), "width = columns × cell + padding")
        check(AnsiRender.scale(for: CGSize(width: 800, height: 15000), wanted: 2) == 2, "2× when it fits")
        check(AnsiRender.scale(for: CGSize(width: 800, height: 17000), wanted: 2) == 1, "1× when huge")

        t.fontName = "Menlo"
        guard let img = AnsiRender.image(AnsiGrid.parse("\(esc)[41mX\(esc)[0m hi 中😀"), theme: t, padding: 4, scale: 2) else {
            check(false, "image rendered"); return
        }
        let pts = AnsiRender.size(AnsiGrid.parse("X hi 中😀"), m, padding: 4)
        check(img.width == Int(pts.width * 2) && img.height == Int(pts.height * 2), "pixel size = points × scale")
        if let px = pixel(img, x: Int((4 + m.cellWidth / 2) * 2), y: Int((4 + 1) * 2)) {
            check(px.r > 150 && px.g < 80, "red bg cell: \(px)")
        }
        if let px = pixel(img, x: 2, y: 2) {
            check(px == (t.background.r, t.background.g, t.background.b) || abs(Int(px.r) - Int(t.background.r)) < 3, "padding = theme background: \(px)")
        }
        if let img2 = AnsiRender.image(AnsiGrid.parse("⏺ \(esc)[31m█\n"), theme: t, padding: 0, scale: 1),
           let px = pixel(img2, x: Int(m.cellWidth * 2.5), y: Int(m.lineHeight / 2)) {
            check(px.r > 150 && px.g < 80, "glyph after a fallback stays in its cell: \(px)")
        } else { check(false, "fallback row rendered") }
        _ = AnsiRender.image(AnsiGrid.parse(""), theme: t)
    }

    static func pixel(_ img: CGImage, x: Int, y: Int) -> (r: UInt8, g: UInt8, b: UInt8)? {
        var buf = [UInt8](repeating: 0, count: 4)
        guard let ctx = CGContext(data: &buf, width: 1, height: 1, bitsPerComponent: 8, bytesPerRow: 4,
                                  space: CGColorSpace(name: CGColorSpace.sRGB)!,
                                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return nil }
        ctx.draw(img, in: CGRect(x: -x, y: y - img.height + 1, width: img.width, height: img.height))
        return (buf[0], buf[1], buf[2])
    }

    static func argTests() {
        func p(_ s: String) -> PaneShotArgs? { try? PaneShotArgs.parse(s.split(separator: " ").map(String.init)).get() }
        check(p("") == PaneShotArgs(), "no args")
        check(p("--pane wB:p1 --lines 50")?.pane == "wB:p1" && p("--lines 50")?.lines == 50, "pane + lines")
        check(p("-n all")?.all == true, "lines all")
        check(p("--file - --no-save --no-copy") == PaneShotArgs(file: "-", save: false, copy: false), "file + switches")
        check(p("--lines x") == nil && p("--lines") == nil && p("--bogus") == nil && p("--lines -3") == nil, "bad args refused")
    }

    static func configTests() {
        let d = PaneShotConfig([:])
        check(d.lines == 200 && d.save && d.copy && d.font.isEmpty && d.fontSize == 0, "defaults")
        let c = PaneShotConfig(["lines": "5000", "save": "false", "font-size": "14", "padding": "-3", "herdr-bin": "/x/herdr"])
        check(c.lines == Herdr.maxLines, "lines clamped to herdr's cap")
        check(!c.save && c.fontSize == 14 && c.padding == 0 && c.herdrBin == "/x/herdr", "values")
    }

    static func herdrTests() {
        let ok = """
        {"id":"cli:pane:current","result":{"pane":{"pane_id":"wB:p1J","scroll":{"viewport_rows":60},"terminal_title_stripped":"build things","agent":"claude"},"type":"pane_current"}}
        """
        check((try? Herdr.pane(fromJSON: ok).get()) == Herdr.Pane(id: "wB:p1J", title: "build things", viewportRows: 60), "pane parsed")
        let bare = #"{"result":{"pane":{"pane_id":"wB:p2","scroll":{"viewport_rows":26}}}}"#
        check((try? Herdr.pane(fromJSON: bare).get())?.title == "wB:p2", "no title → id")
        let err = #"{"id":"x","error":{"code":"server_not_running","message":"no herdr server is running"}}"#
        if case .failure(let f) = Herdr.pane(fromJSON: err) {
            check(f.message == "no herdr server is running", "error message")
        } else { check(false, "error → failure") }
        check(Herdr.lines(viewport: 60, history: 200, all: false) == 260, "screen + history")
        check(Herdr.lines(viewport: 60, history: 5000, all: false) == Herdr.maxLines, "capped")
        check(Herdr.lines(viewport: 60, history: 0, all: true) == Herdr.maxLines, "all")
        let env = Herdr.environment(["HERDR_PANE_ID": "a", "HERDR_TAB_ID": "b", "HERDR_SOCKET_PATH": "/s", "HOME": "/h"])
        check(env == ["HERDR_SOCKET_PATH": "/s", "HOME": "/h"], "pane env dropped, socket kept")
        let fake = NSTemporaryDirectory() + "fake-herdr-\(getpid())"
        try? "#!/bin/sh\necho '{\"error\":{\"code\":\"pane_not_found\",\"message\":\"pane x not found\"}}' >&2\nexit 1\n"
            .write(toFile: fake, atomically: true, encoding: .utf8)
        chmod(fake, 0o755)
        if case .failure(let f) = Herdr.pane(fake, id: "x") {
            check(f.message == "pane x not found", "herdr's stderr error surfaces: \(f.message)")
        } else { check(false, "herdr error → failure") }
        try? FileManager.default.removeItem(atPath: fake)
        if case .failure(let f) = Herdr.run("/no/such/herdr", ["pane", "current"]) {
            check(f.message.contains("herdr-bin"), "missing binary names the config key")
        } else { check(false, "missing binary fails") }
    }

    static func renderFileForALook() {
        let env = ProcessInfo.processInfo.environment
        guard let file = env["WS_ANSI_FILE"], let out = env["WS_ANSI_PNG"],
              let text = try? String(contentsOfFile: file, encoding: .utf8) else { return }
        let cfg = (try? runProcess("/Applications/Ghostty.app/Contents/MacOS/ghostty", ["+show-config"]))?.out ?? ""
        let start = Date()
        let grid = AnsiGrid.parse(text)
        guard let img = AnsiRender.image(grid, theme: AnsiTheme.ghostty(cfg)),
              let dest = CGImageDestinationCreateWithURL(URL(fileURLWithPath: out) as CFURL, UTType.png.identifier as CFString, 1, nil) else {
            print("  render failed"); return
        }
        CGImageDestinationAddImage(dest, img, nil)
        CGImageDestinationFinalize(dest)
        print("  rendered \(grid.rows.count) rows × \(grid.columns) cols → \(img.width)×\(img.height) px in \(Int(Date().timeIntervalSince(start) * 1000)) ms: \(out)")
    }
}
