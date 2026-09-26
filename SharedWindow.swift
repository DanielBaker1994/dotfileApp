import AppKit

// MARK: - Shared window (one place on screen for notes, files + jira)
//
// Notes, the file browser, the jira list and everything jira opens (a
// ticket's details, the release view, Jira Config) — plus command output
// windows (/health-checks) — share ONE on-screen window: exactly one of them
// is visible, in one frame, and switching swaps them in place. The windows
// themselves stay separate objects — a hidden view is PARKED (ordered out
// but alive), so the vim session, the jira tab / filters / scroll and an
// in-progress Jira Config edit all survive a switch.
//
//   Hyper+N / F / J     hidden -> show that view; showing another ->
//                       switch; already in it -> hide the window
//   header              notes | files | jira switch; jira sub-views add
//                       home + back, output views add back
//   Esc                 jira: Back (clears a search first), at the list =
//                       hide; files: hide; output: back; notes: never
//                       (vim / the shell own Esc; the notes terminal drawer
//                       is part of the notes view)
//   Cmd+W / ✕           hide the whole window
//
// Focus goes back to what was focused when the window was SUMMONED, and only
// when the whole window hides — never on a view switch (per-window restore
// targets were what scattered windows across workspaces).
// [app] shared-window = false brings back separate windows.

enum SlotView: String {
    case notes, files, jira, detail, releases, config, output
    var isJira: Bool { [.jira, .detail, .releases, .config].contains(self) }
    // a view you step back out of (Esc / back): jira's sub-views, output
    var isSub: Bool { [.detail, .releases, .config, .output].contains(self) }
}

// a window that can live in the shared window
protocol SlotMember: AnyObject {
    var slotWindow: NSWindow { get }
    var slotShown: Bool { get }
    func slotPark(stopVoice: Bool)
    func slotShow(frame: NSRect?)
}

extension PopupWindow: SlotMember {
    var slotWindow: NSWindow { nativeWindow }
    var slotShown: Bool { isShown }
    func slotPark(stopVoice: Bool) { park(stopVoice: stopVoice) }
    func slotShow(frame: NSRect?) { unpark(frame: frame) }
}

final class SharedWindow {
    // header button ids (every member's chrome)
    static let navNotes = 60, navJira = 61, navHome = 62, navBack = 63, navFiles = 64
    static let navIDs: Set<Int> = [navNotes, navJira, navHome, navBack, navFiles]

    private unowned let controller: SwitcherController
    private(set) var current: SlotView?     // the visible view (nil = hidden)
    private var last: SlotView = .notes     // what a hotkey re-opens
    private var lastJira: SlotView = .jira  // where Hyper+J comes back to
    private var stack: [SlotView] = []      // jira views under `current` (Back)
    private var returnWID: String?
    private var returnPID: pid_t?
    private var summoned = false            // on screen since the last hide

    init(controller: SwitcherController) {
        self.controller = controller
    }

    // MARK: frame

    private static let frameKey = "sharedWindowFrame"
    // the frame every view shares: the last one used (kept across launches),
    // else [app] shared-width x shared-height centered on the mouse's screen
    var frame: NSRect {
        get {
            if let s = UserDefaults.standard.string(forKey: Self.frameKey) {
                let r = NSRectFromString(s)
                if r.width > 200, r.height > 150,
                   NSScreen.screens.contains(where: { $0.visibleFrame.intersects(r) }) { return r }
            }
            let mouse = NSEvent.mouseLocation
            let vis = (NSScreen.screens.first { $0.frame.contains(mouse) } ?? NSScreen.main)?.visibleFrame
                ?? NSRect(x: 0, y: 0, width: 1400, height: 900)
            let w = min(settings.sharedWidth, vis.width - 40), h = min(settings.sharedHeight, vis.height - 40)
            return NSRect(x: vis.midX - w / 2, y: vis.midY - h / 2, width: w, height: h)
        }
        set { UserDefaults.standard.set(NSStringFromRect(newValue), forKey: Self.frameKey) }
    }

    var isVisible: Bool {
        current.flatMap { controller.slotMember($0) }?.slotShown == true
    }

    // MARK: navigation

    // Hyper+N / Hyper+J (and the menu's toggles)
    // userInIt: whether the window focused at the keypress was ours (from
    // the launcher's focus file); nil = unknown, judge from AppKit
    func hotkey(_ v: SlotView, userInIt: Bool? = nil) {
        if let cur = current, let m = controller.slotMember(cur), m.slotShown {
            let same = v == .jira ? cur.isJira : cur == v
            if same {
                // in it -> hide; visible but you're elsewhere -> focus it
                // (AppKit alone can't tell: an accessory app may report
                // isActive while the user types in another app)
                let inIt = userInIt ?? (m.slotWindow.isKeyWindow && NSApp.isActive
                    && NSWorkspace.shared.frontmostApplication?.processIdentifier == getpid())
                if inIt {
                    hide()
                } else {
                    m.slotShow(frame: nil)
                }
                return
            }
        }
        // jira comes back where you left it (a ticket, a release, Config)
        if v == .jira, lastJira != .jira, controller.slotMember(lastJira) != nil {
            present(lastJira)
        } else {
            open(v)
        }
    }

    // show a top-level view, creating its window when needed
    func open(_ v: SlotView) {
        if v == .jira && current?.isJira == true { stack = [] }
        if !v.isJira { stack.removeAll { $0 == v } }
        guard controller.ensureSlotMember(v, frame: currentFrame()) else { return }
        present(v)
    }

    // a jira sub-view whose window is ready (detail / releases / config):
    // show it, remembering where Back goes
    func push(_ v: SlotView) {
        if let cur = current, isVisible {
            if cur != v { stack.append(cur) }
        } else {
            stack = v.isJira ? [.jira] : []
        }
        stack.removeAll { $0 == v }
        present(v)
    }

    func back() {
        while let prev = stack.popLast() {
            if controller.slotMember(prev) != nil || prev == .jira {
                open(prev)
                return
            }
        }
        if let cur = current, cur.isJira, cur != .jira {
            open(.jira)
        } else {
            hide()
        }
    }

    func home() {
        stack = []
        open(.jira)
    }

    // the visible view's frame, else the remembered one
    func currentFrame() -> NSRect {
        if let cur = current, let m = controller.slotMember(cur), m.slotShown { return m.slotWindow.frame }
        return frame
    }

    // make `v` (its window exists) the visible view, in the shared frame
    func present(_ v: SlotView) {
        guard let m = controller.slotMember(v) else { return }
        if !summoned {
            // summoned: remember where focus goes back to on hide
            summoned = true
            returnWID = controller.savedWID
            returnPID = controller.savedPID
        }
        var f = currentFrame()
        if let cur = current, let old = controller.slotMember(cur), old !== m, old.slotShown {
            f = old.slotWindow.frame
            old.slotPark(stopVoice: false)
        }
        // a window with a larger minimum (Jira Config) grows the frame
        let min = m.slotWindow.minSize
        if f.width < min.width { f.size.width = min.width }
        if f.height < min.height { f.origin.y -= min.height - f.height; f.size.height = min.height }
        frame = f
        decorate(m, v)
        m.slotShow(frame: f)
        current = v
        last = v
        if v.isJira { lastJira = v }
        controller.log("shared window: \(v.rawValue)" + (stack.isEmpty ? "" : " (back: \(stack.map(\.rawValue).joined(separator: " > ")))"))
    }

    // hide the whole window; focus returns to what was focused when it was
    // summoned. The view stays parked: the next hotkey brings it back as is.
    func hide() {
        summoned = false
        guard let cur = current else { return }
        current = nil
        if let m = controller.slotMember(cur) {
            if m.slotShown { frame = m.slotWindow.frame }
            m.slotPark(stopVoice: true)
        }
        controller.restoreFocus(wid: returnWID, pid: returnPID)
        returnWID = nil
        returnPID = nil
        controller.log("shared window: hidden (\(cur.rawValue))")
    }

    // a member window went away on its own (rebuilt / closed): forget it
    func memberGone(_ v: SlotView) {
        stack.removeAll { $0 == v }
        if current == v { current = nil }
        if last == v { last = v.isJira ? .jira : .notes }
        if lastJira == v { lastJira = .jira }
    }

    // MARK: header (notes | jira, home, back)

    func navClicked(_ id: Int) {
        switch id {
        case Self.navNotes: current == .notes ? () : open(.notes)
        case Self.navFiles: current == .files ? () : controller.slotShowFiles()
        case Self.navJira: current?.isJira == true ? home() : hotkey(.jira)
        case Self.navHome: home()
        case Self.navBack: back()
        default: break
        }
    }

    // the nav buttons for a view: jira sub-views get home + back, output
    // views back
    static func navButtons(for v: SlotView) -> [(String, Int)] {
        (v.isJira && v.isSub ? [("home", navHome)] : [])
            + (v.isSub ? [("back", navBack)] : [])
            + [("notes", navNotes), ("files", navFiles), ("jira", navJira)]
    }

    private func decorate(_ m: SlotMember, _ v: SlotView) {
        if let w = m as? PopupWindow {
            if !w.headerButtons.contains(where: { $0.1 == Self.navNotes }) {
                let nav = Self.navButtons(for: v)
                w.headerButtons = nav + w.headerButtons
                w.headerOrder = nav.map(\.1) + (w.headerOrder ?? [])
                let prev = w.onHeaderButton
                w.onHeaderButton = { [weak self] id in
                    if Self.navIDs.contains(id) { self?.navClicked(id) } else { prev?(id) }
                }
                w.onCloseWindow = { [weak self] in self?.hide() }
            }
            w.setHeaderButtonOn(Self.navNotes, v == .notes)
            w.setHeaderButtonOn(Self.navFiles, v == .files)
            w.setHeaderButtonOn(Self.navJira, v.isJira)
        } else if let c = m as? JiraDashboardWindow {
            c.setSlotNav(Self.navButtons(for: v), on: [Self.navJira]) { [weak self] id in
                self?.navClicked(id)
            }
        }
    }
}
