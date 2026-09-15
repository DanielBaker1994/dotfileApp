import SwiftUI

// MARK: - Tab bar (pill tabs, wrapping, + button, close)

struct TabBarView: View {
    @Bindable var state: PopupState
    var onTabSelect: (Int) -> Void
    var onAddTab: () -> Void
    var onCloseTab: (Int) -> Void
    var onTabCopyPath: (Int) -> Void
    var showAddButton: Bool

    var body: some View {
        if state.tabs.isEmpty { return AnyView(EmptyView()) }

        return AnyView(
            LazyVGrid(columns: tabGridColumns, spacing: 6 * state.zoom) {
                if showAddButton {
                    TabPill(title: "+", isSelected: false, isAddButton: true) {
                        onAddTab()
                    }
                }

                ForEach(Array(state.tabs.enumerated()), id: \.element.id) { index, tab in
                    TabPill(title: tab.title, isSelected: index == state.selectedTab) {
                        onTabSelect(index)
                    } onClose: {
                        onCloseTab(index)
                    } onCopyPath: {
                        onTabCopyPath(index)
                    }
                }
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 4)
        )
    }

    private var tabGridColumns: [GridItem] {
        [GridItem(.adaptive(minimum: 60, maximum: 200), spacing: 6)]
    }
}

struct TabPill: View {
    let title: String
    let isSelected: Bool
    var isAddButton: Bool = false
    var onAction: () -> Void = {}
    var onClose: () -> Void = {}
    var onCopyPath: () -> Void = {}

    @State private var isHoveringClose = false

    @Environment(\.popupColors) var colors

    var body: some View {
        Button(action: onAction) {
            HStack(spacing: 4) {
                Text(title)
                    .font(.system(size: 10.5, weight: .semibold))
                    .foregroundColor(isSelected ? colors.text : colors.dim)
                    .lineLimit(1)

                if !isAddButton, !title.isEmpty {
                    CloseButton(isVisible: isHoveringClose) {
                        onClose()
                    }
                }
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 4)
            .frame(minWidth: 30)
            .background(
                RoundedRectangle(cornerRadius: 6)
                    .fill(tabBackground)
            )
            .overlay(
                RoundedRectangle(cornerRadius: 6)
                    .stroke(isSelected ? colors.border.opacity(0.8) : .clear, lineWidth: 1)
            )
            .contextMenu {
                if !isAddButton {
                    Button("Copy Path") {
                        onCopyPath()
                    }
                }
            }
        }
        .buttonStyle(.plain)
        .onHover { hovering in
            withAnimation(.easeOut(duration: 0.15)) {
                isHoveringClose = hovering && !isAddButton
            }
        }
    }

    private var tabBackground: Color {
        if isAddButton { return Color(colors.accent.withAlphaComponent(0.5)) }
        if isSelected { return Color(colors.accent.withAlphaComponent(0.7)) }
        return Color(colors.highlight.withAlphaComponent(0.3))
    }
}

struct CloseButton: View {
    let isVisible: Bool
    let action: () -> Void

    @Environment(\.popupColors) var colors

    var body: some View {
        if isVisible {
            Button(action: action) {
                Text("✕")
                    .font(.system(size: 7.5, weight: .semibold))
                    .foregroundColor(colors.text.opacity(0.75))
                    .frame(width: 12, height: 12)
                    .background(
                        RoundedRectangle(cornerRadius: 3)
                            .fill(colors.background.opacity(0.85))
                            .overlay(
                                RoundedRectangle(cornerRadius: 3)
                                    .stroke(colors.dim.opacity(0.6), lineWidth: 0.8)
                            )
                    )
            }
            .buttonStyle(.plain)
        }
    }
}
