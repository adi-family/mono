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
        MoveStep(reason: InstallLocation.current().reason, moving: model.moving) {
            model.moveToApplications()
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
        // One thing to read, one thing to press, one thing to change — in that order.
        //
        // Two large controls of equal weight was the problem: neither read as the answer, so
        // the window asked a question instead of offering one. The dashboard is the only
        // action anyone takes twice, so it is the only prominent control; the state above it
        // is information and has no button at all; and the switch below the rule is a setting,
        // which is what turning a background service on and off actually is.
        VStack(spacing: 16) {
            StatusCard(state: model.powerState,
                       title: model.statusSummary,
                       detail: model.runningDetail)

            DashboardButton(enabled: model.anyRunning) {
                model.openDashboard()
            }

            Divider().opacity(0.5)

            ServicesToggle(isOn: model.servicesOn, busy: model.busy)
        }
    }
}

/// Step one, driven by plain values rather than by the model.
///
/// Split out for the same reason `PermissionRow` is: a view that takes `moving` as an argument
/// can be rendered in both states without a running app, which is the only way to actually look
/// at the in-progress state — the copy it reports on finishes in a second or two and then
/// relaunches the process out from under you.
struct MoveStep: View {
    let reason: String
    let moving: Bool
    let move: () -> Void

    var body: some View {
        VStack(spacing: 14) {
            Text("Move ADI to Applications")
                .font(.title3.weight(.semibold))
            Text(reason)
                .font(.callout)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)

            // The spinner sits inside the button rather than beside it, so the layout does not
            // jump at the moment the user is watching it.
            Button(action: move) {
                HStack(spacing: 8) {
                    if moving {
                        ProgressView().controlSize(.small)
                    }
                    Text(moving ? "Moving to Applications…" : "Move to Applications")
                }
                .frame(maxWidth: .infinity)
            }
            .buttonStyle(.borderedProminent)
            .controlSize(.large)
            .disabled(moving)

            if moving {
                Text("Copying the app, then reopening it from Applications.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
            }
        }
    }
}

/// One permission: what it is for, and a button that becomes a checkmark once it is granted.
struct PermissionRow: View {
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
            } else if busy {
                // Something is running — probably this grant, and the moment after the password
                // prompt is dismissed is exactly when there is otherwise nothing to look at.
                ProgressView().controlSize(.small)
            } else {
                Button("Allow", action: grant)
                    .buttonStyle(.bordered)
            }
        }
        .padding(12)
        .background(.quaternary.opacity(0.4), in: RoundedRectangle(cornerRadius: 10))
    }
}
