import AppKit
import Foundation

// Entry point (must be in main.swift for multi-file builds).

// App settings ([app] section) first so the socket pings below use the
// configured names, not just the built-in defaults.
applyAppConfigFromDisk()

let cliArgs = CommandLine.arguments
var openCommand: String? = nil
if cliArgs.count > 1 {
    switch cliArgs[1] {
    case "toggle":
        exit(sendToggle(name: settings.switcherWindowName) ? 0 : 1)
    case "notes", "jira", "voice":
        // a running daemon opens the window on a socket ping; otherwise
        // launch a daemon that starts straight into that window
        if sendLaunchMessage(cliArgs[1]) {
            exit(0)
        }
        // probe-only ping (workspace_switcher.sh sets this): never fall
        // through to app.run() here or the "ping" becomes a FOREGROUND
        // daemon and the launcher script blocks forever. The launcher
        // cold-starts via LaunchServices instead.
        if ProcessInfo.processInfo.environment["WS_PING_ONLY"] != nil {
            exit(1)
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