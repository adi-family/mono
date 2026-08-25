import SwiftUI

/// The main window, which is whichever of three things the install still needs.
///
/// The order is a gate, not a preference. Until the app is somewhere its services may point at,
/// nothing can be installed — so nothing is offered. Until both privileged grants exist, the
/// services can be started but the names they serve go nowhere — so the power button is not
/// offered either: a stack reporting "Running" while `app.adi` fails to load is worse than one
/// that has not started. Only past both does the app become the app.
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

                switch model.stage {
                case .mustMove: moveStep
                case .needsPermissions: permissionsStep
                case .ready: readyStep
                }
            }
            .padding(.horizontal, 36)
            .padding(.vertical, 34)
            .frame(width: 340)
        }
        .frame(width: 340)
        .alert("ADI could not move itself", isPresented: .constant(model.moveFailure != nil)) {
            Button("OK") { model.moveFailure = nil }
        } message: {
            Text(model.moveFailure ?? "")
        }
    }

    // MARK: step 1 — be somewhere durable

    private var moveStep: some View {
        VStack(spacing: 14) {
            Text("Move ADI to Applications")
                .font(.title3.weight(.semibold))
            Text(InstallLocation.current().reason)
                .font(.callout)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)

            Button("Move to Applications") { model.moveToApplications() }
                .buttonStyle(.borderedProminent)
                .controlSize(.large)
                .disabled(model.busy)
        }
    }

    // MARK: step 2 — the two grants

    private var permissionsStep: some View {
        VStack(spacing: 16) {
            Text("Two permissions to set up")
                .font(.title3.weight(.semibold))
            Text("Each asks for your password once. ADI starts itself as soon as both are done.")
                .font(.callout)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)

            VStack(spacing: 10) {
                PermissionRow(
                    title: "Local names",
                    detail: "Lets .\(Core.domain) addresses resolve on this Mac.",
                    granted: model.hasDNS,
                    busy: model.busy
                ) { model.grantDNS() }

                PermissionRow(
                    title: "Network access",
                    detail: "Lets ADI answer those addresses on port 80.",
                    granted: model.hasNetwork,
                    busy: model.busy
                ) { model.grantNetwork() }
            }
        }
    }

    // MARK: step 3 — the app

    private var readyStep: some View {
        VStack(spacing: 18) {
            // The dashboard is what someone opened this app to reach, so it is the loudest
            // thing in the window. The power switch is maintenance and sits under it.
            Button {
                model.openDashboard()
            } label: {
                Label("Open Dashboard", systemImage: "arrow.up.forward.app")
                    .frame(maxWidth: .infinity)
            }
            .buttonStyle(.borderedProminent)
            .controlSize(.large)
            .disabled(!model.anyRunning)

            VStack(spacing: 12) {
                PowerButton(state: model.powerState) {
                    model.togglePower()
                }
                Text(model.statusSummary)
                    .font(.callout.weight(.medium))
                    .foregroundStyle(model.isOn ? .primary : .secondary)
                    .contentTransition(.opacity)
            }
        }
    }
}

/// One permission: what it is for, and a button that becomes a checkmark once it is granted.
private struct PermissionRow: View {
    let title: String
    let detail: String
    let granted: Bool
    let busy: Bool
    let grant: () -> Void

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text(title).font(.callout.weight(.medium))
                Text(detail)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 8)

            if granted {
                Image(systemName: "checkmark.circle.fill")
                    .foregroundStyle(.green)
                    .font(.title3)
                    .accessibilityLabel("\(title) granted")
            } else {
                Button("Allow", action: grant)
                    .buttonStyle(.bordered)
                    .disabled(busy)
            }
        }
        .padding(12)
        .background(.quaternary.opacity(0.4), in: RoundedRectangle(cornerRadius: 10))
    }
}
