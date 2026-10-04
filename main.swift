import AppKit
import Foundation

// Entry point (must be in main.swift for multi-file builds).

// Children (python, the launcher script) inherit this: an app install keeps
// its python inside the signed bundle, and a __pycache__ written there would
// break the signature.
setenv("PYTHONDONTWRITEBYTECODE", "1", 1)
// ... and the python side finds commands.toml beside its own folder, which
// in an app install is the bundle: point it at the user's file.
if !isRepoBuild { setenv("WS_COMMANDS_CONF", settings.commandsConfPath, 0) }

// ws-settings asks the app's own validation (no daemon, no AppInstall, no
// lock: pure functions over the config tables)
if CommandLine.arguments.count > 1 {
    switch CommandLine.arguments[1] {
    case "config-schema":
        print(configSchemaJSON())
        exit(0)
    case "config-check":
        exit(configCheckCLI(Array(CommandLine.arguments.dropFirst(2))))
    default: break
    }
}

// An app install (DMG): make sure ~/.config/workspace-switcher exists and
// points at THIS app before anything reads commands.toml from it (a marker
// check when nothing changed — a hotkey ping pays two file reads).
AppInstall.ensureHome()

// App settings ([app] section) first so the socket pings below use the
// configured names, not just the built-in defaults.
applyAppConfigFromDisk()

let cliArgs = CommandLine.arguments

// probe-only ping (workspace_switcher.sh sets this): ask the RUNNING daemon
// to open ANY command window (notes/jira/voice/health-checks/...). Exit 0
// when the message was delivered, 1 when no daemon is listening — NEVER fall
// through to app.run(), or the "ping" becomes a foreground daemon and the
// launcher script blocks forever.
if ProcessInfo.processInfo.environment["WS_PING_ONLY"] != nil {
    let name = cliArgs.count > 1 ? cliArgs[1] : settings.switcherWindowName
    exit(sendLaunchMessage(name) ? 0 : 1)
}

var openCommand: String? = nil
if cliArgs.count > 1 {
    switch cliArgs[1] {
    case "toggle":
        exit(sendToggle(name: settings.switcherWindowName) ? 0 : 1)
    case "jira-poll":
        // THE jira switch via the running daemon: on | off | toggle | setup,
        // or open the Jira Config window: dashboard
        let action = cliArgs.count > 2 ? cliArgs[2] : "toggle"
        guard ["on", "off", "toggle", "setup", "dashboard"].contains(action) else {
            FileHandle.standardError.write(Data("usage: workspace-switcher jira-poll on|off|toggle|setup|dashboard\n".utf8))
            exit(2)
        }
        let msg = action == "setup" ? "jira-setup" : action == "dashboard" ? "jira-dashboard" : "jira-poll-" + action
        if sendLaunchMessage(msg) { exit(0) }
        FileHandle.standardError.write(Data("workspace-switcher is not running\n".utf8))
        exit(1)
    case "reload", "restart":
        // re-read commands.toml / relaunch the daemon (ws-settings' apply
        // step). Never STARTS a daemon: nothing running = nothing to apply.
        guard let data = sendRequest(cliArgs[1], timeout: 15) else {
            FileHandle.standardError.write(Data("workspace-switcher is not running\n".utf8))
            exit(1)
        }
        let reply = String(decoding: data, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
        if reply.isEmpty {
            FileHandle.standardError.write(Data("the running workspace-switcher is too old for '\(cliArgs[1])': restart it once\n".utf8))
            exit(1)
        }
        print(reply)
        exit(reply.contains("\"ok\":true") ? 0 : 1)
    case "setup":
        // the Setup & Health Check window (running daemon, else this launch)
        if sendLaunchMessage("setup") { exit(0) }
        AppInstall.requested = true
    case "screenshot":
        // Hyper+X / Flameshot-style CLI: `screenshot [gui|full|screen] [flags]`
        // (ShotArgs). The words travel tab-separated (paths may hold spaces).
        let words = Array(cliArgs.dropFirst(2))
        let parsed: ShotArgs
        switch ShotArgs.parse(words) {
        case .success(let a): parsed = a
        case .failure(let p):
            FileHandle.standardError.write(Data("workspace-switcher screenshot: \(p.message)\n".utf8))
            exit(2)
        }
        let msg = (["screenshot"] + words).joined(separator: "\t")
        if parsed.wantsReply {
            // -r (PNG on stdout) / -g ("W H X Y"): wait for the user to finish
            guard let data = sendRequest(msg, timeout: 3600) else {
                FileHandle.standardError.write(Data("workspace-switcher is not running\n".utf8))
                exit(1)
            }
            if data.isEmpty { exit(1) }   // aborted
            FileHandle.standardOutput.write(data)
            exit(0)
        }
        if sendLaunchMessage(msg) { exit(0) }
        // no daemon: start it (as the hotkey modes do); it opens the capture
        if getppid() != 1 {
            let script = assetDir + "/bin/workspace_switcher.sh"
            if FileManager.default.isExecutableFile(atPath: script) {
                let argv: [UnsafeMutablePointer<CChar>?] = [strdup(script), strdup("screenshot"), nil]
                execv(script, argv)
            }
        }
        openCommand = "screenshot"
    case "pane-shot":
        // the focused herdr pane (scrollback + screen) as one tall image,
        // copied + saved by the daemon (PaneShot.swift). Prints the saved
        // path ("copied" when not saved); errors → stderr, exit 1.
        var words = Array(cliArgs.dropFirst(2))
        switch PaneShotArgs.parse(words) {
        case .success(let a):
            if a.file == "-", let i = words.firstIndex(of: "-") {
                // stdin → a temp file the daemon can read
                let tmp = NSTemporaryDirectory() + "pane-shot-\(getpid()).ansi"
                FileManager.default.createFile(atPath: tmp, contents: FileHandle.standardInput.readDataToEndOfFile())
                words[i] = tmp
            }
        case .failure(let p):
            FileHandle.standardError.write(Data("workspace-switcher pane-shot: \(p.message)\n".utf8))
            exit(2)
        }
        guard let data = sendRequest((["pane-shot"] + words).joined(separator: "\t"), timeout: 60) else {
            FileHandle.standardError.write(Data("workspace-switcher is not running\n".utf8))
            exit(1)
        }
        let line = String(decoding: data, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
        if let tmp = words.first(where: { $0.hasPrefix(NSTemporaryDirectory() + "pane-shot-") }) {
            try? FileManager.default.removeItem(atPath: tmp)
        }
        if line.isEmpty || line.hasPrefix("error: ") {
            FileHandle.standardError.write(Data("workspace-switcher pane-shot: \(line.isEmpty ? "no answer" : String(line.dropFirst(7)))\n".utf8))
            exit(1)
        }
        print(line)
        exit(0)
    case "compare" where cliArgs.count > 2:
        // `compare [--wait] [--title1 T] [--title2 T] LEFT [RIGHT]` (CompareWindow.swift):
        // paths made absolute HERE (the daemon's cwd is not ours); --wait
        // blocks until that session closes or the window hides (git difftool)
        var words: [String] = []
        var wait = false
        var i = 2
        var paths = 0
        while i < cliArgs.count {
            let w = cliArgs[i]
            if w == "--wait" { wait = true } else if (w == "--title1" || w == "--title2") && i + 1 < cliArgs.count {
                words += [w, cliArgs[i + 1].replacingOccurrences(of: "\t", with: " ")]
                i += 1
            } else if w.hasPrefix("-") && w.count > 1 {
                FileHandle.standardError.write(Data("usage: workspace-switcher compare [--wait] [--title1 T] [--title2 T] LEFT [RIGHT]\n".utf8))
                exit(2)
            } else {
                let abs = w.hasPrefix("/") ? w : FileManager.default.currentDirectoryPath + "/" + w
                words.append((abs as NSString).standardizingPath)
                paths += 1
            }
            i += 1
        }
        guard (1...2).contains(paths) else {
            FileHandle.standardError.write(Data("usage: workspace-switcher compare [--wait] [--title1 T] [--title2 T] LEFT [RIGHT]\n".utf8))
            exit(2)
        }
        let msg = (["compare"] + (wait ? ["--wait"] : []) + words).joined(separator: "\t")
        // no daemon: start it (LaunchServices) and hand the request over once it answers
        func deliver() -> Data? {
            if wait { return sendRequest(msg, timeout: 7 * 86400) }
            return sendLaunchMessage(msg) ? Data() : nil
        }
        var reply = deliver()
        if reply == nil {
            let bundle = Bundle.main.bundlePath
            let p = Process()
            p.executableURL = URL(fileURLWithPath: "/usr/bin/open")
            p.arguments = ["-g", bundle]
            try? p.run()
            p.waitUntilExit()
            let deadline = Date().addingTimeInterval(15)
            while reply == nil && Date() < deadline {
                usleep(200_000)
                if sendLaunchMessage("ping") { reply = deliver() }
            }
        }
        guard reply != nil else {
            FileHandle.standardError.write(Data("workspace-switcher is not running\n".utf8))
            exit(1)
        }
        exit(0)
    case let mode where SwitcherController.hotkeyModes.contains(mode):
        // THE hotkey path (aerospace runs this binary directly): a running
        // daemon gets a socket ping and does the rest (~20 ms). No daemon ->
        // hand off to the launcher script (build-if-stale + LaunchServices
        // launch, so mic/speech TCC attribute to the bundle)
        if sendLaunchMessage(cliArgs[1]) {
            exit(0)
        }
        // (a LaunchServices launch — the script's own `open -n -g`, INSTALL.sh
        // — has launchd as parent: that IS the daemon starting, never re-exec)
        if getppid() != 1 {
            // repo build: <root>/bin/…; app install: Contents/Resources/bin/…
            let script = assetDir + "/bin/workspace_switcher.sh"
            if FileManager.default.isExecutableFile(atPath: script) {
                let argv: [UnsafeMutablePointer<CChar>?] = [strdup(script), strdup(cliArgs[1]), nil]
                execv(script, argv)
            }
        }
        // cold start: Hyper+N ("window") opens the default view, files
        openCommand = cliArgs[1] == "window" ? "files" : cliArgs[1]
    default:
        break
    }
}
let showOnLaunch = cliArgs.count > 1 && cliArgs[1] == "show"

// ONE daemon (acquireDaemonLock): another one is running -> hand it this
// launch's request and exit. A daemon on its way out (build.sh / the
// launcher pkill it first) frees the lock within moments: then this one
// takes over.
if !acquireDaemonLock(waitUpTo: 0) {
    let arg = cliArgs.count > 1 ? cliArgs[1] : ""
    let deadline = Date().addingTimeInterval(3)
    while true {
        let delivered = arg == "show" ? sendToggle(name: settings.switcherWindowName)
            : sendLaunchMessage(openCommand == nil && arg != "setup" ? "ping" : arg)
        if delivered { exit(0) }
        if acquireDaemonLock(waitUpTo: 0.25) { break }
        if Date() >= deadline {
            FileHandle.standardError.write(Data("workspace-switcher: another daemon holds the lock but does not answer — not starting a second one\n".utf8))
            exit(1)
        }
    }
}

let app = NSApplication.shared
let delegate = AppDelegate(showOnLaunch: showOnLaunch, openCommand: openCommand)
app.delegate = delegate
app.run()