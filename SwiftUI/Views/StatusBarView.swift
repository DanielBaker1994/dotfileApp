import SwiftUI

// MARK: - Status bar (transient error/success strip at bottom)

struct StatusBarView: View {
    let text: String
    let isError: Bool

    @Environment(\.popupColors) var colors

    var body: some View {
        HStack {
            if isError {
                Image(systemName: "exclamationmark.triangle.fill")
                    .font(.system(size: 12))
                    .foregroundColor(.red)
            }
            Text(text)
                .font(.system(size: 11))
                .foregroundColor(isError ? .red : colors.text)
                .lineLimit(1)
            Spacer()
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 4)
        .background(
            Rectangle()
                .fill(isError ? Color.red.opacity(0.1) : colors.highlight.opacity(0.2))
        )
    }
}
