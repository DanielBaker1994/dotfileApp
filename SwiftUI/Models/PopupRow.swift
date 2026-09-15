import AppKit

// MARK: - Row model (protocol + concrete types)

protocol PopupRow {
    var title: String { get }
    var icons: [NSImage] { get }
    var trailing: String? { get }
    var content: String? { get }
    var detail: String? { get }
    var body: String? { get }
    var searchText: String { get }
    var loadMore: Bool { get }
    var fields: [String: String] { get }
}

extension PopupRow {
    var icons: [NSImage] { [] }
    var trailing: String? { nil }
    var content: String? { nil }
    var detail: String? { nil }
    var body: String? { nil }
    var searchText: String { title }
    var loadMore: Bool { false }
    var fields: [String: String] { [:] }
}

// MARK: - Concrete row types

struct WorkspaceRow: PopupRow {
    let title: String
    let icons: [NSImage]
    let trailing: String?

    init(id: String, icons: [NSImage], trailing: String?) {
        self.title = id
        self.icons = icons
        self.trailing = trailing
    }
}

struct CommandRow: PopupRow {
    let title: String
    let command: CommandSpec
    let icons: [NSImage] = []
    let trailing: String? = nil

    init(_ c: CommandSpec) {
        title = "> \(c.name)"
        command = c
    }
}

struct FieldRow: PopupRow {
    let title: String
    let content: String?
    let trailing: String?
    let detail: String?
    let body: String?
    let searchText: String
    let fields: [String: String]
    let icons: [NSImage] = []

    var loadMore: Bool { fields["__loadmore"] != nil }
}

// MARK: - Filter dimension

struct FilterDim: Equatable {
    let key: String
    let label: String
    let values: [String]     // "All" is at index 0
    let valueLabels: [String] // display titles parallel to values
}

// MARK: - Tab info

struct TabInfo: Equatable, Identifiable {
    let id: String            // unique identifier (file path or source path)
    let title: String         // display name (usually filename)
}

// MARK: - Popup mode

enum PopupMode: Equatable {
    case list       // workspace switcher / searchable list
    case editor     // note / output / detail editor
}

// MARK: - Voice recorder state

enum VoiceState: Int {
    case idle = 0
    case recording = 1
    case paused = 2
    case transcribing = 3
}
