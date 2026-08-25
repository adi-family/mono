import SwiftUI

/// What ADI is doing, as a surface rather than a word.
///
/// The state, and nothing else. It used to be a 160pt button, which made reporting look like
/// an action; it is a card with a lit dot because it is something to read.
///
/// The dot is semantic colour — green running, blue mid-flight, grey off — which is a separate
/// axis from the accent (`docs/design.md`), so it can sit in the same window as an orange button
/// without being a second accent.
struct StatusCard: View {
    let state: PowerState
    let title: String

    private static let working = Color(red: 0.15, green: 0.50, blue: 0.98)
    private static let live = Color(red: 0.20, green: 0.78, blue: 0.42)

    private var dot: Color {
        switch state {
        case .off: return .secondary
        case .inProgress: return Self.working
        case .on: return Self.live
        }
    }

    var body: some View {
        HStack(spacing: 12) {
            // A halo around the dot rather than a bigger dot: at 9pt a bare circle reads as a
            // bullet, and the ring is what makes it read as a light.
            ZStack {
                Circle().fill(dot.opacity(0.20)).frame(width: 24, height: 24)
                Circle().fill(dot).frame(width: 9, height: 9)
                    .shadow(color: state == .off ? .clear : dot.opacity(0.7), radius: 5)
            }

            Text(title)
                .font(.system(size: 15, weight: .semibold))
                .contentTransition(.opacity)

            Spacer(minLength: 0)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 11)
        .background(.quaternary.opacity(0.35), in: RoundedRectangle(cornerRadius: 12))
        .animation(.easeInOut(duration: 0.2), value: state)
    }
}

/// The on/off switch, demoted to what it is: a setting, not the point of the window.
///
/// It was a 160pt disc, which made it compete with the one action worth taking. Turning ADI on
/// and off is something you do once; opening the dashboard is something you do every day.
struct ServicesToggle: View {
    @Binding var isOn: Bool
    let busy: Bool

    var body: some View {
        HStack(spacing: 12) {
            Text("ADI services")
                .font(.system(size: 13, weight: .medium))
            Spacer(minLength: 0)
            Toggle("", isOn: $isOn)
                .labelsHidden()
                .toggleStyle(.switch)
                .controlSize(.small)
                .tint(ADILogo.accent)
                .disabled(busy)
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("ADI services")
    }
}
