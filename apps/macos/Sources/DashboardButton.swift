import SwiftUI

/// The way into the control panel, which is what someone opened this app to reach.
///
/// The one action anyone takes twice, so it is the one filled control on the ready step — the
/// screen's orange (`design/DESIGN.md` §2.4) — unless an update is waiting, in which case the
/// update button takes the orange and this one becomes the ink fill. Two oranges compete.
struct DashboardButton: View {
    /// True while the services are being started on the way to opening it.
    let busy: Bool
    /// Whether this is the screen's one orange. False when the update row holds it.
    let accent: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 6) {
                if busy {
                    ProgressView().controlSize(.small)
                }
                Text(busy ? "Starting ADI…" : "Open control panel")
                if !busy {
                    LucideIcon(icon: .arrowUpRight, size: .sm)
                }
            }
        }
        // Never disabled for being "not ready". A greyed-out button is a dead end: it states a
        // precondition and leaves the person to work out how to satisfy it. Pressing it when
        // nothing is running starts everything and then opens the panel, which is what pressing
        // it meant in the first place.
        .buttonStyle(.adi(accent ? .accent : .strong, wide: true))
        .disabled(busy)
    }
}
