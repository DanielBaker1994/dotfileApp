import SwiftUI
import AppKit
import Foundation

// MARK: - Steps

enum InstallStep: Int, CaseIterable {
    case welcome = 0, deps, jira, install, done

    var title: String {
        switch self {
        case .welcome: return "Welcome"
        case .deps: return "Check dependencies"
        case .jira: return "Jira (optional)"
        case .install: return "Installing"
        case .done: return "Done"
        }
    }
    var subtitle: String {
        switch self {
        case .welcome: return "AeroSpace + SketchyBar + a notes/voice app — installed, wired, and granted permissions in a few clicks."
        case .deps: return "Everything below is required for the setup. Missing items can be installed right here."
        case .jira: return "The Jira window is optional. Enable it to show issues from your Jira Cloud site."
        case .install: return "Installing configs, building the app, granting mic + speech permissions, wiring the poll agent."
        case .done: return "All set. A few one-time system settings remain — see below."
        }
    }
}

// MARK: - Process helpers

@discardableResult
func runShell(_ cmd: String) -> (Int32, String) {
    let p = Process()
    p.executableURL = URL(fileURLWithPath: "/bin/bash")
    p.arguments = ["-lc", cmd]
    let pipe = Pipe()
    p.standardOutput = pipe
    p.standardError = pipe
    do { try p.run() } catch { return (127, "") }
    let data = pipe.fileHandleForReading.readDataToEndOfFile()
    p.waitUntilExit()
    return (p.terminationStatus, String(data: data, encoding: .utf8) ?? "")
}

func shellExists(_ name: String) -> Bool {
    let (rc, _) = runShell("command -v \(name) >/dev/null 2>&1")
    return rc == 0
}

func brewHas(_ formula: String, cask: Bool = false) -> Bool {
    let (rc, _) = runShell("brew list \(cask ? "--cask " : "")\(formula) >/dev/null 2>&1")
    return rc == 0
}

func streamProcess(_ executable: URL, _ args: [String],
                   onLine: @escaping (String) -> Void,
                   onDone: @escaping (Int32) -> Void) {
    let p = Process()
    p.executableURL = executable
    p.arguments = args
    let pipe = Pipe()
    p.standardOutput = pipe
    p.standardError = pipe
    var partial = ""
    pipe.fileHandleForReading.readabilityHandler = { fh in
        let data = fh.availableData
        guard !data.isEmpty else { return }
        partial += String(data: data, encoding: .utf8) ?? ""
        let lines = partial.split(separator: "\n", omittingEmptySubsequences: false)
        partial = lines.last.map { String($0) } ?? ""
        for l in lines.dropLast() { onLine(String(l)) }
    }
    p.terminationHandler = { proc in
        pipe.fileHandleForReading.readabilityHandler = nil
        if !partial.isEmpty { onLine(partial) }
        onDone(proc.terminationStatus)
    }
    do { try p.run() } catch { onDone(127) }
}

func stripANSI(_ s: String) -> String {
    s.replacingOccurrences(of: #"\e\[[0-9;]*m"#, with: "", options: .regularExpression)
}

// MARK: - Model

struct Dep: Identifiable {
    let id = UUID()
    let name: String
    let detail: String
    let install: String?   // "brew install X" / "brew install --cask X"; nil = manual
    var ok: Bool?
}

final class InstallModel: ObservableObject {
    @Published var step: InstallStep = .welcome
    @Published var deps: [Dep] = []
    @Published var depsWorking = false
    @Published var depsOutput: [String] = []
    @Published var jiraEnabled = false
    @Published var jiraSite = ""
    @Published var jiraEmail = ""
    @Published var jiraToken = ""
    @Published var installing = false
    @Published var installFailed = false
    @Published var installLog: [String] = []
    @Published var installFinished = false

    let repo: URL?

    init() {
        let fm = FileManager.default
        let candidates = [
            Bundle.main.bundleURL.deletingLastPathComponent(),
            URL(fileURLWithPath: NSHomeDirectory()).appendingPathComponent("workspace-switcher"),
        ]
        repo = candidates.first { fm.fileExists(atPath: $0.appendingPathComponent("setup.sh").path) }
    }

    // MARK: dependencies

    func checkDeps() {
        let items: [(String, String, String?, () -> Bool)] = [
            ("Homebrew", "package manager", nil, { shellExists("brew") }),
            ("AeroSpace", "tiling window manager", "brew install aerospace",
             { shellExists("aerospace") || brewHas("aerospace") }),
            ("SketchyBar", "status bar", "brew install sketchybar",
             { shellExists("sketchybar") || brewHas("sketchybar") }),
            ("JankyBorders", "focused-window border", "brew install borders",
             { brewHas("borders") }),
            ("jq", "JSON processing", "brew install jq", { shellExists("jq") }),
            ("Karabiner-Elements", "caps_lock → Hyper key", "brew install --cask karabiner-elements",
             { brewHas("karabiner-elements", cask: true) }),
            ("sketchybar-app-font", "app-glyph font", "brew install --cask font-sketchybar-app-font",
             { brewHas("font-sketchybar-app-font", cask: true) }),
            ("Xcode Command Line Tools", "Swift compiler (swiftc)", nil, { shellExists("swiftc") }),
        ]
        deps = items.map { Dep(name: $0.0, detail: $0.1, install: $0.2, ok: $0.3()) }
    }

    var missing: [Dep] { deps.filter { $0.ok == false } }

    func installMissingDeps() {
        depsWorking = true
        depsOutput = []
        let formulae = missing.compactMap { $0.install }
            .filter { !$0.contains("--cask") }
            .map { $0.replacingOccurrences(of: "brew install ", with: "") }
        let casks = missing.compactMap { $0.install }
            .filter { $0.contains("--cask") }
            .map { $0.replacingOccurrences(of: "brew install --cask ", with: "") }
        var cmds: [String] = []
        if !formulae.isEmpty { cmds.append("brew install \(formulae.joined(separator: " "))") }
        if !casks.isEmpty { cmds.append("brew install --cask \(casks.joined(separator: " "))") }
        guard !cmds.isEmpty else { depsWorking = false; return }
        streamProcess(URL(fileURLWithPath: "/bin/bash"), ["-lc", cmds.joined(separator: " && ")],
            onLine: { line in DispatchQueue.main.async { self.depsOutput.append(stripANSI(line)) } },
            onDone: { _ in DispatchQueue.main.async { self.depsWorking = false; self.checkDeps() } })
    }

    // MARK: jira

    func applyJiraConfig() {
        guard let repo = repo else { return }
        if jiraEnabled {
            if !jiraSite.isEmpty && !jiraEmail.isEmpty && !jiraToken.isEmpty {
                let dir = URL(fileURLWithPath: NSHomeDirectory()).appendingPathComponent(".config/jira")
                try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
                let content = """
                JIRA_SITE='\(jiraSite)'
                JIRA_EMAIL='\(jiraEmail)'
                JIRA_TOKEN='\(jiraToken)'
                JIRA_DEFAULT_PROJECT=''
                JIRA_MAX='25'
                JIRA_POLL_MARGIN='5'
                JIRA_POLL_STATE='\(NSHomeDirectory())/.cache/jira/poll-state'
                JIRA_POLL_OUT_DIR='\(NSHomeDirectory())/.cache/workspace-switcher/jira_json'
                JIRA_POLL_PROJECTS=''
                """
                let path = dir.appendingPathComponent("config").path
                try? content.write(toFile: path, atomically: true, encoding: .utf8)
                try? FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: path)
            }
            let conf = repo.appendingPathComponent("commands.conf").path
            _ = runShell("sed -i '' '/^\\[jira\\]/,/^$/s/^enabled = false$/enabled = true/' '\(conf)'")
        }
    }

    // MARK: karabiner (minimal: caps_lock -> Hyper only; the ONLY switcher
    // hotkey is Hyper+S, bound in aerospace.toml)

    func installKarabinerConfig() {
        let dir = URL(fileURLWithPath: NSHomeDirectory()).appendingPathComponent(".config/karabiner")
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let path = dir.appendingPathComponent("karabiner.json").path
        if FileManager.default.fileExists(atPath: path) {
            try? FileManager.default.removeItem(atPath: path + ".bak")
            try? FileManager.default.copyItem(atPath: path, toPath: path + ".bak")
        }
        let minimal = """
        {
            "profiles": [
                {
                    "name": "Default profile",
                    "selected": true,
                    "virtual_hid_keyboard": { "keyboard_type_v2": "ansi" },
                    "complex_modifications": {
                        "rules": [
                            {
                                "description": "Change caps_lock to Hyper (Cmd+Ctrl+Opt+Shift)",
                                "manipulators": [
                                    {
                                        "type": "basic",
                                        "from": { "key_code": "caps_lock", "modifiers": { "optional": ["any"] } },
                                        "to": [
                                            { "key_code": "left_shift", "modifiers": ["left_command", "left_control", "left_option"] },
                                            { "key_code": "left_shift", "modifiers": ["left_command", "left_control", "left_option"], "hold_down_milliseconds": 100 },
                                            { "key_code": "left_shift", "modifiers": ["left_command", "left_control", "left_option"], "hold_down_milliseconds": 100 }
                                        ],
                                        "to_if_alone": { "key_code": "caps_lock" },
                                        "to_if_held_down": { "key_code": "left_shift", "modifiers": ["left_command", "left_control", "left_option"] }
                                    }
                                ]
                            }
                        ]
                    }
                }
            ]
        }
        """
        try? minimal.write(toFile: path, atomically: true, encoding: .utf8)
        try? FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: path)
    }

    // MARK: install

    func startInstall() {
        guard let repo = repo else { return }
        applyJiraConfig()
        installKarabinerConfig()
        installing = true
        installFailed = false
        installFinished = false
        installLog = []
        streamProcess(repo.appendingPathComponent("setup.sh"), [],
            onLine: { line in DispatchQueue.main.async {
                self.installLog.append(stripANSI(line))
                if self.installLog.count > 300 { self.installLog.removeFirst(self.installLog.count - 300) }
            }},
            onDone: { rc in DispatchQueue.main.async {
                self.installing = false
                self.installFailed = rc != 0
                self.installFinished = rc == 0
            }})
    }

    func finish() {
        _ = runShell("aerospace reload-config >/dev/null 2>&1")
    }

    func launchNotes() {
        guard let repo = repo else { return }
        let app = repo.appendingPathComponent("workspace-switcher.app").path
        _ = runShell("open -n -g '\(app)' --args notes")
    }
}

// MARK: - Scaffold

struct WizardFrame<Content: View>: View {
    let stepIndex: Int
    let title: String
    let subtitle: String
    let content: Content

    init(stepIndex: Int, title: String, subtitle: String, @ViewBuilder content: () -> Content) {
        self.stepIndex = stepIndex
        self.title = title
        self.subtitle = subtitle
        self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 6) {
                Text(title).font(.system(size: 24, weight: .semibold))
                Text(subtitle).font(.callout).foregroundColor(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding(.horizontal, 28).padding(.top, 28).padding(.bottom, 14)
            HStack(spacing: 8) {
                ForEach(0..<InstallStep.allCases.count, id: \.self) { i in
                    Circle().fill(i <= stepIndex ? Color.accentColor : Color.secondary.opacity(0.25))
                        .frame(width: 8, height: 8)
                }
            }
            .padding(.horizontal, 28)
            Divider().padding(.top, 12)
            content.padding(28)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        }
        .frame(width: 600, height: 460)
    }
}

// MARK: - Screens

struct WelcomeView: View {
    @ObservedObject var model: InstallModel
    var body: some View {
        WizardFrame(stepIndex: model.step.rawValue, title: "Workspace Switcher", subtitle: InstallStep.welcome.subtitle) {
            VStack(alignment: .leading, spacing: 18) {
                HStack(spacing: 14) {
                    Image(systemName: "square.grid.2x2")
                        .font(.system(size: 44)).foregroundColor(.accentColor)
                    VStack(alignment: .leading, spacing: 3) {
                        Text("AeroSpace + SketchyBar + Notes").font(.headline)
                        Text("Tiling window manager, status bar, and a notes+voice app — installed and wired together.")
                            .font(.callout).foregroundColor(.secondary)
                    }
                }
                .padding(16)
                .background(RoundedRectangle(cornerRadius: 12).fill(Color.secondary.opacity(0.08)))
                if model.repo == nil {
                    Label("Couldn't find the installation files. Keep this app inside the dotfileApp folder.",
                          systemImage: "exclamationmark.triangle.fill")
                        .foregroundColor(.red).font(.callout)
                }
                Spacer()
                HStack {
                    Spacer()
                    Button("Get Started") { model.step = .deps; model.checkDeps() }
                        .buttonStyle(.borderedProminent).controlSize(.large)
                        .disabled(model.repo == nil)
                }
            }
        }
    }
}

struct DepsView: View {
    @ObservedObject var model: InstallModel
    var body: some View {
        WizardFrame(stepIndex: 1, title: "Check dependencies", subtitle: InstallStep.deps.subtitle) {
            VStack(alignment: .leading, spacing: 12) {
                ScrollView {
                    VStack(spacing: 6) {
                        ForEach(model.deps) { dep in
                            HStack(spacing: 10) {
                                Group {
                                    if dep.ok == true {
                                        Image(systemName: "checkmark.circle.fill").foregroundColor(.green)
                                    } else if dep.ok == false {
                                        Image(systemName: "xmark.circle.fill").foregroundColor(.red)
                                    } else {
                                        ProgressView().controlSize(.small)
                                    }
                                }
                                .frame(width: 18)
                                VStack(alignment: .leading, spacing: 1) {
                                    Text(dep.name).font(.body)
                                    Text(dep.detail).font(.caption).foregroundColor(.secondary)
                                }
                                Spacer()
                            }
                            .padding(.horizontal, 10).padding(.vertical, 6)
                            .background(RoundedRectangle(cornerRadius: 8).fill(Color.secondary.opacity(0.06)))
                        }
                    }
                }
                if model.depsWorking {
                    Text("Installing…").font(.caption).foregroundColor(.secondary)
                    ScrollView {
                        Text(model.depsOutput.joined(separator: "\n"))
                            .font(.caption2.monospaced()).foregroundColor(.secondary)
                    }
                    .frame(height: 90)
                    .padding(6)
                    .background(RoundedRectangle(cornerRadius: 8).fill(Color.secondary.opacity(0.06)))
                }
                Spacer()
                HStack {
                    if !model.missing.isEmpty && !model.depsWorking {
                        Button("Install missing (\(model.missing.count))") { model.installMissingDeps() }
                            .buttonStyle(.bordered).controlSize(.large)
                    }
                    Spacer()
                    Button("Back") { model.step = .welcome }.buttonStyle(.bordered).controlSize(.large)
                    Button("Continue") { model.step = .jira }
                        .buttonStyle(.borderedProminent).controlSize(.large)
                }
            }
        }
    }
}

struct JiraView: View {
    @ObservedObject var model: InstallModel
    var body: some View {
        WizardFrame(stepIndex: 2, title: "Jira (optional)", subtitle: InstallStep.jira.subtitle) {
            VStack(alignment: .leading, spacing: 14) {
                Toggle(isOn: $model.jiraEnabled) {
                    Text("Enable the Jira window").font(.headline)
                }
                .toggleStyle(.switch)
                if model.jiraEnabled {
                    VStack(alignment: .leading, spacing: 8) {
                        TextField("https://your-org.atlassian.net", text: $model.jiraSite)
                            .textFieldStyle(.roundedBorder)
                        TextField("you@company.com", text: $model.jiraEmail)
                            .textFieldStyle(.roundedBorder)
                        SecureField("API token (Atlassian → API tokens)", text: $model.jiraToken)
                            .textFieldStyle(.roundedBorder)
                        Text("Stored in ~/.config/jira/config (chmod 600). Can be changed later.")
                            .font(.caption).foregroundColor(.secondary)
                    }
                }
                Spacer()
                HStack {
                    Spacer()
                    Button("Back") { model.step = .deps }.buttonStyle(.bordered).controlSize(.large)
                    Button("Continue") { model.step = .install; model.startInstall() }
                        .buttonStyle(.borderedProminent).controlSize(.large)
                }
            }
        }
    }
}

struct InstallView: View {
    @ObservedObject var model: InstallModel
    var body: some View {
        WizardFrame(stepIndex: 3, title: "Installing", subtitle: InstallStep.install.subtitle) {
            VStack(alignment: .leading, spacing: 12) {
                HStack(spacing: 10) {
                    if model.installing {
                        ProgressView().controlSize(.small)
                        Text("Running setup…").font(.callout)
                    } else if model.installFailed {
                        Label("Installation failed — check the log below.",
                              systemImage: "exclamationmark.triangle.fill").foregroundColor(.red)
                    } else {
                        Label("Installation complete.", systemImage: "checkmark.circle.fill").foregroundColor(.green)
                    }
                }
                ScrollView {
                    Text(model.installLog.joined(separator: "\n"))
                        .font(.caption2.monospaced()).foregroundColor(.secondary)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .frame(maxHeight: .infinity)
                .padding(8)
                .background(RoundedRectangle(cornerRadius: 8).fill(Color.secondary.opacity(0.06)))
                if !model.installing {
                    HStack {
                        Spacer()
                        if model.installFailed {
                            Button("Retry") { model.startInstall() }.buttonStyle(.borderedProminent).controlSize(.large)
                        } else {
                            Button("Finish") { model.finish(); model.step = .done }
                                .buttonStyle(.borderedProminent).controlSize(.large)
                        }
                    }
                }
            }
        }
    }
}

struct DoneView: View {
    @ObservedObject var model: InstallModel
    var body: some View {
        WizardFrame(stepIndex: 4, title: "Done", subtitle: InstallStep.done.subtitle) {
            VStack(alignment: .leading, spacing: 16) {
                HStack(spacing: 12) {
                    Image(systemName: "checkmark.seal.fill")
                        .font(.system(size: 40)).foregroundColor(.green)
                    VStack(alignment: .leading, spacing: 2) {
                        Text("Installation complete").font(.title3.weight(.semibold))
                        Text("The notes+voice window is running. One-time settings below.")
                            .font(.callout).foregroundColor(.secondary)
                    }
                }
                VStack(alignment: .leading, spacing: 8) {
                    stepRow("Karabiner-Elements: open it, allow the system extension, confirm caps_lock = Hyper", "keyboard")
                    stepRow("System Settings → Privacy & Security: Accessibility for AeroSpace + Karabiner-Elements", "lock.shield")
                    stepRow("System Settings → Keyboard → Dictation ON (for voice notes)", "mic")
                    stepRow("Hyper+S opens the switcher — bound in AeroSpace (reloaded automatically)", "command")
                }
                Spacer()
                HStack {
                    Spacer()
                    Button("Launch Notes") { model.launchNotes() }.buttonStyle(.bordered).controlSize(.large)
                    Button("Quit") { NSApp.terminate(nil) }.buttonStyle(.borderedProminent).controlSize(.large)
                }
            }
        }
    }

    func stepRow(_ text: String, _ icon: String) -> some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: icon).foregroundColor(.accentColor).frame(width: 18)
            Text(text).font(.callout)
        }
    }
}

// MARK: - App

struct ContentView: View {
    @ObservedObject var model: InstallModel
    var body: some View {
        Group {
            switch model.step {
            case .welcome: WelcomeView(model: model)
            case .deps: DepsView(model: model)
            case .jira: JiraView(model: model)
            case .install: InstallView(model: model)
            case .done: DoneView(model: model)
            }
        }
    }
}

final class AppDelegate: NSObject, NSApplicationDelegate {
    private var window: NSWindow!

    func applicationDidFinishLaunching(_ notification: Notification) {
        let model = InstallModel()
        let hosting = NSHostingController(rootView: ContentView(model: model))
        window = NSWindow(contentViewController: hosting)
        window.title = "Workspace Switcher Installer"
        window.styleMask = [.titled, .closable]
        window.isReleasedWhenClosed = false
        window.center()
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }
    func applicationSupportsSecureRestorableState(_ app: NSApplication) -> Bool { true }
}

let app = NSApplication.shared
let delegate = AppDelegate()
app.delegate = delegate
app.run()