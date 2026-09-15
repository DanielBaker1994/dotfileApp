import SwiftUI

// MARK: - Row list (scrollable, keyboard-navigable)

struct RowListView: View {
    @Bindable var state: PopupState
    var onAccept: (PopupRow) -> Void
    var onRowClick: (Int) -> Void
    var onRowDoubleClick: (Int) -> Void
    var onToggleTick: (Int) -> Void

    @Environment(\.popupColors) var colors

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(spacing: 0) {
                    ForEach(Array(state.rows.enumerated()), id: \.offset) { index, row in
                        RowView(
                            row: row,
                            index: index,
                            isSelected: index == state.selection,
                            isTicked: state.selectedIndices.contains(index),
                            matchRanges: [],
                            isTickable: state.selectableRows,
                            onTap: {
                                state.selection = index
                                if state.selectableRows && state.selectedIndices.isEmpty {
                                    // tick on first tap in selectable mode
                                }
                                onRowClick(index)
                            },
                            onDoubleTap: {
                                onRowDoubleClick(index)
                            },
                            onToggleTick: {
                                onToggleTick(index)
                            }
                        )
                        .id(index)
                    }
                }
                .onChange(of: state.selection) { _, _ in
                    // Keep selection visible
                    withAnimation(.easeOut(duration: 0.15)) {
                        proxy.scrollTo(state.selection, anchor: .center)
                    }
                }
            }
        }
        .onKeyPress(.upArrow) {
            guard state.selection > 0 else { return .ignored }
            state.selection -= 1
            return .handled
        }
        .onKeyPress(.downArrow) {
            guard state.selection < state.rows.count - 1 else { return .ignored }
            state.selection += 1
            return .handled
        }
        .onKeyPress(.return) {
            if state.selection < state.rows.count {
                onAccept(state.rows[state.selection])
            }
            return .handled
        }
        .onKeyPress(.space) {
            if state.selectableRows && state.selection < state.rows.count {
                onToggleTick(state.selection)
            }
            return .handled
        }
    }
}
