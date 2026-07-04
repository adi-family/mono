import AppKit
import SwiftUI

/// The dropdown shown from the menu-bar icon. Renders one block per managed
/// service (status line, enable/disable, then any service-specific actions),
/// followed by global actions.
struct MenuContent: View {
    @ObservedObject var model: AppModel

    var body: some View {
        Text("ADI")

        Divider()

        ForEach(model.rows) { row in
            Text("\(row.name) — \(row.statusText)")

            Button(row.isEnabled ? "Disable \(row.name)" : "Enable \(row.name)") {
                model.toggle(row.id)
            }

            ForEach(row.actions) { action in
                Button(action.title) {
                    model.perform(serviceID: row.id, actionID: action.id)
                }
            }

            Divider()
        }

        Button("Refresh") { model.refresh() }
        Button("Quit ADI") { NSApplication.shared.terminate(nil) }
    }
}
