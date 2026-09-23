import AppKit
import Observation

// MARK: - AppState (app-level state, observed by the controller)

@Observable
final class AppState {
    // Workspaces (from aerospace)
    var workspaces: [WorkspaceInfo] = []

    // Commands (from commands.conf)
    var commands: [CommandSpec] = []

    // Icon rules (from [icons] section)
    var iconRules: [IconRule] = []

    // App settings (from [app] section)
    var shell = "/opt/homebrew/bin/bash"
    var shellArgs = ["--login", "-i"]
    var terminalFont = "Hack Nerd Font"
    var aerospaceCLI = ["/opt/homebrew/bin/aerospace", "/usr/local/bin/aerospace", "aerospace"]
    var colorSources = [NSString(string: "~/.config/sketchybar/colors.sh").expandingTildeInPath,
                        NSString(string: "~/.config/sketchybar/plugins/aerospacer.sh").expandingTildeInPath]
    var appDirs = ["/Applications", "/Applications/Utilities",
                   "/System/Applications", "/System/Applications/Utilities",
                   "/System/Library/CoreServices",
                   NSHomeDirectory() + "/Applications"]
    var switcherWindowName = "workspace-switcher"
    var detailWindowName = "jira-detail"
    var hideOnFocusLoss = true

    // UI mode in the switcher popup
    var commandMode = false
    var workspaceSelection = 0
    var commandSelection = 0

    // Saved focus target (for focus restoration)
    var savedWID: String?
    var savedPID: pid_t?

    // Theme colors (parsed from sketchybar + [theme])
    var barColor: NSColor = .black
    var groupBgColor: NSColor = .gray
    var textColor: NSColor = .white
    var dimColor: NSColor = .gray
    var borderColor: NSColor = .white
    var accentColor = NSColor(srgbRed: 85/255, green: 104/255, blue: 130/255, alpha: 1)
    var themeHeader: NSColor?

    // Icon cache (computed on demand)
    private var iconCache: [String: NSImage] = [:]

    // MARK: - Filtering

    func filterRows(query: String, workspaces: [WorkspaceInfo], commands: [CommandSpec]) -> [PopupRow] {
        let q = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()

        if q.hasPrefix("/") {
            // Command palette mode
            let sub = String(q.dropFirst()).trimmingCharacters(in: .whitespaces)
            let cmds = PopupFuzzy.filter(commands, query: sub) { $0.name }
            return cmds.map { CommandRow($0) }
        }

        // Workspace mode
        let vis = q.isEmpty
            ? workspaces
            : workspaces.filter { ws in
                ws.id.lowercased().contains(q)
                    || ws.apps.contains { $0.name.lowercased().contains(q) }
            }
        var iconCache = self.iconCache
        let rows = vis.map { ws in
            WorkspaceRow(id: ws.id, icons: iconsForWorkspace(ws, cache: &iconCache), trailing: trailingForWorkspace(ws))
        }
        self.iconCache = iconCache
        return rows
    }

    private func iconsForWorkspace(_ ws: WorkspaceInfo, cache: inout [String: NSImage]) -> [NSImage] {
        let maxIcons = 3
        var imgs: [NSImage] = []
        for app in ws.apps.prefix(maxIcons) {
            let key = (app.bundleID ?? app.name) + "|" + (app.windowTitle ?? "")
            if let cached = cache[key] {
                imgs.append(cached)
            } else {
                let img = iconForApp(app)
                cache[key] = img
                imgs.append(img)
            }
        }
        return imgs
    }

    private func trailingForWorkspace(_ ws: WorkspaceInfo) -> String? {
        let extra = ws.apps.count - 3
        return extra > 0 ? "+\(extra)" : nil
    }

    private func iconForApp(_ app: AppInfo) -> NSImage {
        // Check icon rules first
        if let rule = iconRules.first(where: { $0.app == app.name }) {
            let title = app.windowTitle?.lowercased() ?? ""
            for (match, img) in rule.titleMatches where title.contains(match) {
                return img
            }
            if let d = rule.defaultIcon { return d }
        }

        // Try bundle ID
        var url: URL?
        if let bid = app.bundleID {
            url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: bid)
        }
        if url == nil {
            for dir in appDirs where FileManager.default.fileExists(atPath: dir + "/" + app.name + ".app") {
                url = URL(fileURLWithPath: dir + "/" + app.name + ".app")
                break
            }
        }
        if let url { return NSWorkspace.shared.icon(forFile: url.path) }
        return missingIcon
    }

    // MARK: - Config helpers

    func workspaceColors() -> PopupColors {
        PopupColors(
            background: barColor,
            border: borderColor,
            text: textColor,
            dim: dimColor,
            highlight: groupBgColor,
            accent: accentColor
        )
    }
}

// MARK: - Missing icon (cached)

let missingIcon: NSImage = {
    let size: CGFloat = 22
    let img = NSImage(size: NSSize(width: size, height: size))
    img.lockFocus()
    let rect = NSRect(x: 0, y: 0, width: size, height: size)
    let path = NSBezierPath(roundedRect: rect, xRadius: 5, yRadius: 5)
    NSColor.gray.setFill()
    path.fill()
    NSColor.white.setStroke()
    path.lineWidth = 1
    path.stroke()
    let attrs: [NSAttributedString.Key: Any] = [
        .font: NSFont.systemFont(ofSize: 14), .foregroundColor: NSColor.white,
    ]
    let s = "?" as NSString
    let sz = s.size(withAttributes: attrs)
    s.draw(at: NSPoint(x: (size - sz.width) / 2, y: (size - sz.height) / 2),
           withAttributes: attrs)
    img.unlockFocus()
    return img
}()
