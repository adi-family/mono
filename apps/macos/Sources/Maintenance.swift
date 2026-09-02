import SwiftUI

/// The two controls that have to work when nothing else does: update, and report a problem.
///
/// They sit below a rule at the foot of every step of the window, including the two setup steps,
/// because that is where they are needed. Somebody stuck on "Local names" has an install whose
/// `.adi` route does not resolve — so the control panel is unreachable, the docs are unreachable,
/// and the only two things left worth doing are *take the fix* and *send the evidence*. Both run
/// the CLI in `Contents/Resources`, which needs nothing of ADI's own to be working.
///
/// Quiet on purpose: neither is the reason anybody opened this window, and the accent belongs to
/// the dashboard button above them (`docs/design.md`, rule 3). They are a settings-weight band —
/// small type, bordered buttons, no fill — and what steps one of them forward is its button
/// changing what it offers, never a colour.
struct MaintenanceFooter: View {
    @ObservedObject var model: AppModel
    /// False on the step that asks for the app to be moved. An update installs into
    /// `/Applications` wherever the running copy happens to be, so from a mounted disk image it
    /// would quietly update a *different* bundle and leave this window looking untouched — two
    /// ADIs and no explanation. That step has one thing to do, and this is not it.
    let offerUpdate: Bool

    var body: some View {
        VStack(spacing: 12) {
            Rectangle()
                .fill(.primary.opacity(0.13))
                .frame(height: 1)

            if offerUpdate, model.updatable {
                UpdateRow(
                    installed: model.installedVersion,
                    state: model.updateState,
                    check: { model.checkForUpdates() },
                    install: { model.installUpdate() }
                )
            }

            ReportRow(
                state: model.reportState,
                act: {
                    if case .ready = model.reportState {
                        model.revealReport()
                    } else {
                        model.createReport()
                    }
                },
                openIssue: { model.openIssue() }
            )
        }
    }
}

/// The shape both rows share: a glyph in a fixed gutter, then whatever the row puts beside it.
///
/// The gutter is a fixed width rather than each glyph's own. SF Symbols differ in width, so laid
/// out naturally the two rows' text would start at different x positions and a band of two lines
/// would read as ragged for no reason a viewer could name.
struct RowLayout<Content: View>: View {
    let symbol: String
    @ViewBuilder let content: Content

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 9) {
            Image(systemName: symbol)
                .font(.system(size: 12))
                .foregroundStyle(.secondary)
                .frame(width: 15, alignment: .leading)
                // The glyph labels the band for the eye; the text beside it already says what
                // this is, so announcing it again is noise in a screen reader.
                .accessibilityHidden(true)
            content
        }
    }
}

/// The installed version, what the release channel says about it, and the one button that acts.
///
/// Plain values rather than the model, so both interesting states — an update waiting, an update
/// running — can be put on screen without a machine in that condition. The installing state in
/// particular cannot be looked at any other way: it ends by terminating the app.
struct UpdateRow: View {
    let installed: String
    let state: AppModel.UpdateState
    let check: () -> Void
    let install: () -> Void

    var body: some View {
        RowLayout(symbol: "arrow.down.circle") {
            VStack(alignment: .leading, spacing: 1) {
                Text("Version \(installed)")
                    .font(.system(size: 12, weight: .medium))
                if let note {
                    Text(note)
                        .font(.system(size: 11))
                        // Weight, not colour. The obvious way to mark a waiting update is to
                        // put it in the accent, and it is wrong twice: it would be a second
                        // accent in a window whose one accent is the dashboard button, and
                        // `#FA5019` as *text* measures 3.37:1 on a light surface — below the
                        // 4.5:1 this design language treats as non-negotiable (docs/design.md,
                        // rules 2, 3 and 6). The button beside it already says "Update to X",
                        // which is where the eye goes.
                        .foregroundStyle(highlighted ? AnyShapeStyle(.primary)
                                                     : AnyShapeStyle(.secondary))
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            Spacer(minLength: 0)
            button
        }
        .animation(.easeInOut(duration: 0.18), value: state)
    }

    @ViewBuilder
    private var button: some View {
        switch state {
        case .checking, .installing:
            ProgressView().controlSize(.small)
        case let .available(version):
            Button("Update to \(version)", action: install)
                .buttonStyle(.bordered)
                .controlSize(.small)
        default:
            Button("Check for Updates", action: check)
                .buttonStyle(.bordered)
                .controlSize(.small)
        }
    }

    /// The second line, when there is one worth reading. `unknown` has none: an app that has
    /// never checked should say nothing rather than guess, and the button already says what to
    /// do about it.
    private var note: String? {
        switch state {
        case .unknown: nil
        case .checking: "Checking…"
        case .upToDate: "Up to date"
        case let .available(version): "\(version) is available"
        case let .unavailable(version): "\(version) is out, but not for this Mac"
        // The window is about to disappear and come back as the new version, which looks like a
        // crash unless it was announced first.
        case .installing: "Updating — ADI will restart itself"
        case .failed: "The last attempt failed"
        }
    }

    /// Whether this row has something to say rather than something to report.
    private var highlighted: Bool {
        if case .available = state { return true }
        return false
    }
}

/// The way to hand somebody else the evidence.
///
/// It is offered before anything has gone wrong, and worded as a question rather than an error,
/// because the moment it is needed is the moment the person cannot find anything: the alternative
/// is a support conversation that starts by airdropping them a shell script.
struct ReportRow: View {
    let state: AppModel.ReportState
    let act: () -> Void
    let openIssue: () -> Void

    var body: some View {
        RowLayout(symbol: "ladybug.fill") {
            VStack(alignment: .leading, spacing: 7) {
                VStack(alignment: .leading, spacing: 1) {
                    Text(title)
                        .font(.system(size: 12, weight: .medium))
                    if let note {
                        Text(note)
                            .font(.system(size: 11))
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }

                // Two actions, so they go under the text rather than beside it: side by side on
                // the title's line they would not fit the 340pt window, and shortening either to
                // make them fit costs the word that says what it does.
                HStack(spacing: 8) {
                    if case .collecting = state {
                        ProgressView().controlSize(.small)
                    } else {
                        Button(buttonTitle, action: act)
                            .buttonStyle(.bordered)
                            .controlSize(.small)
                    }
                    Button(action: openIssue) {
                        HStack(spacing: 4) {
                            Text("Open an Issue")
                            Image(systemName: "arrow.up.forward")
                                .font(.system(size: 9, weight: .bold))
                        }
                    }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                    .help("Open a pre-filled GitHub issue, then drag the report into it")
                }
            }
        }
        .animation(.easeInOut(duration: 0.18), value: state)
    }

    private var title: String {
        if case .ready = state { return "Report saved" }
        return "Something not working?"
    }

    private var note: String? {
        switch state {
        case .idle: "Collect the logs into one file, then open an issue with it."
        case .collecting: "Reading logs, services and routes…"
        case let .ready(_, findings):
            findings == 0
                ? "Drag it into the issue you open."
                : "Drag it into the issue you open. \(findings) thing\(findings == 1 ? "" : "s") "
                    + "already look wrong — summary.txt says which."
        case .failed: "It could not be written. Try again."
        }
    }

    private var buttonTitle: String {
        if case .ready = state { return "Show in Finder" }
        return "Create Report"
    }
}
