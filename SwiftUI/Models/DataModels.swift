import AppKit

// MARK: - Workspace & App data (pure data, no UI)

struct AppInfo: Equatable {
    let name: String
    let bundleID: String?
    let windowTitle: String?
}

struct WorkspaceInfo: Equatable {
    let id: String
    var apps: [AppInfo]
}

// MARK: - Command spec (commands.conf section -> typed config)

struct CommandSpec {
    enum Kind { case shell, note, list, output, files }

    let name: String
    let kind: Kind
    let windowName: String
    let chromeTitle: String
    let script: String?
    let paths: [String]
    let sources: [String]
    let root: String?
    let favorites: [String]
    let zoxideTop: Int
    let browserBackground: NSColor?
    let backgroundColor: NSColor?
    let tintAlpha: CGFloat?
    let primary: String?
    let content: String?
    let detail: String?
    let trailing: String?
    let body: String?
    let filter: [String]
    let filters: [String]
    let width: CGFloat
    let maxRows: Int
    let contentCap: Int
    let bodyLines: Int
    let pageSize: Int
    let copyFields: [String]
    let copyFormat: String
    let checkbox: Bool?
    let resize: Bool
    let drag: Bool
    let sticky: Bool
    let searchWidth: CGFloat
    let maxStretch: CGFloat
    let height: CGFloat
    let maxHeight: CGFloat
    let font: String?
    let headerColor: NSColor?
    let voice: Bool
    let terminal: Bool
    let terminalHeight: CGFloat
    let terminalDir: String?
    let terminalBackground: NSColor?
    let vimMode: Bool
    let vimBin: String
    let icon: NSImage?
    let saveDir: String

    init(name: String, kind: Kind = .shell, windowName: String? = nil,
         chromeTitle: String? = nil, script: String? = nil, paths: [String] = [],
         sources: [String] = [], root: String? = nil,
         favorites: [String] = [], zoxideTop: Int = 0,
         browserBackground: NSColor? = nil,
         backgroundColor: NSColor? = nil,
         tintAlpha: CGFloat? = nil,
         primary: String? = nil,
         content: String? = nil, detail: String? = nil, trailing: String? = nil,
         body: String? = nil, filter: [String] = [], filters: [String] = [],
         width: CGFloat = 0, maxRows: Int = 0, contentCap: Int = 0,
         bodyLines: Int = 0, pageSize: Int = 0, copyFields: [String] = [],
         copyFormat: String = "tsv", checkbox: Bool? = nil, resize: Bool = false,
         drag: Bool = true, sticky: Bool = true, searchWidth: CGFloat = 0,
         maxStretch: CGFloat = 0, height: CGFloat = 0, font: String? = nil,
         headerColor: NSColor? = nil, voice: Bool = false,
         terminal: Bool = false, terminalHeight: CGFloat = 240,
         terminalDir: String? = nil,
         terminalBackground: NSColor? = nil,
         vimMode: Bool = false, vimBin: String = "nvim",
         maxHeight: CGFloat = 0,
         icon: NSImage? = nil,
         saveDir: String = "/tmp/") {
        self.name = name
        self.kind = kind
        self.windowName = windowName ?? name
        self.chromeTitle = chromeTitle ?? (windowName ?? name)
        self.script = script
        self.paths = paths
        self.sources = sources
        self.root = root
        self.favorites = favorites
        self.zoxideTop = zoxideTop
        self.browserBackground = browserBackground
        self.backgroundColor = backgroundColor
        self.tintAlpha = tintAlpha
        self.primary = primary
        self.content = content
        self.detail = detail
        self.trailing = trailing
        self.body = body
        self.filter = filter
        self.filters = filters
        self.width = width
        self.maxRows = maxRows
        self.contentCap = contentCap
        self.bodyLines = bodyLines
        self.pageSize = pageSize
        self.copyFields = copyFields
        self.copyFormat = copyFormat
        self.checkbox = checkbox
        self.resize = resize
        self.drag = drag
        self.sticky = sticky
        self.searchWidth = searchWidth
        self.maxStretch = maxStretch
        self.height = height
        self.maxHeight = maxHeight
        self.font = font
        self.headerColor = headerColor
        self.voice = voice
        self.terminal = terminal
        self.terminalHeight = terminalHeight
        self.terminalDir = terminalDir
        self.terminalBackground = terminalBackground
        self.vimMode = vimMode
        self.vimBin = vimBin
        self.icon = icon
        self.saveDir = saveDir
    }
}

// MARK: - Icon rules

struct IconRule {
    let app: String
    let defaultIcon: NSImage?
    let titleMatches: [(match: String, icon: NSImage)]
}

// MARK: - Theme colors

struct PopupColors {
    var background: NSColor
    var border: NSColor
    var text: NSColor
    var dim: NSColor
    var highlight: NSColor
    var accent: NSColor

    init(background: NSColor = NSColor(srgbRed: 36/255, green: 39/255, blue: 58/255, alpha: 1),
         border: NSColor = NSColor(srgbRed: 159/255, green: 200/255, blue: 232/255, alpha: 1),
         text: NSColor = NSColor(srgbRed: 202/255, green: 211/255, blue: 245/255, alpha: 1),
         dim: NSColor = NSColor(srgbRed: 147/255, green: 154/255, blue: 183/255, alpha: 1),
         highlight: NSColor = NSColor(srgbRed: 63/255, green: 74/255, blue: 90/255, alpha: 1),
         accent: NSColor = NSColor(srgbRed: 85/255, green: 104/255, blue: 130/255, alpha: 1)) {
        self.background = background
        self.border = border
        self.text = text
        self.dim = dim
        self.highlight = highlight
        self.accent = accent
    }
}
