import AppKit
import Foundation

// ============================================================================
// Install modes + the Setup & Health Check window.
//
// Two kinds of install (paths: `assetDir` / `userDir` / `homeDir` in
// workspace_switcher.swift):
//   repo build   ./INSTALL.sh — everything lives in the git checkout
//   app install  the DMG — code in Contents/Resources, the user's files in
//                ~/.config/workspace-switcher, set up by bin/setup-home.sh
//
// `AppInstall.ensureHome()` runs first thing in main.swift (app installs
// only; a marker check, no process, when nothing changed). The window shows
// what bin/preflight.sh --json reports — the SAME checks INSTALL.sh prints —
// with a Fix button per row, "Move to Applications", and the opt-in
// "Hotkeys & Borders" step (Homebrew packages + config links).
//
// Config: commands.toml [setup] (title, intro, width, height).
// Open: first run / new version / a required check fails, menu bar ▸
// "Setup & Health Check…", CLI / socket `setup`.
// ============================================================================

enum AppInstall {
    enum State: Equatable {
        case repo              // repo build: INSTALL.sh owns the home
        case ready(fresh: Bool) // app install, home set up (fresh = first run / new version)
        case checkout          // app install, but the home is a developer checkout
        case notInstalled      // running from the disk image / translocated
        case failed(String)
    }
    static var state: State = .repo
    // `workspace-switcher setup` with no daemon running: open it at launch
    static var requested = false
    static let keepCheckoutKey = "setupKeepCheckout"

    static var version: String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? ""
    }
    static var markerPath: String { homeDir + "/.install" }

    // translocated (quarantined app opened in place) or still on the image
    static var runsFromImage: Bool {
        guard let b = appBundlePath else { return false }
        return b.contains("/AppTranslocation/") || b.hasPrefix("/Volumes/")
    }

    // bash <assetDir>/bin/NAME ARGS, synchronously (setup-home.sh is quick)
    @discardableResult
    static func runSync(_ script: String, _ args: [String]) -> (code: Int32, out: String) {
        do {
            let r = try runProcess("/bin/bash", [assetDir + "/bin/" + script] + args, mergeStderr: true)
            return (r.code, r.out)
        } catch {
            return (-1, "cannot run \(script): \(error.localizedDescription)")
        }
    }

    // `key=value` lines of the marker
    static func marker() -> [String: String] {
        guard let text = try? String(contentsOfFile: markerPath, encoding: .utf8) else { return [:] }
        var d: [String: String] = [:]
        for line in text.split(separator: "\n") {
            guard let eq = line.firstIndex(of: "="), !line.hasPrefix("seed ") else { continue }
            d[String(line[..<eq])] = String(line[line.index(after: eq)...])
        }
        return d
    }

    // Called before any config is read. App installs only: make sure the
    // home exists and points at THIS app. Unchanged install = two file reads.
    static func ensureHome(switchFromCheckout: Bool = false) {
        guard !isRepoBuild, let bundle = appBundlePath else { state = .repo; return }
        if runsFromImage { state = .notInstalled; return }
        let fm = FileManager.default
        let m = marker()
        if !switchFromCheckout, m["mode"] == "app", m["app"] == bundle, m["version"] == version,
           fm.fileExists(atPath: homeDir + "/commands.toml"),
           (try? fm.destinationOfSymbolicLink(atPath: homeDir + "/workspace-switcher.app")) == bundle {
            state = .ready(fresh: false)
            return
        }
        let r = runSync("setup-home.sh", ["app", bundle] + (switchFromCheckout ? ["--switch"] : []))
        switch r.code {
        case 0: state = .ready(fresh: true)
        case 3: state = .checkout
        default:
            let why = r.out.split(separator: "\n").last.map(String.init) ?? "setup-home.sh failed"
            state = .failed(why)
        }
    }

    // should the Setup window open by itself at launch?
    static var wantsSetupWindow: Bool {
        switch state {
        case .repo: return false
        case .ready(let fresh): return fresh
        case .checkout: return !UserDefaults.standard.bool(forKey: keepCheckoutKey)
        case .notInstalled, .failed: return true
        }
    }
}

struct SetupCheck {
    let id, group, level, title, detail, fix, action: String
    let ok: Bool
    var required: Bool { level == "required" }
}

// flipped so rows lay out top-down
private final class SetupFlippedView: NSView {
    override var isFlipped: Bool { true }
}

final class SetupWindow: NSObject, NSWindowDelegate {
    private static var live: SetupWindow?

    private let window: NSWindow
    private let intro = NSTextField(wrappingLabelWithString: "")
    private let scroll = NSScrollView()
    private let rows = SetupFlippedView()
    private let logScroll = NSScrollView()
    private let logView = NSTextView()
    private let status = NSTextField(labelWithString: "")
    private let recheck = ThemedPushButton(title: "Check Again", target: nil, action: nil)
    private let stack = ThemedPushButton(title: "Set Up Hotkeys & Borders…", target: nil, action: nil)
    private let done = ThemedPushButton(title: "Done", target: nil, action: nil)
    private var monitor: Any?
    private weak var controller: SwitcherController?
    private var checks: [SetupCheck] = []
    private var running: Process?
    private var busy = false { didSet { updateButtons() } }
    private var colors: PopupColors { PopupThemeDefaults.colors }

    private static func setting(_ key: String, _ fallback: String) -> String {
        let v = configSectionValue("setup", key) ?? ""
        return v.isEmpty ? fallback : v
    }

    static func show(controller: SwitcherController?) {
        if let w = live {
            NSApp.activate(ignoringOtherApps: true)
            w.window.makeKeyAndOrderFront(nil)
            w.runChecks()
            return
        }
        let w = SetupWindow(controller: controller)
        live = w
        NSApp.activate(ignoringOtherApps: true)
        w.window.center()
        w.window.makeKeyAndOrderFront(nil)
        w.runChecks()
    }

    private init(controller: SwitcherController?) {
        self.controller = controller
        let W = CGFloat(Double(Self.setting("width", "640")) ?? 640)
        let H = CGFloat(Double(Self.setting("height", "620")) ?? 620)
        window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: max(520, W), height: max(440, H)),
                          styleMask: [.titled, .closable, .resizable, .fullSizeContentView],
                          backing: .buffered, defer: false)
        super.init()
        let c = colors
        window.title = Self.setting("title", "workspace-switcher — Setup & Health Check")
        window.titlebarAppearsTransparent = true
        window.isReleasedWhenClosed = false
        window.backgroundColor = c.base
        window.appearance = NSAppearance(named: c.isLight ? .aqua : .darkAqua)
        window.minSize = NSSize(width: 520, height: 440)
        // above the popup windows (they float at .popUpMenu)
        window.level = NSWindow.Level(rawValue: NSWindow.Level.popUpMenu.rawValue + 1)
        window.delegate = self

        let content = NSView(frame: window.contentLayoutRect)
        content.autoresizingMask = [.width, .height]
        let w = content.bounds.width, h = content.bounds.height

        intro.font = .systemFont(ofSize: 12)
        intro.textColor = c.dim
        intro.frame = NSRect(x: 20, y: h - 78, width: w - 40, height: 40)
        intro.autoresizingMask = [.width, .minYMargin]
        content.addSubview(intro)

        let logH = Self.logHeight
        scroll.frame = NSRect(x: 20, y: 64, width: w - 40, height: h - 88 - 64)
        scroll.autoresizingMask = [.width, .height]
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = true
        scroll.backgroundColor = c.mantle
        scroll.wantsLayer = true
        scroll.layer?.cornerRadius = 8
        scroll.layer?.borderWidth = 1
        scroll.layer?.borderColor = c.hairline.cgColor
        scroll.documentView = rows
        content.addSubview(scroll)

        // what a Fix / the stack set-up printed (read-only; Cmd+C / Cmd+A work)
        logScroll.frame = NSRect(x: 20, y: 64, width: w - 40, height: logH)
        logScroll.autoresizingMask = [.width, .maxYMargin]
        logScroll.hasVerticalScroller = true
        logScroll.drawsBackground = true
        logScroll.backgroundColor = c.crust
        logScroll.wantsLayer = true
        logScroll.layer?.cornerRadius = 8
        logView.isEditable = false
        logView.isSelectable = true
        logView.drawsBackground = false
        logView.font = .monospacedSystemFont(ofSize: 11, weight: .regular)
        logView.textColor = c.dim
        logView.textContainerInset = NSSize(width: 6, height: 6)
        logView.autoresizingMask = [.width]
        logView.frame = NSRect(x: 0, y: 0, width: logScroll.contentSize.width, height: logH)
        logScroll.documentView = logView
        logScroll.isHidden = true     // appears with the first output
        content.addSubview(logScroll)

        status.font = .systemFont(ofSize: 11)
        status.textColor = c.dim
        status.lineBreakMode = .byTruncatingTail
        status.frame = NSRect(x: 20, y: 44, width: w - 40, height: 16)
        status.autoresizingMask = [.width, .maxYMargin]
        content.addSubview(status)

        done.role = .primary
        for (b, sel) in [(recheck, #selector(recheckClicked(_:))), (stack, #selector(stackClicked(_:))),
                         (done, #selector(doneClicked(_:)))] {
            b.target = self
            b.action = sel
            b.colors = c
            b.sizeToFit()
            content.addSubview(b)
        }
        let bw = { (b: NSButton) in b.intrinsicContentSize.width }
        done.frame = NSRect(x: w - 20 - bw(done), y: 12, width: bw(done), height: 26)
        done.autoresizingMask = [.minXMargin, .maxYMargin]
        recheck.frame = NSRect(x: 20, y: 12, width: bw(recheck), height: 26)
        recheck.autoresizingMask = [.maxXMargin, .maxYMargin]
        stack.frame = NSRect(x: recheck.frame.maxX + 8, y: 12, width: bw(stack), height: 26)
        stack.autoresizingMask = [.maxXMargin, .maxYMargin]

        window.contentView = content
        installKeys()
        updateIntro()
    }

    // MARK: text

    private func updateIntro() {
        let fallback: String
        switch AppInstall.state {
        case .notInstalled:
            fallback = "The app is running from the disk image. Move it to the Applications folder first — nothing can be saved from here."
        case .checkout:
            fallback = "~/.config/workspace-switcher is a developer checkout. The app uses its settings as they are; nothing was changed."
        case .failed(let why):
            fallback = "Setting up ~/.config/workspace-switcher failed: \(why)"
        default:
            fallback = "What this Mac needs. Notes, files, Jira, Confluence and AI work on their own; the Hyper hotkeys and window borders are an optional extra step."
        }
        let custom = Self.setting("intro", "")
        if case .ready = AppInstall.state, !custom.isEmpty { intro.stringValue = custom } else { intro.stringValue = fallback }
    }

    private func say(_ s: String, tone: PopupTone = .dim) {
        status.stringValue = s
        status.textColor = colors.tone(tone)
    }

    private static let logHeight: CGFloat = 130

    // the log takes the bottom of the list's space once there is output
    private func showLog() {
        guard logScroll.isHidden else { return }
        logScroll.isHidden = false
        var f = scroll.frame
        f.origin.y += Self.logHeight + 10
        f.size.height -= Self.logHeight + 10
        scroll.frame = f
        rebuildRows()
    }

    private func appendLog(_ s: String) {
        guard !s.isEmpty else { return }
        showLog()
        logView.textStorage?.append(NSAttributedString(string: s, attributes: [
            .font: NSFont.monospacedSystemFont(ofSize: 11, weight: .regular),
            .foregroundColor: colors.dim,
        ]))
        logView.scrollToEndOfDocument(nil)
    }

    // MARK: checks

    private func runChecks() {
        guard !busy else { return }
        busy = true
        say("Checking…")
        var args = ["--json", "--mode", isRepoBuild ? "repo" : "app"]
        if !isRepoBuild, let b = appBundlePath { args += ["--app", b] }
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            let r = AppInstall.runSync("preflight.sh", args)
            DispatchQueue.main.async {
                guard let self else { return }
                self.busy = false
                // stderr shares the pipe: the JSON object is the last line
                let line = r.out.split(separator: "\n").last(where: { $0.hasPrefix("{") }).map(String.init) ?? ""
                guard let obj = (try? JSONSerialization.jsonObject(with: Data(line.utf8))) as? [String: Any],
                      let list = obj["checks"] as? [[String: Any]] else {
                    self.say("The checks could not run: \(r.out.prefix(160))", tone: .danger)
                    return
                }
                self.checks = list.map { d in
                    let s = { (k: String) in d[k] as? String ?? "" }
                    return SetupCheck(id: s("id"), group: s("group"), level: s("level"), title: s("title"),
                                      detail: s("detail"), fix: s("fix"), action: s("action"),
                                      ok: d["ok"] as? Bool ?? false)
                }
                self.rebuildRows()
                self.summarize()
            }
        }
    }

    private func summarize() {
        let failed = checks.filter { !$0.ok && $0.required }
        let warned = checks.filter { !$0.ok && !$0.required && $0.group != "stack" }
        let stackMissing = checks.filter { !$0.ok && $0.group == "stack" }
        if let f = failed.first {
            say("\(f.title): \(f.detail)", tone: .danger)
        } else if !warned.isEmpty {
            say("Ready. \(warned.count) optional feature\(warned.count == 1 ? " is" : "s are") off — see the list.", tone: .warning)
        } else if !stackMissing.isEmpty {
            say("Ready. Hotkeys and window borders are not set up (optional).", tone: .dim)
        } else {
            say("Everything is in place.", tone: .success)
        }
        updateButtons()
    }

    private func updateButtons() {
        recheck.isEnabled = !busy
        done.isEnabled = !busy
        let stackMissing = checks.contains { !$0.ok && $0.group == "stack" }
        stack.isEnabled = !busy && stackMissing && AppInstall.state != .notInstalled
        stack.isHidden = !stackMissing
    }

    private static let groupTitles = ["core": "THIS MAC", "features": "FEATURES (OPTIONAL)",
                                      "stack": "HOTKEYS + BORDERS (OPTIONAL)", "dev": "BUILDING FROM SOURCE"]

    private func rebuildRows() {
        rows.subviews.forEach { $0.removeFromSuperview() }
        let c = colors
        let width = scroll.contentSize.width
        // sized BEFORE the rows go in: growing it afterwards would
        // autoresize them (buttons pushed out of view)
        rows.frame = NSRect(x: 0, y: 0, width: width, height: rows.frame.height)
        var y: CGFloat = 8
        var group = ""
        for (i, ck) in checks.enumerated() {
            if ck.group != group {
                group = ck.group
                let head = NSTextField(labelWithString: Self.groupTitles[group] ?? group.uppercased())
                head.font = .systemFont(ofSize: 10, weight: .semibold)
                head.textColor = c.dim
                head.frame = NSRect(x: 14, y: y + 6, width: width - 28, height: 14)
                head.autoresizingMask = [.width]
                rows.addSubview(head)
                y += 26
            }
            let glyph = NSTextField(labelWithString: ck.ok ? "✔" : (ck.required ? "✘" : "!"))
            glyph.font = .systemFont(ofSize: 13, weight: .bold)
            glyph.alignment = .center
            glyph.textColor = c.tone(ck.ok ? .success : (ck.required ? .danger : .warning))
            glyph.frame = NSRect(x: 12, y: y + 1, width: 18, height: 18)
            rows.addSubview(glyph)

            var right = width - 14
            if !ck.ok, !ck.action.isEmpty {
                let b = ThemedPushButton(title: fixTitle(ck.action), target: self, action: #selector(fixClicked(_:)))
                b.controlSize = .small
                b.colors = c
                b.tag = i
                if ck.required { b.role = .primary }
                let bw = b.intrinsicContentSize.width
                b.frame = NSRect(x: right - bw, y: y - 1, width: bw, height: 22)
                b.autoresizingMask = [.minXMargin]
                rows.addSubview(b)
                right -= bw + 10
            }
            let line = NSMutableAttributedString(string: ck.title, attributes: [
                .font: NSFont.systemFont(ofSize: 12, weight: .medium), .foregroundColor: c.text])
            if !ck.detail.isEmpty {
                line.append(NSAttributedString(string: "   " + ck.detail, attributes: [
                    .font: NSFont.systemFont(ofSize: 11), .foregroundColor: c.dim]))
            }
            let label = NSTextField(labelWithAttributedString: line)
            label.lineBreakMode = .byTruncatingTail
            label.frame = NSRect(x: 36, y: y + 2, width: right - 36, height: 17)
            label.autoresizingMask = [.width]
            label.toolTip = ck.detail
            rows.addSubview(label)
            y += 22
            if !ck.ok, !ck.fix.isEmpty {
                let fix = NSTextField(wrappingLabelWithString: ck.fix)
                fix.font = .systemFont(ofSize: 11)
                fix.textColor = c.dim
                fix.isSelectable = true
                let fh = ceil(fix.sizeThatFits(NSSize(width: right - 36, height: 200)).height)
                fix.frame = NSRect(x: 36, y: y, width: right - 36, height: fh)
                fix.autoresizingMask = [.width]
                rows.addSubview(fix)
                y += fh + 6
            }
            y += 4
        }
        rows.frame = NSRect(x: 0, y: 0, width: width, height: max(y + 8, scroll.contentSize.height))
        rows.autoresizingMask = [.width]
    }

    private func fixTitle(_ action: String) -> String {
        if action == "move-app" { return "Move to Applications" }
        if action == "setup-home" { return "Choose…" }
        if action == "stack" { return "Link" }
        if action.hasPrefix("brew:") || action.hasPrefix("cask:") { return "Install" }
        if action.hasPrefix("url:") { return "Open" }
        if action.hasPrefix("term:") { return "Copy Command" }
        return "Fix"
    }

    // MARK: actions

    @objc private func recheckClicked(_ sender: Any?) { runChecks() }
    @objc private func doneClicked(_ sender: Any?) { close() }

    @objc private func fixClicked(_ sender: NSButton) {
        guard checks.indices.contains(sender.tag), !busy else { return }
        let action = checks[sender.tag].action
        if action == "move-app" {
            moveToApplications()
        } else if action == "setup-home" {
            chooseHome()
        } else if action == "stack" {
            runSteps([("Linking the configs", "/bin/bash", [assetDir + "/bin/setup-home.sh", "stack"])])
        } else if action.hasPrefix("brew:") {
            brew(["install", String(action.dropFirst(5))])
        } else if action.hasPrefix("cask:") {
            brew(["install", "--cask", String(action.dropFirst(5))])
        } else if action.hasPrefix("url:"), let u = URL(string: String(action.dropFirst(4))) {
            NSWorkspace.shared.open(u)
        } else if action.hasPrefix("term:") {
            let cmd = String(action.dropFirst(5))
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(cmd, forType: .string)
            NSWorkspace.shared.open(URL(fileURLWithPath: "/System/Applications/Utilities/Terminal.app"))
            say("Copied “\(cmd)” — paste it in Terminal, then Check Again.")
        }
    }

    private var brewPath: String? {
        ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"].first { FileManager.default.isExecutableFile(atPath: $0) }
    }

    private func brew(_ args: [String]) {
        guard let b = brewPath else { noBrew(); return }
        runSteps([("brew " + args.joined(separator: " "), b, args)])
    }

    private func noBrew() {
        let cmd = "/bin/bash -c \"$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)\""
        ask(title: "Homebrew is not installed",
            text: "The hotkeys and window borders are built on AeroSpace and borders, which come from Homebrew. "
                + "Its installer needs your password, so it has to run in Terminal:\n\n\(cmd)\n\nRun it, then press Check Again.",
            buttons: ["Copy Command & Open Terminal", "Cancel"]) { [weak self] pick in
            guard pick == 0 else { return }
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(cmd, forType: .string)
            NSWorkspace.shared.open(URL(fileURLWithPath: "/System/Applications/Utilities/Terminal.app"))
            self?.say("Copied the Homebrew install command — paste it in Terminal.")
        }
    }

    // the opt-in step: brew packages, config links, services
    @objc private func stackClicked(_ sender: Any?) {
        guard !busy else { return }
        guard let b = brewPath else { noBrew(); return }
        let formulae = checks.filter { !$0.ok && $0.action.hasPrefix("brew:") && $0.group == "stack" }
            .map { String($0.action.dropFirst(5)) }
        let casks = checks.filter { !$0.ok && $0.action.hasPrefix("cask:") }.map { String($0.action.dropFirst(5)) }
        let installs = (formulae + casks).joined(separator: ", ")
        ask(title: "Set up the hotkeys and window borders?",
            text: (installs.isEmpty ? "" : "Installs with Homebrew: \(installs).\n\n")
                + "Links this app's AeroSpace and borders configs into ~/.config. "
                + "Real files already there are left alone and reported — nothing is moved or deleted.\n\n"
                + "Afterwards macOS asks once for Accessibility access for AeroSpace (System Settings ▸ Privacy & Security).",
            buttons: ["Set Up", "Cancel"]) { [weak self] pick in
            guard pick == 0, let self else { return }
            var steps: [(String, String, [String])] = []
            if !formulae.isEmpty { steps.append(("Installing \(formulae.joined(separator: ", "))", b, ["install"] + formulae)) }
            if !casks.isEmpty { steps.append(("Installing \(casks.joined(separator: ", "))", b, ["install", "--cask"] + casks)) }
            steps.append(("Linking the configs", "/bin/bash", [assetDir + "/bin/setup-home.sh", "stack"]))
            self.runSteps(steps)
        }
    }

    // run the steps one after another, output streamed into the log; stops
    // at the first failure, then re-checks
    private func runSteps(_ steps: [(String, String, [String])]) {
        guard let (title, exe, args) = steps.first else {
            busy = false
            runChecks()
            return
        }
        busy = true
        say(title + "…")
        appendLog("$ \((exe as NSString).lastPathComponent) \(args.joined(separator: " "))\n")
        let p = Process()
        p.executableURL = URL(fileURLWithPath: exe)
        p.arguments = args
        var env = ProcessInfo.processInfo.environment
        env["PATH"] = "/opt/homebrew/bin:/usr/local/bin:" + (env["PATH"] ?? "/usr/bin:/bin")
        env["HOMEBREW_NO_AUTO_UPDATE"] = "1"
        env["NONINTERACTIVE"] = "1"
        // brew's tap trust list lives under $XDG_CONFIG_HOME when the user's
        // shell sets it; launched from Finder we don't inherit that, and brew
        // then refuses the borders / aerospace taps
        let home = NSHomeDirectory(), fm = FileManager.default
        if env["XDG_CONFIG_HOME"] == nil, fm.fileExists(atPath: home + "/.config/homebrew/trust.json"),
           !fm.fileExists(atPath: home + "/.homebrew/trust.json") {
            env["XDG_CONFIG_HOME"] = home + "/.config"
        }
        p.environment = env
        let pipe = Pipe()
        p.standardOutput = pipe
        p.standardError = pipe
        pipe.fileHandleForReading.readabilityHandler = { [weak self] h in
            let d = h.availableData
            guard !d.isEmpty, let s = String(data: d, encoding: .utf8) else { return }
            DispatchQueue.main.async { self?.appendLog(s) }
        }
        p.terminationHandler = { [weak self] proc in
            pipe.fileHandleForReading.readabilityHandler = nil
            DispatchQueue.main.async {
                guard let self else { return }
                self.running = nil
                if proc.terminationStatus == 0 {
                    self.runSteps(Array(steps.dropFirst()))
                } else {
                    self.busy = false
                    self.say("\(title) failed (exit \(proc.terminationStatus)) — see the log below.", tone: .danger)
                    self.runChecks()
                }
            }
        }
        do {
            try p.run()
            running = p
        } catch {
            busy = false
            say("Cannot run \(exe): \(error.localizedDescription)", tone: .danger)
        }
    }

    // the home is a git checkout: keep it (nothing changes) or hand over
    private func chooseHome() {
        ask(title: "~/.config/workspace-switcher is a developer checkout",
            text: "Keep Using the Checkout: nothing changes — the app reads the checkout's commands.toml.\n\n"
                + "Switch to the Installed App: the checkout is kept (renamed to workspace-switcher.repo-<date>, or just unlinked), "
                + "your commands.toml, rules and configs are carried over, and the hotkeys point at this app.",
            buttons: ["Keep Using the Checkout", "Switch to the Installed App", "Cancel"]) { [weak self] pick in
            guard let self else { return }
            if pick == 0 {
                UserDefaults.standard.set(true, forKey: AppInstall.keepCheckoutKey)
                self.say("Keeping the developer checkout.")
            } else if pick == 1 {
                AppInstall.ensureHome(switchFromCheckout: true)
                if case .ready = AppInstall.state {
                    UserDefaults.standard.removeObject(forKey: AppInstall.keepCheckoutKey)
                    // userDir was the checkout when this process started
                    self.relaunch(appBundlePath ?? "")
                } else {
                    self.updateIntro()
                    self.runChecks()
                }
            }
        }
    }

    // copy the bundle into /Applications (the old copy goes to the Trash)
    // and start it from there
    private func moveToApplications() {
        guard let src = appBundlePath else { return }
        let fm = FileManager.default
        let name = (src as NSString).lastPathComponent
        var dir = "/Applications"
        if !fm.isWritableFile(atPath: dir) {
            dir = NSHomeDirectory() + "/Applications"
            try? fm.createDirectory(atPath: dir, withIntermediateDirectories: true)
        }
        let dst = dir + "/" + name
        do {
            if fm.fileExists(atPath: dst) {
                try fm.trashItem(at: URL(fileURLWithPath: dst), resultingItemURL: nil)
            }
            try fm.copyItem(atPath: src, toPath: dst)
        } catch {
            say("Could not copy the app to \(dir): \(error.localizedDescription)", tone: .danger)
            return
        }
        relaunch(dst)
    }

    private func relaunch(_ bundle: String) {
        guard !bundle.isEmpty else { return }
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/bin/sh")
        // wait for this process to go (one bundle id = one daemon), then open
        p.arguments = ["-c", "while kill -0 \(getpid()) 2>/dev/null; do sleep 0.1; done; /usr/bin/open -n \"$0\" --args setup", bundle]
        try? p.run()
        NSApp.terminate(nil)
    }

    // sheets, never app-modal alerts (those open hidden behind the popups)
    private func ask(title: String, text: String, buttons: [String], then: @escaping (Int) -> Void) {
        let a = NSAlert()
        a.messageText = title
        a.informativeText = text
        buttons.forEach { a.addButton(withTitle: $0) }
        a.beginSheetModal(for: window) { r in
            then(r.rawValue - NSApplication.ModalResponse.alertFirstButtonReturn.rawValue)
        }
    }

    // MARK: keys + window

    // accessory app, no Edit menu: route the edit shortcuts by hand (rule 1)
    private func installKeys() {
        monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] e in
            guard let self, self.window.isKeyWindow, self.window.attachedSheet == nil else { return e }
            if e.keyCode == 53 { if !self.busy { self.close() }; return nil }
            let mods = e.modifierFlags.intersection(.deviceIndependentFlagsMask)
            let cmd = mods.contains(.command), ctrl = mods.contains(.control)
            guard cmd || ctrl else { return e }
            let ed = (self.window.firstResponder as? NSText) ?? self.logView
            switch e.keyCode {
            case 8: ed.copy(nil)                          // Cmd+C / Ctrl+C
            case 0 where cmd: ed.selectAll(nil)           // Cmd+A
            case 13 where cmd: if !self.busy { self.close() }   // Cmd+W
            case 15 where cmd: self.runChecks()           // Cmd+R
            default: return e
            }
            return nil
        }
    }

    private func close() {
        window.close()
    }

    func windowDidResize(_ notification: Notification) {
        rebuildRows()
    }

    func windowWillClose(_ notification: Notification) {
        running?.terminate()
        if let m = monitor { NSEvent.removeMonitor(m) }
        monitor = nil
        Self.live = nil
        // first run: the window just closed is the only thing on screen —
        // show the app itself
        if case .ready(let fresh) = AppInstall.state, fresh {
            AppInstall.state = .ready(fresh: false)
            controller?.showCommand("files")
        }
    }
}
