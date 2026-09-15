import AppKit
import Observation

// MARK: - PopupState (single source of truth for popup UI)

@Observable
final class PopupState {
    // Mode
    var mode: PopupMode = .list

    // List mode state
    var query = ""
    var selection = 0
    var rows: [PopupRow] = []
    var selectedIndices: Set<Int> = []

    // Tabs
    var tabs: [TabInfo] = []
    var selectedTab = 0

    // Filters
    var filterDims: [FilterDim] = []
    var filterSelections: [Int] = []  // index into each dim's values

    // Editor mode state
    var editorText = ""
    var editorReadOnly = false
    var editorSyntaxHighlighted = false

    // Header chrome
    var chromeHeaderTitle: String?
    var headerIcon: NSImage?
    var copyPathButtonLabel: String?
    var copyConfigButtonLabel: String?
    var itemCount: String?
    var headerButtons: [(label: String, id: Int)] = []

    // Status bar
    var statusText: String?
    var isStatusError = false

    // Voice recording
    var voiceState: VoiceState = .idle
    var voiceElapsed: TimeInterval = 0
    var voiceLevel: Float = 0
    var meterEnabled = false

    // Drawers
    var terminalShown = true
    var fileBrowserShown = false

    // Zoom
    var zoom: CGFloat = 1.0

    // Colors (from theme)
    var colors = PopupColors()

    // Config
    var width: CGFloat = 250
    var height: CGFloat = 420
    var maxHeight: CGFloat = 0
    var resizable = false
    var sticky = false
    var scrollableRows = false
    var selectableRows = false
    var showSearchBar = false
    var showFilters = false
    var showTabs = false
    var showCloseButton = false
    var dragHeader = false
    var titlePill = true
    var headerHeight: CGFloat = 30
    var headerColor: NSColor?
    var fontName: String?
    var searchWidthFraction: CGFloat = 0.8
    var maxRowStretch: CGFloat = 26
    var bodyMaxLines: Int = 5
    var terminalHeight: CGFloat = 240
    var fileBrowserHeight: CGFloat = 300
    var fileBrowserDefault = false

    // Computed
    var hasHeaderChrome: Bool {
        headerIcon != nil || chromeHeaderTitle != nil || !headerButtons.isEmpty
    }

    var effectiveHeight: CGFloat {
        var h = height
        if maxHeight > 0 { h = min(h, maxHeight) }
        return h
    }

    // MARK: - Mutating helpers

    func setRows(_ newRows: [PopupRow]) {
        rows = newRows
        selectedIndices = selectedIndices.filter { newRows.indices.contains($0) }
        if selection >= newRows.count {
            selection = max(0, newRows.count - 1)
        }
    }

    func setStatus(_ text: String?, isError: Bool = false) {
        statusText = text
        isStatusError = isError
    }

    func clearStatus() {
        statusText = nil
        isStatusError = false
    }

    func toggleSelection(_ index: Int) {
        guard rows.indices.contains(index) else { return }
        if selectedIndices.contains(index) {
            selectedIndices.remove(index)
        } else {
            selectedIndices.insert(index)
        }
    }

    func selectAll() {
        selectedIndices = Set(rows.indices)
    }

    func clearSelection() {
        selectedIndices = []
    }

    var copiedSelectionLabel: String? {
        guard selectableRows else { return nil }
        let n = selectedIndices.count
        return n == 0 ? "copy all" : "copy \(n)"
    }
}
