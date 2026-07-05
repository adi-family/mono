import AppKit
import SwiftUI

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
