import Foundation
import CoreGraphics

// Ctrl+H / J / K / L pane navigation, the geometry half (no AppKit:
// `bin/run-tests.sh panes`). PaneNav.swift hands it the visible panes of
// the current view as top-down rects (y grows downward, the window's
// top-left = 0,0) and moves focus to whatever `next` answers.
//
// The rule, tmux-like (the pane maps' `go()`):
//   - candidates lie fully past the current pane's center in that direction
//   - and overlap it on the other axis (a pane only diagonally away is
//     not "to the left"); among them the smallest gap (within 16 pt =
//     a tie), then the larger overlap
//   - the way back retraces where you came from (`came`), when that pane is
//     still a candidate that overlaps
//   - nothing that way: nil (no wrapping)
enum PaneDir: String, CaseIterable {
    case left = "h", down = "j", up = "k", right = "l"

    var opposite: PaneDir {
        switch self {
        case .left: return .right
        case .right: return .left
        case .up: return .down
        case .down: return .up
        }
    }
    var vertical: Bool { self == .up || self == .down }

    // the letter keys' virtual key codes (ANSI layout, like the rest of the app)
    init?(keyCode: UInt16) {
        switch keyCode {
        case 4: self = .left
        case 38: self = .down
        case 40: self = .up
        case 37: self = .right
        default: return nil
        }
    }
}

struct PaneRect {
    let id: String
    let rect: CGRect   // top-down
}

enum PaneGeometry {
    // overlap below this many points doesn't count (rounded borders touch)
    static let minOverlap: CGFloat = 2
    static let gapSlack: CGFloat = 16

    static func next(from id: String, _ dir: PaneDir, panes: [PaneRect],
                     came: [String: [PaneDir: String]] = [:]) -> String? {
        guard let cur = panes.first(where: { $0.id == id })?.rect else { return nil }
        struct Cand { let id: String; let gap: CGFloat; let overlap: CGFloat; let dist: CGFloat }
        var cands: [Cand] = []
        for p in panes where p.id != id {
            let r = p.rect
            let ahead: Bool
            switch dir {
            case .right: ahead = r.midX > cur.midX && r.minX >= cur.midX
            case .left: ahead = r.midX < cur.midX && r.maxX <= cur.midX
            case .down: ahead = r.midY > cur.midY && r.minY >= cur.midY
            case .up: ahead = r.midY < cur.midY && r.maxY <= cur.midY
            }
            guard ahead else { continue }
            let gap: CGFloat
            switch dir {
            case .right: gap = max(0, r.minX - cur.maxX)
            case .left: gap = max(0, cur.minX - r.maxX)
            case .down: gap = max(0, r.minY - cur.maxY)
            case .up: gap = max(0, cur.minY - r.maxY)
            }
            let overlap = dir.vertical
                ? min(r.maxX, cur.maxX) - max(r.minX, cur.minX)
                : min(r.maxY, cur.maxY) - max(r.minY, cur.minY)
            let dist = dir.vertical ? abs(r.midX - cur.midX) : abs(r.midY - cur.midY)
            cands.append(Cand(id: p.id, gap: gap, overlap: overlap, dist: dist))
        }
        let hits = cands.filter { $0.overlap > minOverlap }
        if let back = came[id]?[dir], hits.contains(where: { $0.id == back }) { return back }
        // gaps within one `gapSlack` are padding, not distance: the bigger
        // overlap wins there (the full-width terminal under sidebar + editor
        // goes up to the editor, though the sidebar sits 4 pt closer)
        func bucket(_ g: CGFloat) -> Int { Int((g / gapSlack).rounded(.down)) }
        return hits.min { a, b in
            if bucket(a.gap) != bucket(b.gap) { return a.gap < b.gap }
            if a.overlap != b.overlap { return a.overlap > b.overlap }
            return a.dist < b.dist
        }?.id
    }

    // after a move from → to in `dir`: going the opposite way from `to`
    // comes back to `from`
    static func remember(_ came: inout [String: [PaneDir: String]], from: String, to: String, _ dir: PaneDir) {
        came[to, default: [:]][dir.opposite] = from
    }
}
