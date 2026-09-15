// Hides SketchyBar while the auto-hidden native menu bar is revealed, so the
// two don't draw on top of each other.
//
// Detection: the WindowServer "Menubar" window sits at y=-height and is
// off-screen while auto-hidden; when revealed it is on-screen. No
// accessibility / screen-recording permission is needed for window bounds.
//
// Cost: idle until the mouse moves. Polling (the reveal is animated and
// delayed) only runs while the cursor is near the top edge or the menu bar
// is showing (e.g. a menu is open), then stops again.
//
// Build: swiftc -O menubar_watch.swift -o menubar_watch  (sketchybarrc does this)
import AppKit

let sketchybar = "/opt/homebrew/bin/sketchybar"
let hotZone: CGFloat = 60   // pt from the top edge where we start polling
let pollInterval = 0.08

var barHidden = true   // forces the initial setBarHidden(false) to run
var pollTimer: Timer?

func menuBarShown() -> Bool {
    guard let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID)
            as? [[String: Any]] else { return false }
    return list.contains {
        ($0[kCGWindowOwnerName as String] as? String) == "Window Server" &&
        ($0[kCGWindowName as String] as? String) == "Menubar"
    }
}

func setBarHidden(_ hide: Bool) {
    guard hide != barHidden else { return }
    barHidden = hide
    let p = Process()
    p.executableURL = URL(fileURLWithPath: sketchybar)
    p.arguments = ["--bar", "hidden=\(hide ? "on" : "off")"]
    try? p.run()
}

func cursorNearTop() -> Bool {
    let loc = NSEvent.mouseLocation   // bottom-left origin, global
    guard let screen = NSScreen.screens.first(where: { NSMouseInRect(loc, $0.frame, false) })
    else { return false }
    return screen.frame.maxY - loc.y <= hotZone
}

func tick() {
    let shown = menuBarShown()
    setBarHidden(shown)
    if !shown && !cursorNearTop() {
        pollTimer?.invalidate()
        pollTimer = nil
    }
}

func startPolling() {
    guard pollTimer == nil else { return }
    pollTimer = Timer.scheduledTimer(withTimeInterval: pollInterval, repeats: true) { _ in tick() }
    tick()
}

let app = NSApplication.shared
app.setActivationPolicy(.prohibited)

NSEvent.addGlobalMonitorForEvents(matching: [.mouseMoved, .leftMouseDragged]) { _ in
    if cursorNearTop() || barHidden { startPolling() }
}

// Never leave the bar hidden if we exit.
setBarHidden(false)
for sig in [SIGTERM, SIGINT] {
    signal(sig) { _ in
        let p = Process()
        p.executableURL = URL(fileURLWithPath: sketchybar)
        p.arguments = ["--bar", "hidden=off"]
        try? p.run(); p.waitUntilExit()
        exit(0)
    }
}

app.run()
