import AppKit
import Foundation

// Entry point (must be in main.swift for multi-file builds).

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
        // THE jira switch via the running daemon: on | off | toggle | setup
        let action = cliArgs.count > 2 ? cliArgs[2] : "toggle"
        guard ["on", "off", "toggle", "setup"].contains(action) else {
            FileHandle.standardError.write(Data("usage: workspace-switcher jira-poll on|off|toggle|setup\n".utf8))
            exit(2)
        }
        if sendLaunchMessage(action == "setup" ? "jira-setup" : "jira-poll-" + action) { exit(0) }
        FileHandle.standardError.write(Data("workspace-switcher is not running\n".utf8))
        exit(1)
    case "notes", "jira", "voice", "files":
        // a running daemon opens the window on a socket ping; otherwise
        // launch a daemon that starts straight into that window
        if sendLaunchMessage(cliArgs[1]) {
            exit(0)
        }
        openCommand = cliArgs[1]
    default:
        break
    }
}
let showOnLaunch = cliArgs.count > 1 && cliArgs[1] == "show"

let app = NSApplication.shared
let delegate = AppDelegate(showOnLaunch: showOnLaunch, openCommand: openCommand)
app.delegate = delegate
app.run()