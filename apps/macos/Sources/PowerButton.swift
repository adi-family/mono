import SwiftUI

/// The one big control: a circular power toggle with a solid, color-filled disc that
/// tracks state — dim when off, blue (with a spinner) while in progress, green when
/// running — and a white power glyph.
struct PowerButton: View {
    let state: PowerState
    let action: () -> Void

    private static let blue = Color(red: 0.15, green: 0.50, blue: 0.98)
    private static let green = Color(red: 0.20, green: 0.78, blue: 0.42)

    /// The solid fill color for the current state.
    private var color: Color {
        switch state {
        case .off: return Color.primary.opacity(0.12)
        case .inProgress: return Self.blue
        case .on: return Self.green
        }
    }

    private var isBusy: Bool { state == .inProgress }
    private var isActive: Bool { state != .off }

    var body: some View {
        Button(action: action) {
            ZStack {
                Circle()
                    .fill(color)
                    .overlay(
                        Circle().strokeBorder(
                            isActive ? Color.white.opacity(0.25) : Color.primary.opacity(0.15),
                            lineWidth: 2
                        )
                    )
                    .frame(width: 160, height: 160)
                    .shadow(color: isActive ? color.opacity(0.5) : .clear, radius: 24)

                if isBusy {
                    ProgressView()
                        .controlSize(.large)
                        .tint(.white)
                } else {
                    Image(systemName: "power")
                        .font(.system(size: 62, weight: .medium))
                        .foregroundStyle(isActive ? Color.white : Color.secondary)
                }
            }
        }
        .buttonStyle(.plain)
        .disabled(isBusy)
        .animation(.easeInOut(duration: 0.25), value: state)
        .accessibilityLabel(state == .on ? "Turn ADI off" : "Turn ADI on")
    }
}
