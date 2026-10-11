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

    /// The cell size in points SwiftTerm actually laid the grid out with,
    /// derived from the terminal's current dimensions and the view's bounds.
    /// The notes surface needs the real height (not the font's nominal
    /// metrics) to position the inline-image rows over the pane. `(0, 0)`
    /// until the grid has been sized.
    @objc public var cellWidth: Double { Double(cellSizePoints().width) }
    @objc public var cellHeight: Double { Double(cellSizePoints().height) }

    private func cellSizePoints() -> NSSize {
        let dims = terminal.terminalDimensions
        guard dims.cols > 0, dims.rows > 0 else { return .zero }
        let size = terminal.bounds.size
        guard size.width > 0, size.height > 0 else { return .zero }
        return NSSize(width: size.width / CGFloat(dims.cols),
                      height: size.height / CGFloat(dims.rows))
    }

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

    /// Theme the terminal: background (clear → transparent, Swift's vim
    /// pane), foreground, and the 16-color ANSI palette (`ansiPalette`).
    @objc(setColorsBackground:foreground:palette:)
    public func setColors(background: NSColor, foreground: NSColor, palette: [NSColor]) {
        terminal.nativeBackgroundColor = background
        terminal.nativeForegroundColor = foreground
        let clear = (background.usingColorSpace(.sRGB) ?? background).alphaComponent < 0.01
        terminal.wantsLayer = true
        terminal.layer?.backgroundColor = clear ? NSColor.clear.cgColor : background.cgColor
        if palette.count == 16 {
            terminal.installColors(palette.map { c in
                let s = c.usingColorSpace(.sRGB) ?? c
                return SwiftTerm.Color(red: UInt16(s.redComponent * 65535),
                                       green: UInt16(s.greenComponent * 65535),
                                       blue: UInt16(s.blueComponent * 65535))
            })
        }
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
