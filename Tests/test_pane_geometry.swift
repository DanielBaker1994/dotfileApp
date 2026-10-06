// sources: PaneGeometry.swift
// Ctrl+H / J / K / L pane navigation (PaneGeometry.swift): every jump on
// the layouts of the pane maps (notes with both drawers, files, jira list
// with the issue panel, compare text, AI), the way back retracing the way
// in, and no wrapping at the edges.
// Usage: bin/run-tests.sh panes

import Foundation

@main
struct PaneGeometryTests {
    static var passed = 0
    static var failed = 0

    static func check(_ condition: Bool, _ message: String, line: Int = #line) {
        if condition {
            passed += 1
        } else {
            failed += 1
            print("  FAIL: \(message) (test_pane_geometry.swift:\(line))")
        }
    }

    // percent boxes like the maps (x, y, w, h), top-down
    static func pane(_ id: String, _ x: CGFloat, _ y: CGFloat, _ w: CGFloat, _ h: CGFloat) -> PaneRect {
        PaneRect(id: id, rect: CGRect(x: x * 16, y: y * 10, width: w * 16, height: h * 10))
    }

    // walk a path of moves, remembering like PaneNav does; returns where it ended
    static func walk(_ start: String, _ keys: String, _ panes: [PaneRect]) -> [String] {
        var came: [String: [PaneDir: String]] = [:]
        var at = start
        var trail: [String] = []
        for k in keys {
            guard let d = PaneDir(rawValue: String(k)) else { continue }
            if let to = PaneGeometry.next(from: at, d, panes: panes, came: came) {
                PaneGeometry.remember(&came, from: at, to: to, d)
                at = to
            }
            trail.append(at)
        }
        return trail
    }

    static func expect(_ panes: [PaneRect], _ from: String, _ d: PaneDir, _ to: String?, line: Int = #line) {
        let got = PaneGeometry.next(from: from, d, panes: panes)
        check(got == to, "\(from) \(d.rawValue) → \(to ?? "nil"), got \(got ?? "nil")", line: line)
    }

    static func main() {
        // notes: sidebar left, editor, files drawer (list | preview), terminal
        let notes = [
            pane("sidebar", 1, 9, 19, 90),
            pane("editor", 21, 9, 78, 44),
            pane("list", 21, 55, 41, 20),
            pane("preview", 63, 55, 36, 20),
            pane("terminal", 21, 77, 78, 22),
        ]
        expect(notes, "editor", .left, "sidebar")
        expect(notes, "editor", .right, nil)
        expect(notes, "editor", .up, nil)
        expect(notes, "editor", .down, "list")          // bigger overlap than the preview
        expect(notes, "list", .right, "preview")
        expect(notes, "preview", .left, "list")
        expect(notes, "list", .down, "terminal")
        expect(notes, "preview", .down, "terminal")
        expect(notes, "terminal", .up, "list")
        expect(notes, "sidebar", .right, "editor")       // nearest gap, then largest overlap
        expect(notes, "terminal", .left, "sidebar")
        expect(notes, "sidebar", .left, nil)             // no wrap
        // the way back: sidebar → l comes back to where h left from
        check(walk("terminal", "hl", notes) == ["sidebar", "terminal"], "terminal h l returns to terminal")
        check(walk("preview", "kj", notes) == ["editor", "preview"], "preview k j returns to the preview")

        // files: sidebar, address bar on top, list | preview
        let files = [
            pane("sidebar", 1, 9, 19, 90),
            pane("filter", 21, 9, 78, 6),
            pane("list", 21, 17, 45, 82),
            pane("preview", 67, 17, 32, 82),
        ]
        expect(files, "list", .up, "filter")
        expect(files, "preview", .up, "filter")
        expect(files, "filter", .down, "list")
        expect(files, "list", .right, "preview")
        expect(files, "list", .left, "sidebar")
        expect(files, "preview", .right, nil)
        check(walk("preview", "kj", files) == ["filter", "preview"], "files preview k j retraces")

        // jira list: sidebar, filter field, table, issue panel on the right
        let jira = [
            pane("sidebar", 1, 9, 17, 90),
            pane("field", 19, 9, 80, 6),
            pane("rows", 19, 17, 52, 82),
            pane("issue", 72, 17, 27, 82),
        ]
        expect(jira, "rows", .right, "issue")
        expect(jira, "issue", .left, "rows")
        expect(jira, "rows", .left, "sidebar")
        expect(jira, "rows", .up, "field")
        expect(jira, "field", .left, "sidebar")

        // compare text: sessions, left side, right side
        let compare = [
            pane("sidebar", 1, 9, 17, 90),
            pane("left", 19, 9, 40, 90),
            pane("right", 59, 9, 40, 90),
        ]
        expect(compare, "left", .right, "right")
        expect(compare, "right", .left, "left")
        expect(compare, "left", .left, "sidebar")
        expect(compare, "right", .down, nil)
        // touching edges (no gap) still count as "past the center"
        expect(compare, "sidebar", .right, "left")

        // AI: rules, input on top, preview below
        let ai = [
            pane("rules", 1, 9, 17, 90),
            pane("input", 19, 9, 80, 40),
            pane("preview", 19, 51, 80, 48),
        ]
        expect(ai, "input", .down, "preview")
        expect(ai, "preview", .up, "input")
        expect(ai, "preview", .left, "rules")
        check(walk("preview", "hl", ai) == ["rules", "preview"], "AI preview h l comes back to the preview")
        check(walk("input", "hl", ai) == ["rules", "input"], "AI input h l comes back to the input")

        // the real notes layout: a full-width terminal under sidebar + editor;
        // the sidebar is 4 pt closer, the editor overlaps far more
        let notesLive = [
            PaneRect(id: "sidebar", rect: CGRect(x: 0, y: 30, width: 50, height: 774)),
            PaneRect(id: "editor", rect: CGRect(x: 62, y: 36, width: 1620, height: 764)),
            PaneRect(id: "terminal", rect: CGRect(x: 4, y: 808, width: 1682, height: 240)),
        ]
        expect(notesLive, "terminal", .up, "editor")
        expect(notesLive, "sidebar", .down, "terminal")
        expect(notesLive, "editor", .left, "sidebar")

        // only diagonally away: not a neighbour
        let diag = [pane("a", 0, 0, 10, 10), pane("b", 20, 20, 10, 10)]
        expect(diag, "a", .right, nil)
        expect(diag, "a", .down, nil)
        // an unknown pane id: nil, never a crash
        expect(diag, "zz", .right, nil)

        print("pane geometry: \(passed) passed, \(failed) failed")
        if failed > 0 { exit(1) }
    }
}
