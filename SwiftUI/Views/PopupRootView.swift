import SwiftUI

// MARK: - PopupRootView (combines all content for the popup)

struct PopupRootView: View {
    @Bindable var state: PopupState
    var onAccept: (PopupRow) -> Void
    var onRowClick: (Int) -> Void
    var onRowDoubleClick: (Int) -> Void
    var onToggleTick: (Int) -> Void
    var onIconClick: () -> Void
    var onHeaderButtonClick: (Int) -> Void
    var onRecord: () -> Void
    var onPause: () -> Void
    var onCopyPath: () -> Void
    var onCopyConfig: () -> Void
    var onTabSelect: (Int) -> Void
    var onAddTab: () -> Void
    var onCloseTab: (Int) -> Void
    var onTabCopyPath: (Int) -> Void
    var onFilterChange: (Int, Int) -> Void

    var body: some View {
        VStack(spacing: 0) {
            // Chrome header
            ChromeView(
                state: state,
                onIconClick: onIconClick,
                onHeaderButtonClick: onHeaderButtonClick,
                onRecord: onRecord,
                onPause: onPause,
                onCopyPath: onCopyPath,
                onCopyConfig: onCopyConfig
            )

            // Search field (list mode only)
            if state.mode == .list && state.showSearchBar {
                SearchFieldView(state: state) {}
            }

            // Filter bar
            if state.showFilters {
                FilterBarView(state: state, onFilterChange: onFilterChange)
            }

            // Tab bar
            if state.showTabs {
                TabBarView(
                    state: state,
                    onTabSelect: onTabSelect,
                    onAddTab: onAddTab,
                    onCloseTab: onCloseTab,
                    onTabCopyPath: onTabCopyPath,
                    showAddButton: true
                )
            }

            // Content
            if state.mode == .list {
                RowListView(
                    state: state,
                    onAccept: onAccept,
                    onRowClick: onRowClick,
                    onRowDoubleClick: onRowDoubleClick,
                    onToggleTick: onToggleTick
                )
            } else {
                // Editor placeholder — real editor uses NSTextView via NSHostingView bridge
                EditorPlaceholderView(state: state)
            }

            // Status bar
            if let statusText = state.statusText {
                StatusBarView(text: statusText, isError: state.isStatusError)
            }
        }
        .environment(\.popupColors, state.colors)
        .environment(\.popupZoom, state.zoom)
    }
}

// MARK: - Editor placeholder (will be replaced with NSTextView bridge)

struct EditorPlaceholderView: View {
    @Bindable var state: PopupState

    @Environment(\.popupColors) var colors

    var body: some View {
        ScrollView {
            Text(state.editorText)
                .font(.system(size: 13 * state.zoom))
                .foregroundColor(colors.text)
                .textSelection(.enabled)
                .padding(12)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}
