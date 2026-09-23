import SwiftUI

// MARK: - Search field (centered placeholder, rounded background)

struct SearchFieldView: View {
    @Bindable var state: PopupState
    var onSubmit: () -> Void

    var body: some View {
        TextField("Search", text: $state.query)
            .font(.system(size: 12 * state.zoom))
            .textFieldStyle(.plain)
            .multilineTextAlignment(.center)
            .frame(height: 26 * state.zoom)
            .padding(.horizontal, 12 * state.zoom)
            .background(
                RoundedRectangle(cornerRadius: 6 * state.zoom)
                    .fill(state.colors.highlight.opacity(0.3))
            )
            .padding(.horizontal, 10)
            .padding(.top, 4)
            .onSubmit { onSubmit() }
    }
}
