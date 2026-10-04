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
//   Hyper+N             hidden -> show the view you were last on;
//                       already in it -> hide the window (no per-view keys)
//   header              notes | files | jira switch; jira sub-views add
//                       home + back, output views add back
//   Esc                 jira: Back (clears a search first); output: back.
//                       At the top of a view Esc HIDES the window only when
//                       that view's kitchen sink "Esc Hides Window" is on
//                       (`esc-close` in its section; default off — vim, the
//                       shell, AI and confluence keep Esc for themselves)
//   Cmd+W / ✕           hide the whole window
//   confluence          shares the frame like every other view (a switch
//                       never resizes the window)
//   ai                  fm (Apple's on-device model) driven by rule files
//                       (AIWindow.swift); shares the frame
//   compare             Beyond Compare-style Text Compare (CompareWindow
//                       .swift); compareText = a text compare pushed on it
//                       (back / Esc return to the view under it)
//
// Focus goes back to what was focused when the window was SUMMONED, and only
// when the whole window hides — never on a view switch (per-window restore
// targets were what scattered windows across workspaces).
// Floating vs tiling is AeroSpace's call (its on-window-detected rule places
// the app's windows); the app only keeps them at normal level. A shown view lands
// on the focused workspace, at the remembered frame moved onto that
// workspace's monitor (`targetScreen`) — after AeroSpace's closed-windows
// cache is cleared (`clearAerospaceCache`), or AeroSpace "restores the
// world" and throws you back to the workspace you hid it on.
// [app] shared-window = false brings back separate windows.

enum SlotView: String {
    case notes, files, jira, detail, releases, config, output, confluence, ai, compare, compareText
    var isJira: Bool { [.jira, .detail, .releases, .config].contains(self) }
    var isCompare: Bool { self == .compare || self == .compareText }
    // a view you step back out of (Esc / back): jira's sub-views, output,
    // a text compare pushed on the compare view
    var isSub: Bool { [.detail, .releases, .config, .output, .compareText].contains(self) }
}

// a window that can live in the shared window
protocol SlotMember: AnyObject {
    var slotWindow: NSWindow { get }
    var slotShown: Bool { get }
    // the frame the other views share: notes leaves its drawer growth out
    var slotBaseFrame: NSRect { get }
    func slotPark(stopVoice: Bool)
    func slotShow(frame: NSRect?)
}

extension PopupWindow: SlotMember {
    var slotWindow: NSWindow { nativeWindow }
    var slotShown: Bool { isShown }
    var slotBaseFrame: NSRect { baseFrame }
    func slotPark(stopVoice: Bool) { park(stopVoice: stopVoice) }
    func slotShow(frame: NSRect?) { unpark(frame: frame) }
}

final class SharedWindow {
    // header button ids (every member's chrome)
    static let navNotes = 60, navJira = 61, navHome = 62, navBack = 63, navFiles = 64, navConfluence = 65,
               navAI = 66, navCompare = 67
    static let navIDs: Set<Int> = [navNotes, navJira, navHome, navBack, navFiles, navConfluence, navAI, navCompare]

    private unowned let controller: SwitcherController
    private(set) var current: SlotView?     // the visible view (nil = hidden)
    private var last: SlotView = .files     // what Hyper+N re-opens
    private var lastJira: SlotView = .jira  // where the jira icon comes back to
    private var stack: [SlotView] = []      // jira views under `current` (Back)
    private var returnWID: String?
    private var returnPID: pid_t?
    private var summoned = false            // on screen since the last hide
    private var swappedAt: Date?            // the last view swap (focus-steal grace)
    private var focusLossGen = 0            // stale pending checks drop out
    // the focused AeroSpace workspace's monitor at the last hotkey (1-based
    // NSScreen.screens index, from hotkeyPrep): a hidden window comes back
    // on THAT screen. Cleared on hide.
    var targetScreen: Int?
    // hotkeyPrep already cleared AeroSpace's closed-windows cache for the
    // show this hotkey triggers (only for that hotkey: reset after it)
    var aerospaceCacheCleared = false

    init(controller: SwitcherController) {
        self.controller = controller
        // focus loss, for EVERY view (popups stand down: hostHandlesFocusLoss)
        let nc = NotificationCenter.default
        nc.addObserver(forName: NSWindow.didResignKeyNotification, object: nil, queue: .main) { [weak self] n in
            guard let self, let w = n.object as? NSWindow, let cur = self.current,
                  let m = self.controller.slotMember(cur), m.slotWindow === w else { return }
            self.focusLossGen += 1
            let gen = self.focusLossGen
            DispatchQueue.main.asyncAfter(deadline: .now() + settings.focusLossDelay) { [weak self] in
                guard let self, gen == self.focusLossGen else { return }
                self.checkFocusLoss(cur, w)
            }
        }
        nc.addObserver(forName: NSWindow.didBecomeKeyNotification, object: nil, queue: .main) { [weak self] _ in
            self?.focusLossGen += 1
        }
        // a drag / resize of the visible view is remembered at once, so a
        // window left up on another workspace comes back where you put it
        // (AeroSpace's off-screen "hidden" corner is refused by `frame`).
        // Read a turn later: a drawer opening moves the window mid-change,
        // before its bookkeeping (baseFrame) is final.
        for name in [NSWindow.didMoveNotification, NSWindow.didEndLiveResizeNotification] {
            nc.addObserver(forName: name, object: nil, queue: .main) { [weak self] n in
                guard let w = n.object as? NSWindow else { return }
                DispatchQueue.main.async {
                    guard let self, let cur = self.current, let m = self.controller.slotMember(cur),
                          m.slotShown, m.slotWindow === w else { return }
                    self.frame = m.slotBaseFrame
                }
            }
        }
    }

    // Focus left the visible view `v` (window `w`) and stayed away
    // settings.focusLossDelay. Right after a view swap it was STOLEN: the
    // outgoing window ordering out makes aerospace focus "the next window",
    // often another app's, raised over ours — take it back. Otherwise the
    // user left: hide the whole window when hide-on-focus-loss says so. It is
    // PARKED like any hide (views keep their state — hiding the popup member
    // itself tore files / jira / detail down) and focus is NOT handed back
    // (the user already went somewhere). alt-hjkl to another window IS
    // leaving.
    private func checkFocusLoss(_ v: SlotView, _ w: NSWindow) {
        guard current == v, let m = controller.slotMember(v), m.slotShown, m.slotWindow === w,
              focusLeft(w) else { return }
        let front = NSWorkspace.shared.frontmostApplication?.bundleIdentifier ?? "?"
        if let t = swappedAt, Date().timeIntervalSince(t) < 1.0 {
            swappedAt = nil
            controller.log("shared window: focus stolen by \(front) right after a view swap — refocused \(v.rawValue)")
            m.slotShow(frame: nil)
            return
        }
        guard settings.hideOnFocusLoss else { return }
        // one global switch for every view (no per-view `sticky` here)
        if let p = m as? PopupWindow, p.isShowingMenu { return }
        hide("focus loss → \(front)", restoreFocus: false)
    }

    // MARK: frame

    private static let frameKey = "sharedWindowFrame"
    // the frame every view shares: the last one used (kept across launches),
    // else [app] shared-width x shared-height centered on the mouse's screen;
    // moved onto `targetScreen` when it sits on another one
    var frame: NSRect {
        get {
            if let s = UserDefaults.standard.string(forKey: Self.frameKey) {
                let r = NSRectFromString(s)
                if r.width > 200, r.height > 150, let from = Self.screen(of: r) {
                    return Self.place(r, from: from, to: targetNSScreen ?? from)
                }
            }
            let mouse = NSEvent.mouseLocation
            let vis = (targetNSScreen ?? NSScreen.screens.first { $0.frame.contains(mouse) } ?? NSScreen.main)?.visibleFrame
                ?? NSRect(x: 0, y: 0, width: 1400, height: 900)
            let w = min(settings.sharedWidth, vis.width - 40), h = min(settings.sharedHeight, vis.height - 40)
            return NSRect(x: vis.midX - w / 2, y: vis.midY - h / 2, width: w, height: h)
        }
        set {
            // a window on a hidden AeroSpace workspace sits off-screen in a
            // corner: never remember THAT as the shared frame (the next show
            // would land off-screen / slide in)
            guard Self.screen(of: newValue) != nil else { return }
            UserDefaults.standard.set(NSStringFromRect(newValue), forKey: Self.frameKey)
        }
    }

    private var targetNSScreen: NSScreen? {
        guard let i = targetScreen, NSScreen.screens.indices.contains(i - 1) else { return nil }
        return NSScreen.screens[i - 1]
    }

    // the screen holding most of `r`; nil = under half of it is on any screen
    static func screen(of r: NSRect) -> NSScreen? {
        let area = r.width * r.height
        guard area > 0 else { return nil }
        let best = NSScreen.screens.max { a, b in
            let ia = a.frame.intersection(r), ib = b.frame.intersection(r)
            return ia.width * ia.height < ib.width * ib.height
        }
        guard let best else { return nil }
        let i = best.frame.intersection(r)
        return i.width * i.height >= area / 2 ? best : nil
    }

    // `r` (on `from`) at the same relative spot on `to`, inside its visible
    // frame (a smaller screen shrinks it to fit)
    static func place(_ r: NSRect, from: NSScreen, to: NSScreen) -> NSRect {
        let fv = from.visibleFrame, tv = to.visibleFrame
        let w = min(r.width, tv.width), h = min(r.height, tv.height)
        var x = r.minX, y = r.minY
        if from != to {
            x = tv.minX + (r.midX - fv.minX) / fv.width * tv.width - w / 2
            y = tv.minY + (r.midY - fv.minY) / fv.height * tv.height - h / 2
        }
        x = min(max(x, tv.minX), tv.maxX - w)
        y = min(max(y, tv.minY), tv.maxY - h)
        return NSRect(x: x, y: y, width: w, height: h)
    }

    // where a view shows: the visible view's frame (every view shares it,
    // so a switch never resizes the window), else the remembered one
    func currentFrame() -> NSRect {
        shownMember?.slotBaseFrame ?? frame
    }

    var isVisible: Bool {
        shownMember != nil
    }

    // MARK: navigation

    // the visible view's member, nil = hidden
    private var shownMember: SlotMember? {
        current.flatMap { controller.slotMember($0) }.flatMap { $0.slotShown ? $0 : nil }
    }

    // a hotkey for the visible view: in it -> hide; visible but you're
    // elsewhere -> focus it. userInIt: whether the window focused at the
    // keypress was ours (the launcher's focus file); nil = unknown, judge
    // from AppKit (alone it can't always tell: an accessory app may report
    // isActive while the user types in another app)
    private func hideOrFocus(_ m: SlotMember, userInIt: Bool?) {
        let inIt = userInIt ?? (m.slotWindow.isKeyWindow && NSApp.isActive
            && NSWorkspace.shared.frontmostApplication?.processIdentifier == getpid())
        if inIt { hide("hotkey pressed while in it") } else { m.slotShow(frame: nil) }
    }

    // a named view (CLI `notes` / `jira` / …, the menu's toggles)
    func hotkey(_ v: SlotView, userInIt: Bool? = nil) {
        if let cur = current, let m = shownMember, v == .jira ? cur.isJira : cur == v {
            hideOrFocus(m, userInIt: userInIt)
            return
        }
        // jira comes back where you left it (a ticket, a release, Config)
        if v == .jira, lastJira != .jira, controller.slotMember(lastJira) != nil {
            present(lastJira)
        } else {
            open(v)
        }
    }

    // Hyper+N: THE show / hide key. Hidden -> the view you were last on
    // (jira stays jira, never back to notes); in it -> hide; visible but
    // you're elsewhere -> focus it. Views are switched inside the window
    // (Ctrl+Tab, header icons, the Hyper+S palette).
    func toggle(userInIt: Bool? = nil) {
        if let m = shownMember {
            hideOrFocus(m, userInIt: userInIt)
            return
        }
        if controller.slotMember(last) != nil {
            present(last)
            return
        }
        // its window is gone (rebuilt, feature switched off): the top-level
        // view, else files
        let top: SlotView = last.isJira ? .jira : last
        if top == .files { controller.slotShowFiles() } else { open(top) }
        if !isVisible && top != .files { controller.slotShowFiles() }
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
            stack = v.isJira ? [.jira] : v == .compareText ? [.compare] : []
        }
        stack.removeAll { $0 == v }
        present(v)
    }

    // the header's back (nothing left = hide) or Esc (`esc`: nothing left =
    // hide only when the view's "Esc Hides Window" is on)
    func back(esc: Bool = false) {
        while let prev = stack.popLast() {
            if controller.slotMember(prev) != nil || prev == .jira || prev == .compare {
                open(prev)
                return
            }
        }
        if let cur = current, cur.isJira, cur != .jira {
            open(.jira)
        } else if current == .compareText {
            open(.compare)
        } else if let cur = current {
            if esc { escapeAtTop(cur) } else { hide("back from the first view") }
        }
    }

    func home() {
        stack = []
        open(.jira)
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
        // the old view is parked only AFTER the new one is up: parked first,
        // aerospace sees its focused window vanish and focuses the next
        // window on the workspace (a terminal raised over the slower jira
        // window looked like the switch had closed it)
        var outgoing: SlotMember?
        if let old = shownMember, old !== m {
            frame = old.slotBaseFrame
            outgoing = old
        }
        // normal level, whatever the window was built with (Jira Config is
        // made for a standalone life above the popups)
        if let p = m as? PopupWindow { p.setFloating(false) } else { m.slotWindow.level = .normal }
        // a window with a larger minimum (Jira Config) grows the frame
        let min = m.slotWindow.minSize
        if f.width < min.width { f.size.width = min.width }
        if f.height < min.height { f.origin.y -= min.height - f.height; f.size.height = min.height }
        frame = f
        decorate(m, v)
        // a hidden view coming back is, to AeroSpace, a closed window
        // reappearing: clear its closed-windows cache first (the hotkey's
        // prep already did, in parallel with its queries)
        if !m.slotShown && !aerospaceCacheCleared { Self.clearAerospaceCache() }
        aerospaceCacheCleared = false
        m.slotShow(frame: f)
        outgoing?.slotPark(stopVoice: false)
        if outgoing != nil { swappedAt = Date() }
        current = v
        last = v
        if v.isJira { lastJira = v }
        controller.log("shared window: \(v.rawValue)" + (stack.isEmpty ? "" : " (back: \(stack.map(\.rawValue).joined(separator: " > ")))"))
    }

    // AeroSpace's lock-screen defence: whenever a window dies — and an
    // ordered-out window is a dead one to it — it snapshots the WHOLE world
    // (every workspace's tiles + floating windows, which workspace each
    // monitor shows), and when that window id shows up again it RESTORES
    // the snapshot: the monitor flips back to the workspace you hid the
    // window on (the old "jumped straight back to 4", "focus stolen right
    // after a view swap") and tiles snap back to their old sizes. Only
    // layout-changing commands clear that cache — `workspace N` doesn't. A
    // no-op `eval true` does (AeroSpace closedWindowsCache.swift / Shell.swift).
    // Before any hidden view is ordered in, then: the window comes back as a
    // NEW window, on the focused workspace. ~10-20 ms; bounded.
    static func clearAerospaceCache() {
        guard liveAerospaceSocket() != nil else { return }
        _ = aerospaceSocket(["eval", "true"], timeout: 0.25)
    }

    // preload: a freshly built, still hidden view gets the shared frame and
    // its header now, so its first show is an unpark like any other
    func prepare(_ v: SlotView) {
        guard let m = controller.slotMember(v), !m.slotShown else { return }
        m.slotWindow.setFrame(currentFrame(), display: false)
        decorate(m, v)
    }

    // hide the whole window; focus returns to what was focused when it was
    // summoned. The view stays parked: the next hotkey brings it back as is.
    // `reason` rides in the log: a hide the user didn't expect can be traced
    func hide(_ reason: String = "", restoreFocus: Bool = true) {
        summoned = false
        swappedAt = nil
        targetScreen = nil
        aerospaceCacheCleared = false
        guard let cur = current else { return }
        current = nil
        if let m = controller.slotMember(cur) {
            if m.slotShown { frame = m.slotBaseFrame }
            m.slotPark(stopVoice: true)
        }
        if restoreFocus { controller.restoreFocus(wid: returnWID, pid: returnPID) }
        returnWID = nil
        returnPID = nil
        controller.log("shared window: hidden (\(cur.rawValue))" + (reason.isEmpty ? "" : " — \(reason)"))
    }

    // MARK: Esc

    // the views with an "Esc Hides Window" switch (their kitchen sink);
    // jira's sub-views step back with Esc and follow jira's switch
    static let escViews: [SlotView] = [.files, .notes, .ai, .jira, .confluence, .compare]

    // Esc reached the top of view `v` (no search to clear, nothing to step
    // back from): hide the window if the view's switch is on, else nothing
    func escapeAtTop(_ v: SlotView) {
        guard controller.escHideCount(v) > 0 else { return }
        hide("Esc (\(v.rawValue))")
    }

    // a member window went away on its own (rebuilt / closed): forget it
    func memberGone(_ v: SlotView) {
        stack.removeAll { $0 == v }
        if current == v { current = nil }
        if last == v { last = v.isJira ? .jira : v == .compareText ? .compare : .files }
        if lastJira == v { lastJira = .jira }
    }

    // MARK: header (notes / files / jira icons; back, home)

    // Ctrl+Tab / Ctrl+Shift+Tab: the next / previous view in header-icon
    // order, wrapping (a view with no icon, e.g. output, starts at the ends)
    func cycle(_ dir: Int) {
        let ids = Self.navIcons.map(\.id)
        guard !ids.isEmpty else { return }
        let n = ids.count
        let ci = current.flatMap(Self.navOn).flatMap { ids.firstIndex(of: $0) }
        let i = ci ?? (dir > 0 ? -1 : n)
        let next = ids[((i + dir) % n + n) % n]
        controller.log("cycle: current=\(current?.rawValue ?? "nil") currentNav=\(ci.map(String.init) ?? "nil") ids=\(ids) dir=\(dir) next=\(next)")
        navClicked(next)
    }

    func navClicked(_ id: Int) {
        switch id {
        case Self.navNotes: current == .notes ? () : open(.notes)
        case Self.navFiles: current == .files ? () : controller.slotShowFiles()
        case Self.navJira: current?.isJira == true ? home() : hotkey(.jira)
        case Self.navConfluence: current == .confluence ? () : open(.confluence)
        case Self.navAI: current == .ai ? () : open(.ai)
        case Self.navCompare: current == .compare ? () : open(.compare)
        case Self.navHome: home()
        case Self.navBack: back()
        default: break
        }
    }

    // the view switcher: files (first, the default view) / notes / AI /
    // jira / confluence / compare as icons just right of the kitchen sink
    // (the app's icon menu), top-left in every view
    static var navIcons: [(image: NSImage, id: Int, tip: String)] {
        [(filesNavIcon, navFiles, "Files"), (notesNavIcon, navNotes, "Notes")]
            + (aiEnabled() ? [(aiNavIcon, navAI, "AI view")] : [])
            + [(jiraNavIcon, navJira, "Jira")]
            + (confluenceEnabled() ? [(confluenceNavIcon, navConfluence, "Confluence search")] : [])
            + (compareEnabled() ? [(compareNavIcon, navCompare, "Compare")] : [])
    }

    // the view's own nav words (right-hand bar, never beside the switcher):
    // jira sub-views get home + back, output views + compareText back
    static func navButtons(for v: SlotView) -> [(String, Int)] {
        (v.isSub ? [("back", navBack)] : [])
            + (v.isJira && v.isSub ? [("home", navHome)] : [])
    }

    static func navOn(_ v: SlotView) -> Int? {
        switch v {
        case .notes: return navNotes
        case .files: return navFiles
        case .confluence: return navConfluence
        case .ai: return navAI
        case .compare, .compareText: return navCompare
        case _ where v.isJira: return navJira
        default: return nil
        }
    }

    private func decorate(_ m: SlotMember, _ v: SlotView) {
        if let w = m as? PopupWindow {
            w.hostHandlesFocusLoss = true
            if w.navIcons.isEmpty {
                let nav = Self.navButtons(for: v)
                w.headerButtons = w.headerButtons + nav
                if let order = w.headerOrder { w.headerOrder = order + nav.map(\.1) }
                let prev = w.onHeaderButton
                w.onHeaderButton = { [weak self] id in
                    if Self.navIDs.contains(id) { self?.navClicked(id) } else { prev?(id) }
                }
                w.onCloseWindow = { [weak self] in self?.hide("✕ / Cmd+W") }
            }
            // [confluence] enabled may have changed since the last show
            w.navIcons = Self.navIcons
            // the kitchen sink, whatever the view (its menu stays the view's)
            w.headerIcon = appIcon
            w.navOn = Self.navOn(v)
            w.onCycleView = { [weak self] in self?.cycle($0) }
        } else if let c = m as? CardWindowController, let on = Self.navOn(v) {
            // Confluence, AI, Jira Config
            c.setSlotNav(Self.navButtons(for: v), icons: Self.navIcons, icon: appIcon, on: on) { [weak self] id in
                self?.navClicked(id)
            }
            c.onCycleView = { [weak self] in self?.cycle($0) }
            c.onSlotHide = { [weak self] in self?.hide("✕ / Cmd+W (\(v.rawValue))") }
        }
    }
}
