import SwiftUI

// MARK: - Row view (pill, title, icons, trailing, checkbox, multi-line)

struct RowView: View {
    let row: PopupRow
    let index: Int
    let isSelected: Bool
    let isTicked: Bool
    let matchRanges: [NSRange]
    let isTickable: Bool
    let onTap: () -> Void
    let onDoubleTap: () -> Void
    let onToggleTick: () -> Void

    @Environment(\.popupColors) var colors
    @Environment(\.popupZoom) var zoom: CGFloat

    var body: some View {
        HStack(spacing: 8) {
            if isTickable {
                CheckboxView(isTicked: isTicked) {
                    onToggleTick()
                }
            }

            VStack(alignment: .leading, spacing: 2) {
                // Title line (with optional match highlighting)
                titleLine

                // Content line (truncated, same line as title in wrap mode)
                if let content = row.content, !content.isEmpty {
                    Text(content)
                        .font(.system(size: 11 * zoom))
                        .foregroundColor(colors.dim)
                        .lineLimit(1)
                        .truncationMode(.tail)
                }

                // Meta line (detail + trailing)
                if row.detail != nil || row.trailing != nil {
                    HStack {
                        if let detail = row.detail, !detail.isEmpty {
                            Text(detail)
                                .font(.system(size: 10 * zoom))
                                .foregroundColor(colors.dim)
                                .lineLimit(1)
                                .truncationMode(.tail)
                        }
                        Spacer()
                        if let trailing = row.trailing, !trailing.isEmpty {
                            Text(trailing)
                                .font(.system(size: 11 * zoom))
                                .foregroundColor(colors.dim)
                        }
                    }
                }

                // Body (multi-line wrapped)
                if let body = row.body, !body.isEmpty {
                    Text(body)
                        .font(.system(size: 10 * zoom))
                        .foregroundColor(colors.dim)
                        .lineLimit(3)
                        .truncationMode(.tail)
                }
            }

            Spacer(minLength: 8)

            // Icons
            if !row.icons.isEmpty {
                HStack(spacing: 2) {
                    ForEach(Array(row.icons.enumerated()), id: \.offset) { _, img in
                        Image(nsImage: img)
                            .resizable()
                            .frame(width: 22 * zoom, height: 22 * zoom)
                    }
                }
            }
        }
        .padding(.horizontal, 18)
        .padding(.vertical, 4)
        .frame(minHeight: 28 * zoom)
        .background(selectionBackground, in: RoundedRectangle(cornerRadius: 6))
        .onTapGesture(count: 1) { onTap() }
        .onTapGesture(count: 2) { onDoubleTap() }
    }

    @ViewBuilder
    private var titleLine: some View {
        if let ranges = highlightedRanges, !ranges.isEmpty {
            // TODO: Attributed text with match highlights
            Text(row.title)
                .font(.system(size: 11 * zoom, weight: .regular))
                .foregroundColor(colors.text)
        } else {
            Text(row.title)
                .font(.system(size: 11 * zoom, weight: .regular))
                .foregroundColor(colors.text)
        }
    }

    private var highlightedRanges: [NSRange]? {
        // For now, return nil — match highlighting can be added later
        // when we port the full fuzzy match range logic
        nil
    }

    private var selectionBackground: some View {
        Group {
            if isSelected {
                RoundedRectangle(cornerRadius: 6)
                    .fill(colors.accent.opacity(0.3))
                    .overlay(
                        RoundedRectangle(cornerRadius: 6)
                            .stroke(colors.border.opacity(0.5), lineWidth: 1)
                    )
            } else {
                RoundedRectangle(cornerRadius: 6)
                    .fill(Color.clear)
            }
        }
    }
}

// MARK: - Checkbox

struct CheckboxView: View {
    let isTicked: Bool
    let action: () -> Void

    @Environment(\.popupColors) var colors

    var body: some View {
        Button(action: action) {
            Image(systemName: isTicked ? "checkmark.square.fill" : "square")
                .font(.system(size: 13))
                .foregroundColor(isTicked ? colors.accent : colors.dim)
                .frame(width: 18, height: 18)
        }
        .buttonStyle(.plain)
    }
}

// MARK: - Environment keys

private struct PopupColorsKey: EnvironmentKey {
    static let defaultValue = PopupColors()
}

private struct PopupZoomKey: EnvironmentKey {
    static let defaultValue: CGFloat = 1.0
}

extension EnvironmentValues {
    var popupColors: PopupColors {
        get { self[PopupColorsKey.self] }
        set { self[PopupColorsKey.self] = newValue }
    }

    var popupZoom: CGFloat {
        get { self[PopupZoomKey.self] }
        set { self[PopupZoomKey.self] = newValue }
    }
}
