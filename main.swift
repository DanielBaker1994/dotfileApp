import AppKit
import Foundation

setenv("PYTHONDONTWRITEBYTECODE", "1", 1)
if !isRepoBuild { setenv("WS_COMMANDS_CONF", settings.commandsConfPath, 0) }

if CommandLine.arguments.count > 1 {
    switch CommandLine.arguments[1] {
    case "config-schema":
        print(configSchemaJSON())
        exit(0)
    case "config-check":
        exit(configCheckCLI(Array(CommandLine.arguments.dropFirst(2))))
    case "prose":
        ProseProcess.run(Array(CommandLine.arguments.dropFirst(2)))
    default: break
    }
}

AppInstall.ensureHome()

applyAppConfigFromDisk()

let cliArgs = CommandLine.arguments

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
        let action = cliArgs.count > 2 ? cliArgs[2] : "toggle"
        guard ["on", "off", "toggle", "setup", "dashboard"].contains(action) else {
            FileHandle.standardError.write(Data("usage: kitchen-sink jira-poll on|off|toggle|setup|dashboard\n".utf8))
            exit(2)
        }
        let msg = action == "setup" ? "jira-setup" : action == "dashboard" ? "jira-dashboard" : "jira-poll-" + action
        if sendLaunchMessage(msg) { exit(0) }
        FileHandle.standardError.write(Data("kitchen-sink is not running\n".utf8))
        exit(1)
    case "reload", "restart":
        guard let data = sendRequest(cliArgs[1], timeout: 15) else {
            FileHandle.standardError.write(Data("kitchen-sink is not running\n".utf8))
            exit(1)
        }
        let reply = String(decoding: data, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
        if reply.isEmpty {
            FileHandle.standardError.write(Data("the running kitchen-sink is too old for '\(cliArgs[1])': restart it once\n".utf8))
            exit(1)
        }
        print(reply)
        exit(reply.contains("\"ok\":true") ? 0 : 1)
    case "setup":
        if sendLaunchMessage("setup") { exit(0) }
        AppInstall.requested = true
    case "screenshot":
        let words = Array(cliArgs.dropFirst(2))
        let parsed: ShotArgs
        switch ShotArgs.parse(words) {
        case .success(let a): parsed = a
        case .failure(let p):
            FileHandle.standardError.write(Data("kitchen-sink screenshot: \(p.message)\n".utf8))
            exit(2)
        }
        let msg = (["screenshot"] + words).joined(separator: "\t")
        if parsed.wantsReply {
            guard let data = sendRequest(msg, timeout: 3600) else {
                FileHandle.standardError.write(Data("kitchen-sink is not running\n".utf8))
                exit(1)
            }
            if data.isEmpty { exit(1) }
            FileHandle.standardOutput.write(data)
            exit(0)
        }
        if sendLaunchMessage(msg) { exit(0) }
        if getppid() != 1 {
            let script = assetDir + "/bin/kitchen_sink.sh"
            if FileManager.default.isExecutableFile(atPath: script) {
                let argv: [UnsafeMutablePointer<CChar>?] = [strdup(script), strdup("screenshot"), nil]
                execv(script, argv)
            }
        }
        openCommand = "screenshot"
    case "pane-shot":
        var words = Array(cliArgs.dropFirst(2))
        switch PaneShotArgs.parse(words) {
        case .success(let a):
            if a.file == "-", let i = words.firstIndex(of: "-") {
                let tmp = NSTemporaryDirectory() + "pane-shot-\(getpid()).ansi"
                FileManager.default.createFile(atPath: tmp, contents: FileHandle.standardInput.readDataToEndOfFile())
                words[i] = tmp
            }
        case .failure(let p):
            FileHandle.standardError.write(Data("kitchen-sink pane-shot: \(p.message)\n".utf8))
            exit(2)
        }
        guard let data = sendRequest((["pane-shot"] + words).joined(separator: "\t"), timeout: 60) else {
            FileHandle.standardError.write(Data("kitchen-sink is not running\n".utf8))
            exit(1)
        }
        let line = String(decoding: data, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
        if let tmp = words.first(where: { $0.hasPrefix(NSTemporaryDirectory() + "pane-shot-") }) {
            try? FileManager.default.removeItem(atPath: tmp)
        }
        if line.isEmpty || line.hasPrefix("error: ") {
            FileHandle.standardError.write(Data("kitchen-sink pane-shot: \(line.isEmpty ? "no answer" : String(line.dropFirst(7)))\n".utf8))
            exit(1)
        }
        print(line)
        exit(0)
    case "compare" where cliArgs.count > 2:
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
                FileHandle.standardError.write(Data("usage: kitchen-sink compare [--wait] [--title1 T] [--title2 T] LEFT [RIGHT]\n".utf8))
                exit(2)
            } else {
                let abs = w.hasPrefix("/") ? w : FileManager.default.currentDirectoryPath + "/" + w
                words.append((abs as NSString).standardizingPath)
                paths += 1
            }
            i += 1
        }
        guard (1...2).contains(paths) else {
            FileHandle.standardError.write(Data("usage: kitchen-sink compare [--wait] [--title1 T] [--title2 T] LEFT [RIGHT]\n".utf8))
            exit(2)
        }
        let msg = (["compare"] + (wait ? ["--wait"] : []) + words).joined(separator: "\t")
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
            FileHandle.standardError.write(Data("kitchen-sink is not running\n".utf8))
            exit(1)
        }
        exit(0)
    case "term":
        if sendLaunchMessage(cliArgs[1]) { exit(0) }
        FileHandle.standardError.write(Data("kitchen-sink is not running\n".utf8))
        exit(1)
    case let mode where SwitcherController.hotkeyModes.contains(mode):
        if sendLaunchMessage(cliArgs[1]) {
            exit(0)
        }
        if getppid() != 1 {
            let script = assetDir + "/bin/kitchen_sink.sh"
            if FileManager.default.isExecutableFile(atPath: script) {
                let argv: [UnsafeMutablePointer<CChar>?] = [strdup(script), strdup(cliArgs[1]), nil]
                execv(script, argv)
            }
        }
        openCommand = cliArgs[1] == "window" ? "notes" : cliArgs[1]
    default:
        break
    }
}
let showOnLaunch = cliArgs.count > 1 && cliArgs[1] == "show"

if !acquireDaemonLock(waitUpTo: 0) {
    let arg = cliArgs.count > 1 ? cliArgs[1] : ""
    let deadline = Date().addingTimeInterval(3)
    while true {
        let delivered = arg == "show" ? sendToggle(name: settings.switcherWindowName)
            : sendLaunchMessage(openCommand == nil && arg != "setup" ? "ping" : arg)
        if delivered { exit(0) }
        if acquireDaemonLock(waitUpTo: 0.25) { break }
        if Date() >= deadline {
            FileHandle.standardError.write(Data("kitchen-sink: another daemon holds the lock but does not answer — not starting a second one\n".utf8))
            exit(1)
        }
    }
}

let app = NSApplication.shared
let delegate = AppDelegate(showOnLaunch: showOnLaunch, openCommand: openCommand)
app.delegate = delegate
app.run()
