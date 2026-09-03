import SwiftUI

/// The two controls that have to work when nothing else does: update, and report a problem.
///
/// They sit below a hairline at the foot of every step of the window, including the two setup
/// steps, because that is where they are needed. Somebody stuck on "Local names" has an install
/// whose `.adi` route does not resolve — so the control panel is unreachable, the docs are
/// unreachable, and the only two things left worth doing are *take the fix* and *send the
/// evidence*. Both run the CLI in `Contents/Resources`, which needs nothing of ADI's own to be
/// working.
///
/// Quiet on purpose: neither is the reason anybody opened this window. They are a band of
/// label-weight rows — 13px `ink2` titles, 12px `ink3` notes, the default button — and the only
/// time one of them takes the orange is when an update is actually waiting, which is then the
/// screen's one orange (`design/DESIGN.md` §2.4) and the panel button gives it up.
struct MaintenanceFooter: View {
    @ObservedObject var model: AppModel
    /// False on the step that asks for the app to be moved. An update installs into
    /// `/Applications` wherever the running copy happens to be, so from a mounted disk image it
    /// would quietly update a *different* bundle and leave this window looking untouched — two
    /// ADIs and no explanation. That step has one thing to do, and this is not it.
    let offerUpdate: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Hairline()

            if offerUpdate, model.updatable {
                UpdateRow(
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
/// The gutter is a fixed width rather than each glyph's own, so the two rows' text starts on one
/// edge. The icon takes the note's ink (§9) — it labels the band for the eye, and the text beside
/// it already says what this is, so it is hidden from a screen reader.
struct RowLayout<Content: View>: View {
    let icon: Lucide
    @ViewBuilder let content: Content

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            LucideIcon(icon: icon, size: .sm)
                .foregroundStyle(ADI.ink3)
                .frame(width: 14, alignment: .leading)
                .alignmentGuide(.firstTextBaseline) { $0[.bottom] - 2 }
            content
        }
    }
}

/// What the release channel says about this install, and the one button that acts.
///
/// Plain values rather than the model, so both interesting states — an update waiting, an update
/// running — can be put on screen without a machine in that condition. The installing state in
/// particular cannot be looked at any other way: it ends by terminating the app. The version
/// itself is in the window's header, so the row only carries the news.
struct UpdateRow: View {
    let state: AppModel.UpdateState
    let check: () -> Void
    let install: () -> Void

    var body: some View {
        RowLayout(icon: .arrowUp) {
            VStack(alignment: .leading, spacing: 1) {
                Text("Updates")
                    .font(ADI.sans(13, .medium))
                    .foregroundStyle(ADI.ink2)
                if let note {
                    Text(note)
                        .font(ADI.TextStyle.label)
                        .foregroundStyle(ADI.ink3)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            Spacer(minLength: 0)
            button
        }
    }

    @ViewBuilder
    private var button: some View {
        switch state {
        case .checking, .installing:
            ProgressView().controlSize(.small)
        case let .available(version):
            // The one orange on this screen: a waiting update is the most important action in
            // the window, and the panel button steps back to the ink fill while it is.
            Button("Update to \(version)", action: install)
                .buttonStyle(.adi(.accent, .small))
        default:
            Button("Check for updates", action: check)
                .buttonStyle(.adi(.normal, .small))
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
        RowLayout(icon: .bug) {
            VStack(alignment: .leading, spacing: 8) {
                VStack(alignment: .leading, spacing: 1) {
                    Text(title)
                        .font(ADI.sans(13, .medium))
                        .foregroundStyle(ADI.ink2)
                    if let note {
                        Text(note)
                            .font(ADI.TextStyle.label)
                            .foregroundStyle(ADI.ink3)
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
                            .buttonStyle(.adi(.normal, .small))
                    }
                    Button(action: openIssue) {
                        HStack(spacing: 4) {
                            Text("Open an issue")
                            LucideIcon(icon: .arrowUpRight, size: .sm)
                        }
                    }
                    .buttonStyle(.adi(.normal, .small))
                    .help("Open a pre-filled GitHub issue, then drag the report into it")
                }
            }
        }
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
        return "Create report"
    }
}
