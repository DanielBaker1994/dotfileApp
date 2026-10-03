import AppKit

// Full-size view of a note's picture: click an inline image (vim pane or
// native editor) and it opens in its own borderless panel. Esc closes it and
// hands the keyboard back; clicking anywhere else (another window or app)
// closes it too. A non-activating panel — the app is never re-activated, so
// the shared window and AeroSpace are untouched (like the tool panels).
final class ImagePopupPanel: NSPanel {
    var onEscape: (() -> Void)?
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }
    override func cancelOperation(_ sender: Any?) { onEscape?() }
    override func keyDown(with event: NSEvent) {
        if event.keyCode == 53 { onEscape?() } else { super.keyDown(with: event) }
    }
}

enum ImagePopup {
    private static var panel: ImagePopupPanel?
    private static var resignObserver: NSObjectProtocol?

    static func show(path: String, over parent: NSWindow?) {
        guard let img = NSImage(contentsOfFile: path),
              img.size.width > 0, img.size.height > 0 else { return }
        close()
        let screen = parent?.screen ?? NSScreen.main ?? NSScreen.screens[0]
        let room = screen.visibleFrame.insetBy(dx: 24, dy: 24)
        // natural size (points), shrunk to fit the screen, never upscaled
        let scale = min(1, room.width / img.size.width, room.height / img.size.height)
        let size = NSSize(width: floor(img.size.width * scale), height: floor(img.size.height * scale))
        let frame = NSRect(x: room.midX - size.width / 2, y: room.midY - size.height / 2,
                           width: size.width, height: size.height)

        let p = ImagePopupPanel(contentRect: frame,
                                styleMask: [.borderless, .nonactivatingPanel],
                                backing: .buffered, defer: false)
        p.isReleasedWhenClosed = false
        p.isFloatingPanel = true
        p.level = .floating
        p.hidesOnDeactivate = false
        p.hasShadow = true
        p.isOpaque = false
        p.backgroundColor = .clear
        p.collectionBehavior = [.moveToActiveSpace, .fullScreenAuxiliary]
        let iv = NSImageView(frame: NSRect(origin: .zero, size: size))
        iv.image = img
        iv.imageScaling = .scaleProportionallyUpOrDown
        iv.wantsLayer = true
        iv.layer?.cornerRadius = 8
        iv.layer?.masksToBounds = true
        p.contentView = iv
        p.setAccessibilityLabel((path as NSString).lastPathComponent)
        p.onEscape = { [weak parent] in
            close()
            parent?.makeKey()
        }
        // key lost = the user clicked elsewhere: just go away
        resignObserver = NotificationCenter.default.addObserver(
            forName: NSWindow.didResignKeyNotification, object: p, queue: .main
        ) { _ in close() }
        panel = p
        p.makeKeyAndOrderFront(nil)
    }

    static func close() {
        if let o = resignObserver { NotificationCenter.default.removeObserver(o) }
        resignObserver = nil
        let p = panel
        panel = nil
        p?.orderOut(nil)
    }
}
