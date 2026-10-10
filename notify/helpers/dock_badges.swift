import AppKit
import ApplicationServices

guard let dock = NSRunningApplication.runningApplications(
        withBundleIdentifier: "com.apple.dock").first else { exit(1) }
let app = AXUIElementCreateApplication(dock.processIdentifier)

func attr(_ e: AXUIElement, _ name: String) -> AnyObject? {
    var v: AnyObject?
    return AXUIElementCopyAttributeValue(e, name as CFString, &v) == .success ? v : nil
}

guard AXIsProcessTrusted() else {
    FileHandle.standardError.write("dock_badges: no Accessibility permission\n".data(using: .utf8)!)
    exit(2)
}
var out = ""
for list in (attr(app, kAXChildrenAttribute) as? [AXUIElement]) ?? [] {
    for item in (attr(list, kAXChildrenAttribute) as? [AXUIElement]) ?? [] {
        guard let url = attr(item, "AXURL") as? URL,
              let id = Bundle(url: url)?.bundleIdentifier else { continue }
        let label = (attr(item, "AXStatusLabel") as? String) ?? ""
        out += "\(id)\t\(label)\n"
    }
}
print(out, terminator: "")
