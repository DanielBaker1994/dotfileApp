import SwiftUI

// MARK: - Chrome view (header: icon, title, buttons, item count, voice meter)

struct ChromeView: View {
    @Bindable var state: PopupState
    var onIconClick: () -> Void
    var onHeaderButtonClick: (Int) -> Void
    var onRecord: () -> Void
    var onPause: () -> Void
    var onCopyPath: () -> Void
    var onCopyConfig: () -> Void

    @Environment(\.popupColors) var colors

    var body: some View {
        HStack(spacing: 8) {
            // Left: icon (with dropdown menu)
            if let icon = state.headerIcon {
                Button(action: onIconClick) {
                    Image(nsImage: icon)
                        .resizable()
                        .frame(width: 18, height: 18)
                }
                .buttonStyle(.plain)
            }

            // Title pill
            if let title = state.chromeHeaderTitle, state.titlePill {
                Text(title)
                    .font(.system(size: 11, weight: .medium))
                    .foregroundColor(colors.dim)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 2)
                    .background(
                        Capsule()
                            .fill(colors.highlight.opacity(0.3))
                    )
            } else if let title = state.chromeHeaderTitle {
                Text(title)
                    .font(.system(size: 11, weight: .medium))
                    .foregroundColor(colors.text)
            }

            // Item count (dim, left side)
            if let count = state.itemCount {
                Text(count)
                    .font(.system(size: 10))
                    .foregroundColor(colors.dim)
            }

            Spacer()

            // Voice meter (when recording)
            if state.meterEnabled {
                VoiceMeterView(state: state, onRecord: onRecord, onPause: onPause)
            }

            // Header buttons
            if state.copyConfigButtonLabel != nil && !state.copyConfigButtonLabel!.isEmpty {
                HeaderButton(label: state.copyConfigButtonLabel!) {
                    onCopyConfig()
                }
            }
            if state.copyPathButtonLabel != nil && !state.copyPathButtonLabel!.isEmpty {
                HeaderButton(label: state.copyPathButtonLabel!) {
                    onCopyPath()
                }
            }

            // Extra buttons
            ForEach(state.headerButtons, id: \.id) { button in
                HeaderButton(label: button.label) {
                    onHeaderButtonClick(button.id)
                }
            }
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .frame(height: state.headerHeight)
        .background(headerBackground)
    }

    private var headerBackground: some View {
        Group {
            if let headerColor = state.headerColor {
                RoundedRectangle(cornerRadius: 0)
                    .fill(Color(headerColor))
            } else {
                Color.clear
            }
        }
    }
}

// MARK: - Header button

struct HeaderButton: View {
    let label: String
    let action: () -> Void

    @Environment(\.popupColors) var colors

    var body: some View {
        Button(action: action) {
            Text(label)
                .font(.system(size: 10, weight: .semibold))
                .foregroundColor(colors.dim)
                .padding(.horizontal, 8)
                .padding(.vertical, 3)
                .background(
                    RoundedRectangle(cornerRadius: 4)
                        .fill(colors.highlight.opacity(0.2))
                )
        }
        .buttonStyle(.plain)
    }
}

// MARK: - Voice meter

struct VoiceMeterView: View {
    @Bindable var state: PopupState
    let onRecord: () -> Void
    let onPause: () -> Void

    @Environment(\.popupColors) var colors

    var body: some View {
        HStack(spacing: 6) {
            // Record / Stop button
            Button(action: onRecord) {
                Image(systemName: recordIcon)
                    .font(.system(size: 14))
                    .foregroundColor(recordColor)
                    .frame(width: 28, height: 28)
                    .background(
                        Circle()
                            .fill(recordColor.opacity(0.15))
                    )
            }
            .buttonStyle(.plain)

            // Pause button (when recording or paused)
            if state.voiceState == .recording || state.voiceState == .paused {
                Button(action: onPause) {
                    Image(systemName: state.voiceState == .paused ? "mic.fill" : "pause.fill")
                        .font(.system(size: 12))
                        .foregroundColor(colors.dim)
                }
                .buttonStyle(.plain)
            }

            // Elapsed time
            if state.voiceState != .idle {
                Text(elapsedString)
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundColor(colors.dim)
            }

            // Level bars
            if state.voiceState == .recording {
                LevelBars(level: state.voiceLevel, colors: colors)
            }
        }
    }

    private var recordIcon: String {
        switch state.voiceState {
        case .idle, .paused: return "mic.fill"
        case .recording: return "stop.fill"
        case .transcribing: return "waveform"
        }
    }

    private var recordColor: Color {
        if state.voiceState == .idle || state.voiceState == .paused {
            return .red
        }
        if state.voiceState == .transcribing {
            return colors.dim
        }
        return .red
    }

    private var elapsedString: String {
        let secs = Int(state.voiceElapsed)
        let mins = secs / 60
        let s = secs % 60
        return String(format: "%d:%02d", mins, s)
    }
}

struct LevelBars: View {
    let level: Float
    let colors: PopupColors

    var body: some View {
        HStack(spacing: 1) {
            ForEach(0..<8) { i in
                Rectangle()
                    .fill(levelBarColor(i))
                    .frame(width: 3, height: 8 + CGFloat(i) * 2)
            }
        }
        .frame(height: 24)
    }

    private func levelBarColor(_ index: Int) -> Color {
        let threshold = Float(index) / 8.0
        if level > threshold {
            if index < 4 { return .green }
            if index < 6 { return .yellow }
            return .red
        }
        return colors.dim.opacity(0.2)
    }
}
