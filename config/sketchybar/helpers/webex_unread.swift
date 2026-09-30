// webex_unread: print what the Webex app itself shows as unread, read from its
// window's accessibility tree (no API, no sign-in — Webex sets no Dock badge):
//   "count\t<N>"      the badge on the Messaging tab (WTMessagingHubButton)
//   "space\t<title>"  one per space / person with new messages (spaces_list)
// Exit 2 = no Accessibility permission, 3 = Webex not running / no window.
// Needs Accessibility for the process that runs it (sketchybar).
// Built on demand by notify/notify_poll.py into ~/.cache/sketchybar/.
import AppKit
import ApplicationServices

func fail(_ msg: String, _ code: Int32) -> Never {
    FileHandle.standardError.write("webex_unread: \(msg)\n".data(using: .utf8)!)
    exit(code)
}

guard AXIsProcessTrusted() else { fail("no Accessibility permission", 2) }
let bundle = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "Cisco-Systems.Spark"
guard let webex = NSRunningApplication.runningApplications(withBundleIdentifier: bundle).first
else { fail("Webex is not running", 3) }
let app = AXUIElementCreateApplication(webex.processIdentifier)
AXUIElementSetMessagingTimeout(app, 2)

func attr(_ e: AXUIElement, _ name: String) -> AnyObject? {
    var v: AnyObject?
    return AXUIElementCopyAttributeValue(e, name as CFString, &v) == .success ? v : nil
}
func kids(_ e: AXUIElement) -> [AXUIElement] { (attr(e, kAXChildrenAttribute) as? [AXUIElement]) ?? [] }
func text(_ e: AXUIElement, _ name: String) -> String {
    let v = attr(e, name)
    return (v as? String) ?? (v as? NSNumber)?.stringValue ?? ""
}

/// First element with this AXIdentifier, breadth-first (menus are skipped).
func find(_ id: String, in root: AXUIElement, depth: Int = 8) -> AXUIElement? {
    var level = [root]
    for _ in 0..<depth {
        var next: [AXUIElement] = []
        for e in level {
            if text(e, "AXIdentifier") == id { return e }
            if text(e, "AXRole") != "AXMenuBar" { next += kids(e) }
        }
        level = next
    }
    return nil
}

guard let window = (attr(app, kAXWindowsAttribute) as? [AXUIElement])?
        .first(where: { text($0, "AXIdentifier") == "main_window" }),
      let hub = find("WTMessagingHubButton", in: window)
else { fail("no Webex window", 3) }

let badge = kids(hub).first { text($0, "AXRole") == "AXValueIndicator" }
var out = "count\t\(badge.map { text($0, "AXValue") } ?? "0")\n"
if let list = find("spaces_list", in: window) {
    for row in kids(list) where text(row, "AXRole") == "AXRow" {
        guard let cell = kids(row).first, let brick = kids(cell).first,
              text(brick, "AXIdentifier") == "RegularSpaceBrickletCellView" else { continue }
        // cell = "<title>, New messages"; the title alone is the button inside
        let title = kids(brick).first.map { text($0, "AXDescription") } ?? ""
        let desc = text(cell, "AXDescription")
        // read: "<title>" / "Favorites, <title>" — anything after the title = unread
        if !title.isEmpty, title != "Recommended messages", desc.contains(title + ", ") {
            out += "space\t\(title)\n"
        }
    }
}
print(out, terminator: "")
