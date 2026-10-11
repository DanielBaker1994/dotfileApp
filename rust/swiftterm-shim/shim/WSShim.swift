// WSShim.swift — thin Swift shim exposing a SwiftTerm TerminalView to Rust.
// Compiled by build.rs against the pinned libSwiftTerm.a (PLAN-rust-port.md,
// Phase 0.7). Kept minimal on purpose: one NSView wrapper, a factory, a small
// start/send/font API, and one delegate hook the Rust side can drive.
import AppKit
import SwiftTerm

@objc(WSShimDelegate)
public protocol WSShimDelegate: AnyObject {
    func shimEvent(_ kind: String, value: String)
}

@objc(WSShim)
@MainActor
public final class WSShim: NSView, LocalProcessTerminalViewDelegate {
    @objc public weak var eventDelegate: WSShimDelegate?

    private let terminal: LocalProcessTerminalView
    private var exitCode: Int32 = -1

    @objc(makeTerminalWithFrame:)
    public static func makeTerminal(frame: NSRect) -> NSView {
        return WSShim(frame: frame)
    }

    public override init(frame: NSRect) {
        terminal = LocalProcessTerminalView(frame: NSRect(origin: .zero, size: frame.size))
        super.init(frame: frame)
        terminal.autoresizingMask = [.width, .height]
        terminal.processDelegate = self
        addSubview(terminal)
    }

    public required init?(coder: NSCoder) {
        terminal = LocalProcessTerminalView(frame: .zero)
        super.init(coder: coder)
        terminal.autoresizingMask = [.width, .height]
        terminal.processDelegate = self
        addSubview(terminal)
    }

    @objc public var terminalView: NSView { terminal }

    @objc(startWithExecutable:args:directory:)
    public func start(executable: String, args: [String], directory: String?) {
        // A relaunch reuses this view (Swift's `TerminalAutoRestart` restarts
        // the same `LocalProcessTerminalView`): clear the last exit code.
        exitCode = -1
        terminal.startProcess(executable: executable, args: args, currentDirectory: directory)
    }

    @objc(sendKeys:)
    public func sendKeys(_ text: String) {
        terminal.send(txt: text)
    }

    @objc(setFontWithName:size:)
    public func setFont(name: String, size: Double) {
        terminal.font = NSFont(name: name, size: CGFloat(size))
            ?? NSFont.monospacedSystemFont(ofSize: CGFloat(size), weight: .regular)
    }

    @objc public var terminalRunning: Bool { terminal.process?.running ?? false }
    @objc public var terminalExitCode: Int32 { exitCode }

    nonisolated public func sizeChanged(source: LocalProcessTerminalView, newCols: Int, newRows: Int) {
        emit("size", "\(newCols)x\(newRows)")
    }

    nonisolated public func setTerminalTitle(source: LocalProcessTerminalView, title: String) {
        emit("title", title)
    }

    nonisolated public func hostCurrentDirectoryUpdate(source: TerminalView, directory: String?) {
        emit("cwd", directory ?? "")
    }

    nonisolated public func processTerminated(source: TerminalView, exitCode: Int32?) {
        // SwiftTerm reports `nil` for a signal death; store 128 so the Rust
        // side still sees the exit edge (`-1` means running / unknown).
        let code = exitCode ?? 128
        Task { @MainActor in
            self.exitCode = code
            self.eventDelegate?.shimEvent("exit", value: String(code))
        }
    }

    nonisolated private func emit(_ kind: String, _ value: String) {
        Task { @MainActor in self.eventDelegate?.shimEvent(kind, value: value) }
    }
}
