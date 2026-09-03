import SwiftUI

/// The window, which is whichever of three things the install still needs.
///
/// The order is a gate, not a preference. Until the app is somewhere its services may point at,
/// nothing can be installed — so nothing is offered. Until both privileged grants exist, the
/// services can be started but the names they serve go nowhere — so the switch is not offered
/// either: a stack reporting "Running" while `app.adi` fails to load is worse than one that has
/// not started. Only past both does the app become the app.
///
/// One surface (`ADI.bgSide`, the bar and panel surface — this window is all chrome), hairlines
/// between the bands, one filled orange per step (`design/DESIGN.md` §2.4, §5).
struct ContentView: View {
    @ObservedObject var model: AppModel

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            Hairline()
                .padding(.vertical, 16)

            switch model.stage {
            case .mustMove: moveStep
            case .needsPermissions: permissionsStep
            case .ready: readyStep
            }

            // Below every step, including the two the gate above has not let past yet. The
            // gate is about what can be *installed*; reporting needs none of it, and a
            // person stuck on a permission they cannot grant has exactly two useful moves
            // left — take a version that fixes it, or send somebody the evidence.
            MaintenanceFooter(model: model, offerUpdate: model.stage != .mustMove)
                .padding(.top, 16)
        }
        // The title bar is hidden, so the traffic lights sit over the content's top edge; the
        // header starts below them.
        .padding(.top, 36)
        .padding([.horizontal, .bottom], 24)
        .frame(width: 340)
        .background(ADI.bgSide.ignoresSafeArea())
        .preferredColorScheme(.dark)
        .alert(model.notice?.title ?? "",
               isPresented: Binding(get: { model.notice != nil },
                                    set: { if !$0 { model.notice = nil } })) {
            Button("OK") { model.notice = nil }
        } message: {
            Text(model.notice?.body ?? "")
        }
    }

    /// The mark at 18 beside the wordmark at 15/600 (§10), and the version at the right.
    private var header: some View {
        HStack(spacing: 8) {
            ADILogo(size: 18)
            Text("adi")
                .font(ADI.TextStyle.wordmark)
                .foregroundStyle(ADI.ink)
            Spacer(minLength: 0)
            Text(model.installedVersion)
                .font(ADI.TextStyle.label)
                .foregroundStyle(ADI.ink3)
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("ADI \(model.installedVersion)")
    }

    // MARK: step 1 — be somewhere durable

    private var moveStep: some View {
        MoveStep(reason: InstallLocation.current().reason, moving: model.moving) {
            model.moveToApplications()
        }
    }

    // MARK: step 2 — the two grants

    private var permissionsStep: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Two permissions to set up")
                .font(ADI.TextStyle.section)
                .foregroundStyle(ADI.ink)
            Text("Each asks for your password once. ADI starts itself as soon as both are done.")
                .font(ADI.TextStyle.small)
                .foregroundStyle(ADI.ink2)
                .fixedSize(horizontal: false, vertical: true)

            VStack(spacing: 0) {
                PermissionRow(
                    title: "Local names",
                    detail: "Lets .\(Core.domain) addresses resolve on this Mac.",
                    granted: model.hasDNS,
                    busy: model.busy
                ) { model.grantDNS() }

                Hairline()

                PermissionRow(
                    title: "Network access",
                    detail: "Lets ADI answer those addresses on port 80.",
                    granted: model.hasNetwork,
                    busy: model.busy
                ) { model.grantNetwork() }
            }
            .padding(.top, 8)
        }
    }

    // MARK: step 3 — the app

    private var readyStep: some View {
        // One thing to read, one thing to change, one thing to press — in that order. The state
        // is information and has no button; the switch is a setting; the panel is the action.
        VStack(alignment: .leading, spacing: 12) {
            StatusLine(state: model.powerState, title: model.statusSummary)
            ServicesToggle(isOn: model.servicesOn, busy: model.busy)
            DashboardButton(busy: model.launching, accent: !model.updateAvailable) {
                model.openDashboard()
            }
            .padding(.top, 8)
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
        VStack(alignment: .leading, spacing: 8) {
            Text("Move ADI to Applications")
                .font(ADI.TextStyle.section)
                .foregroundStyle(ADI.ink)
            Text(reason)
                .font(ADI.TextStyle.small)
                .foregroundStyle(ADI.ink2)
                .fixedSize(horizontal: false, vertical: true)

            // The one thing this step is for, so it is the step's orange. The spinner sits inside
            // the button rather than beside it, so the layout does not jump at the moment the
            // user is watching it.
            Button(action: move) {
                HStack(spacing: 8) {
                    if moving {
                        ProgressView().controlSize(.small)
                    }
                    Text(moving ? "Moving to Applications…" : "Move to Applications")
                }
            }
            .buttonStyle(.adi(.accent, wide: true))
            .disabled(moving)
            .padding(.top, 8)

            if moving {
                Text("Copying the app, then reopening it from Applications.")
                    .font(ADI.TextStyle.label)
                    .foregroundStyle(ADI.ink3)
            }
        }
    }
}

/// One permission: what it is for, and a button that becomes a granted mark once it is.
///
/// Granted is a status, so it is a 6px dot and a word (§9), not an icon and not a colour on
/// the row.
struct PermissionRow: View {
    let title: String
    let detail: String
    let granted: Bool
    let busy: Bool
    let grant: () -> Void

    var body: some View {
        HStack(alignment: .center, spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(ADI.TextStyle.row)
                    .foregroundStyle(ADI.ink)
                Text(detail)
                    .font(ADI.TextStyle.small)
                    .foregroundStyle(ADI.ink3)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 8)

            if granted {
                HStack(spacing: 7) {
                    StatusDot(color: ADI.ok)
                    Text("Granted")
                        .font(ADI.TextStyle.small)
                        .foregroundStyle(ADI.ink2)
                }
                .accessibilityElement(children: .combine)
                .accessibilityLabel("\(title) granted")
            } else if busy {
                // Something is running — probably this grant, and the moment after the password
                // prompt is dismissed is exactly when there is otherwise nothing to look at.
                ProgressView().controlSize(.small)
            } else {
                Button("Allow", action: grant)
                    .buttonStyle(.adi(.normal, .small))
            }
        }
        .padding(.vertical, 10)
    }
}
