// ks_droptarget X Y W H: a window (Cocoa frame) that logs dropped file paths to /tmp/drops.log.
// Build: swiftc -O -target arm64-apple-macos14 Tests/tools/ks_droptarget.swift -o ks-droptarget
import AppKit
class V: NSView {
  override init(frame: NSRect) { super.init(frame: frame); registerForDraggedTypes([.fileURL]) }
  required init?(coder: NSCoder) { fatalError() }
  override func draw(_ r: NSRect) { NSColor.systemTeal.setFill(); r.fill()
    ("DROP HERE" as NSString).draw(at: NSPoint(x: 20, y: 20), withAttributes: [.font: NSFont.boldSystemFont(ofSize: 24)]) }
  override func draggingEntered(_ s: NSDraggingInfo) -> NSDragOperation { .copy }
  override func performDragOperation(_ s: NSDraggingInfo) -> Bool {
    let urls = s.draggingPasteboard.readObjects(forClasses: [NSURL.self]) as? [URL] ?? []
    let line = urls.map { $0.path }.joined(separator: "\n") + "\n"
    if let h = FileHandle(forWritingAtPath: "/tmp/drops.log") { h.seekToEndOfFile(); h.write(line.data(using: .utf8)!) }
    else { try? line.write(toFile: "/tmp/drops.log", atomically: true, encoding: .utf8) }
    return true }
}
let a = CommandLine.arguments.dropFirst().compactMap { Double($0) }
let app = NSApplication.shared; app.setActivationPolicy(.accessory)
let w = NSWindow(contentRect: NSRect(x: a[0], y: a[1], width: a[2], height: a[3]), styleMask: [.borderless], backing: .buffered, defer: false)
w.level = .floating; w.contentView = V(frame: NSRect(x: 0, y: 0, width: a[2], height: a[3])); w.orderFrontRegardless()
app.run()
