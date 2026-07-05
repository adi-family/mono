import SwiftUI

/// The main window: a translucent panel with the ADI logo and one big On/Off power
/// button — nothing else. Turning on enables all services (installing the `.adi` route
/// on first enable); quit via the app menu (⌘Q).
struct ContentView: View {
    @ObservedObject var model: AppModel

    var body: some View {
        ZStack {
            VisualEffectView().ignoresSafeArea()

            VStack(spacing: 20) {
                VStack(spacing: 9) {
                    ADILogo(size: 60)
                    Text("ADI")
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundStyle(.secondary)
                        .kerning(3)
                }

                PowerButton(state: model.powerState) {
                    model.togglePower()
                }

                Text(model.statusSummary)
                    .font(.title3.weight(.medium))
                    .foregroundStyle(model.isOn ? .primary : .secondary)
                    .contentTransition(.opacity)
            }
            .padding(.horizontal, 40)
            .padding(.vertical, 36)
            .frame(width: 320)
        }
        .frame(width: 320)
    }
}
