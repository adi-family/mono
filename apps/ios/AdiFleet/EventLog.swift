import SwiftUI

/// What went wrong in the background, kept out of the way.
///
/// A machine that is asleep, off the network or refusing this phone is a condition, not an event
/// the reader has to deal with before they can use the app — and the sentence the mesh gives for
/// it runs to a paragraph. So it is written down here, once per problem rather than once per
/// retry, and the screens that were polling say at most one short line.
@MainActor
@Observable
final class EventLog {
    static let shared = EventLog()

    struct Entry: Identifiable {
        let id = UUID()
        /// The machine it is about.
        let source: String
        /// What was being asked of it: "Apps", "Chats".
        let area: String
        var message: String
        let first: Date
        var last: Date
        /// How many times in a row the same ask has failed.
        var count = 1
        /// Still failing as of the last ask. A later success closes it; the next failure is a new
        /// entry, because that is a new outage.
        var open = true
    }

    /// Newest first.
    private(set) var entries: [Entry] = []

    /// How many problems are still going on.
    var open: Int { entries.filter(\.open).count }

    func failed(_ source: String, _ area: String, _ message: String) {
        if let index = entries.firstIndex(where: { $0.open && $0.source == source && $0.area == area }) {
            // The same outage, asked again. The wording drifts between retries (a backoff, a
            // different resolver giving up first), so the latest is kept rather than compared.
            entries[index].message = message
            entries[index].last = Date()
            entries[index].count += 1
            return
        }
        let now = Date()
        entries.insert(Entry(source: source, area: area, message: message, first: now, last: now), at: 0)
        if entries.count > 200 { entries.removeLast(entries.count - 200) }
    }

    func succeeded(_ source: String, _ area: String) {
        for index in entries.indices where entries[index].open
            && entries[index].source == source && entries[index].area == area
        {
            entries[index].open = false
        }
    }

    func clear() {
        entries.removeAll()
    }
}

/// The log, as a list: which machine, what was asked, when, and the full sentence.
struct EventLogView: View {
    @Environment(\.dismiss) private var dismiss
    private let log = EventLog.shared

    var body: some View {
        NavigationStack {
            Group {
                if log.entries.isEmpty {
                    VStack(spacing: 12) {
                        LucideIcon(icon: .check, size: .xl)
                            .foregroundStyle(ADI.ink3)
                        Text("Nothing has gone wrong")
                            .font(ADI.TextStyle.section)
                            .foregroundStyle(ADI.ink)
                        Text("When a machine cannot be reached or refuses this phone, the reason is kept here.")
                            .font(ADI.TextStyle.small)
                            .foregroundStyle(ADI.ink2)
                            .multilineTextAlignment(.center)
                            .frame(maxWidth: 320)
                    }
                    .padding(32)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                } else {
                    List(log.entries) { entry in
                        VStack(alignment: .leading, spacing: 4) {
                            HStack(spacing: 6) {
                                StatusDot(color: entry.open ? ADI.err : ADI.ink3)
                                Text(entry.source)
                                    .font(ADI.sans(13.5, .medium))
                                    .foregroundStyle(ADI.ink)
                                Text(entry.area)
                                Spacer(minLength: 8)
                                if entry.count > 1 {
                                    Text("\(entry.count) times")
                                        .monospacedDigit()
                                }
                                Text(entry.last, format: .relative(presentation: .named, unitsStyle: .abbreviated))
                                    .monospacedDigit()
                            }
                            .font(ADI.TextStyle.label)
                            .foregroundStyle(ADI.ink3)
                            Text(entry.message)
                                .font(ADI.TextStyle.small)
                                .foregroundStyle(entry.open ? ADI.ink2 : ADI.ink3)
                                .textSelection(.enabled)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                        .padding(.vertical, 4)
                        .adiRow()
                    }
                    .listStyle(.plain)
                    .scrollContentBackground(.hidden)
                }
            }
            .background(ADI.bg)
            .navigationTitle("Event log")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Clear") { log.clear() }
                        .disabled(log.entries.isEmpty)
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
    }
}
