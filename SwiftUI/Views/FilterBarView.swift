import SwiftUI

// MARK: - Filter bar (dropdown pills for filter dimensions)

struct FilterBarView: View {
    @Bindable var state: PopupState
    var onFilterChange: (Int, Int) -> Void

    var body: some View {
        if state.filterDims.isEmpty { return AnyView(EmptyView()) }

        return AnyView(
            HStack(spacing: 1) {
                ForEach(Array(state.filterDims.enumerated()), id: \.element.key) { index, dim in
                    FilterPill(
                        dim: dim,
                        selection: state.filterSelections.indices.contains(index) ? state.filterSelections[index] : 0,
                        colors: state.colors,
                        zoom: state.zoom
                    ) { newSelection in
                        onFilterChange(index, newSelection)
                    }
                }
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 2)
        )
    }
}

struct FilterPill: View {
    let dim: FilterDim
    let selection: Int
    let colors: PopupColors
    let zoom: CGFloat
    let onSelect: (Int) -> Void

    @State private var showMenu = false

    var body: some View {
        Button(action: { showMenu = true }) {
            HStack(spacing: 4) {
                Text(currentLabel)
                    .font(.system(size: 10.5, weight: .semibold))
                    .foregroundColor(selection > 0 ? colors.text : colors.dim)
                    .lineLimit(1)
            }
            .padding(.horizontal, 12 * zoom)
            .padding(.vertical, 4 * zoom)
        }
        .buttonStyle(.plain)
        .background(
            RoundedRectangle(cornerRadius: 6 * zoom)
                .fill(selection > 0 ? colors.accent.opacity(0.35) : .clear)
        )
        .overlay(
            RoundedRectangle(cornerRadius: 6 * zoom)
                .stroke(selection > 0 ? colors.border.opacity(0.85) : .clear, lineWidth: 1)
        )
        .popover(isPresented: $showMenu, arrowEdge: .bottom) {
            FilterMenu(dim: dim, selection: selection, onSelect: onSelect)
                .frame(minWidth: 150)
                .padding(8)
        }
    }

    private var currentLabel: String {
        let sel = dim.values.indices.contains(selection) ? dim.values[selection] : "All"
        let display = dim.valueLabels.indices.contains(selection) && !dim.valueLabels[selection].isEmpty
            ? dim.valueLabels[selection]
            : sel
        return "\(dim.label): \(display) ▾"
    }
}

struct FilterMenu: View {
    let dim: FilterDim
    let selection: Int
    let onSelect: (Int) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            ForEach(Array(dim.values.enumerated()), id: \.offset) { index, value in
                let display = dim.valueLabels.indices.contains(index) && !dim.valueLabels[index].isEmpty
                    ? dim.valueLabels[index]
                    : value

                Button(action: {
                    onSelect(index)
                }) {
                    HStack {
                        Text(display)
                            .font(.system(size: 12))
                        Spacer()
                        if index == selection {
                            Image(systemName: "checkmark")
                                .font(.system(size: 10))
                        }
                    }
                    .frame(maxWidth: .infinity)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 4)
                }
                .buttonStyle(.plain)
                .background(index == selection ? Color.blue.opacity(0.2) : .clear)
                .cornerRadius(4)
            }
        }
    }
}
