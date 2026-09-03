import SwiftUI

/// What ADI is doing, as a 6px dot and a word (`design/DESIGN.md` §6). Something to read, not
/// something to press — so it is a line, not a card and not a button.
///
/// The dot is semantic colour, which is a separate axis from the accent: green is *up*, the
/// accent is *live* — a service still coming up is the one thing in the window happening right
/// now — and grey is off.
struct StatusLine: View {
    let state: PowerState
    let title: String

    private var dot: Color {
        switch state {
        case .off: ADI.ink3
        case .inProgress: ADI.accent
        case .on: ADI.ok
        }
    }

    var body: some View {
        HStack(spacing: 8) {
            StatusDot(color: dot)
            Text(title)
                .font(ADI.TextStyle.row)
                .foregroundStyle(ADI.ink)
                .contentTransition(.opacity)
        }
        .accessibilityElement(children: .combine)
    }
}

/// The on/off switch, as what it is: a setting, not the point of the window.
///
/// Turning ADI on and off is something you do once; opening the control panel is something you
/// do every day, so the switch is small and the panel button is the one filled control.
struct ServicesToggle: View {
    @Binding var isOn: Bool
    let busy: Bool

    var body: some View {
        HStack(spacing: 12) {
            Text("ADI services")
                .font(ADI.TextStyle.row)
                .foregroundStyle(ADI.ink2)
            Spacer(minLength: 0)
            Toggle("", isOn: $isOn)
                .labelsHidden()
                .toggleStyle(.switch)
                .controlSize(.small)
                // Ink, never the accent: a switch that is on is a selected state (§3).
                .tint(ADI.ink)
                .disabled(busy)
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("ADI services")
    }
}
