import SwiftUI

/// The way into the dashboard, which is what someone opened this app to reach.
///
/// Drawn rather than taken from `.borderedProminent`. The stock style is a system-blue rounded
/// rectangle with a system corner radius, and sitting it above a bespoke 160pt disc put two
/// different design languages in one small window — it read as a control borrowed from another
/// app. This one is built from the same parts as `PowerButton`: a solid fill, a hairline of
/// white on top of it, and a glow the colour of the fill.
///
/// The fill is the ADI accent, so the one saturated thing in the window is the one action worth
/// taking. The power button beside it is a *state* — green when running — and stays the larger
/// shape without competing for the same colour.
struct DashboardButton: View {
    /// True while the services are being started on the way to opening it.
    let busy: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 7) {
                if busy {
                    ProgressView().controlSize(.small).tint(.white)
                }
                Text(busy ? "Starting ADI…" : "Open Dashboard")
                if !busy {
                    Image(systemName: "arrow.up.forward")
                        .font(.system(size: 12, weight: .bold))
                        // Nudged to sit on the text's optical centre; the glyph's own box is
                        // bottom-heavy, so aligned by frame it looks like it is sliding off.
                        .offset(y: -0.5)
                }
            }
        }
        // Never disabled for being "not ready". A greyed-out button is a dead end: it states a
        // precondition and leaves the person to work out how to satisfy it. Pressing it when
        // nothing is running starts everything and then opens the dashboard, which is what
        // pressing it meant in the first place.
        .buttonStyle(Style(enabled: true))
        .disabled(busy)
        .animation(.easeInOut(duration: 0.2), value: busy)
    }

    private struct Style: ButtonStyle {
        let enabled: Bool

        func makeBody(configuration: Configuration) -> some View {
            configuration.label
                .font(.system(size: 14.5, weight: .semibold))
                .foregroundStyle(enabled ? Color.white : Color.secondary)
                .frame(maxWidth: .infinity)
                .frame(height: 46)
                .background(
                    Capsule().fill(enabled ? ADILogo.accent : Color.primary.opacity(0.10))
                )
                .overlay(
                    Capsule().strokeBorder(
                        Color.white.opacity(enabled ? 0.22 : 0.08),
                        lineWidth: 1
                    )
                )
                .shadow(color: enabled ? ADILogo.accent.opacity(0.45) : .clear, radius: 18, y: 5)
                .scaleEffect(configuration.isPressed ? 0.975 : 1)
                .opacity(configuration.isPressed ? 0.88 : 1)
                .animation(.easeOut(duration: 0.12), value: configuration.isPressed)
        }
    }
}
