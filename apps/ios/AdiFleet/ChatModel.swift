import Foundation
import SwiftUI

/// One conversation, as the list shows it: a run of an agent on a node.
struct Chat: Identifiable, Hashable {
    let node: String
    let agent: String
    let runID: String
    let title: String
    let lastActivity: Date
    let running: Bool
    let starred: Bool
    /// The agent is waiting on an answer from its person.
    let asking: Bool
    /// The run is waiting on the world: a wake it registered has not landed yet.
    let awaiting: Bool

    var id: String { "\(node)/\(agent)/\(runID)" }

    /// What the chat is doing, most pressing first. It is both the dot before the title and the
    /// first key the list is sorted by.
    enum Activity: Int, Comparable {
        /// It asked its person something and is stopped until they answer.
        case asking
        case running
        /// Waiting on a wake it left itself.
        case awaiting
        case starred
        case idle

        static func < (a: Self, b: Self) -> Bool { a.rawValue < b.rawValue }
    }

    var activity: Activity {
        if asking { return .asking }
        if running { return .running }
        if awaiting { return .awaiting }
        return starred ? .starred : .idle
    }
}

/// An agent a new conversation can be started with.
struct ChatAgent: Identifiable, Hashable {
    let node: String
    let name: String
    /// Starred on its machine, by whoever runs that machine — for everybody.
    let starred: Bool

    var id: String { "\(node)/\(name)" }
}

/// Every conversation on every paired node.
///
/// Each node is asked on its own and answers on its own, for the reason the fleet list does the
/// same: they are independent machines, and one that is asleep must not blank the chats of the two
/// that are awake.
/// Which conversations the list shows — the control panel's own four choices, in its own words.
enum ChatFilter: String, CaseIterable, Identifiable {
    case all, starred, mine, me

    var id: String { rawValue }

    var label: String {
        switch self {
        case .all: "All sessions"
        case .starred: "Only starred"
        case .mine: "Only started by any user"
        case .me: "Only started by me"
        }
    }
}

@MainActor
@Observable
final class ChatStore {
    /// Started by a person, by default: on a machine whose agents launch each other, the runs
    /// nobody typed outnumber the conversations by an order of magnitude.
    ///
    /// The node applies it, and keeps a chat whose agent is waiting on an answer whatever the
    /// choice — a question nobody is shown is a question nobody answers.
    var filter: ChatFilter = ChatFilter(rawValue: UserDefaults.standard.string(forKey: "chat.filter") ?? "") ?? .mine {
        didSet { UserDefaults.standard.set(filter.rawValue, forKey: "chat.filter") }
    }

    private(set) var chats: [Chat] = []
    /// Every agent a chat can be started with, starred first and then by name — the order the
    /// control panel's own picker lists them in.
    private(set) var agents: [ChatAgent] = []
    /// Why a node's agents could not be read, by petname.
    private(set) var agentFailures: [String: String] = [:]
    /// The agents this phone's owner took the star off **for themselves**, by `ChatAgent.id`.
    ///
    /// A star belongs to the machine and is the same for everyone paired with it; this is the
    /// personal half, the same one the panel's web UI keeps in the browser. It never leaves the
    /// phone and changes nothing on the node.
    private(set) var unstarred: Set<String> = Set(UserDefaults.standard.stringArray(forKey: "chat.unstarred") ?? [])

    /// Take the star off an agent for this phone only, or put it back.
    func setUnstarred(_ agent: ChatAgent, _ off: Bool) {
        if off { unstarred.insert(agent.id) } else { unstarred.remove(agent.id) }
        UserDefaults.standard.set(unstarred.sorted(), forKey: "chat.unstarred")
    }

    private var agentsByNode: [String: [ChatAgent]] = [:]

    /// Ask every node for its agents. Separate from the chat listing because it is only wanted
    /// when a new chat is being started, and is the slower of the two on a machine with many.
    func refreshAgents(_ nodes: [Node]) async {
        let known = Set(nodes.map(\.petname))
        for gone in agentsByNode.keys where !known.contains(gone) { agentsByNode[gone] = nil }
        agentFailures = agentFailures.filter { known.contains($0.key) }
        await withTaskGroup(of: Void.self) { group in
            for node in nodes {
                group.addTask { @MainActor in
                    do {
                        // One that lives in a terminal has no transcript to chat in, and one that
                        // cannot run has nothing to start.
                        self.agentsByNode[node.petname] = try await NodeAPI(node: node.petname).agents()
                            .filter { $0.runnable && !$0.interactive }
                            .map { ChatAgent(node: node.petname, name: $0.name, starred: $0.starred) }
                        self.agentFailures[node.petname] = nil
                        EventLog.shared.succeeded(node.petname, "Agents")
                    } catch {
                        self.agentFailures[node.petname] = error.localizedDescription
                        EventLog.shared.failed(node.petname, "Agents", error.localizedDescription)
                    }
                    self.agents = self.agentsByNode.values.joined().sorted {
                        $0.starred != $1.starred
                            ? $0.starred
                            : $0.name.localizedStandardCompare($1.name) == .orderedAscending
                    }
                }
            }
        }
    }
    /// Why a node's chats could not be read, by petname.
    private(set) var failures: [String: String] = [:]
    /// Petnames that have answered at least once — what tells "no chats" from "not asked yet".
    private(set) var answered: Set<String> = []

    private var byNode: [String: [Chat]] = [:]

    func refresh(_ nodes: [Node]) async {
        let known = Set(nodes.map(\.petname))
        for gone in byNode.keys where !known.contains(gone) { byNode[gone] = nil }
        failures = failures.filter { known.contains($0.key) }
        answered.formIntersection(known)
        await withTaskGroup(of: Void.self) { group in
            for node in nodes {
                group.addTask { @MainActor in await self.refresh(node.petname) }
            }
        }
        publish()
    }

    func refresh(_ node: String) async {
        do {
            let asked = filter
            let all = try await NodeAPI(node: node).chats(filter: asked)
            // The filter changed while this was in flight: the answer is to a question nobody is
            // asking any more, and the read the change started is already on its way.
            guard asked == filter else { return }
            var chats: [Chat] = []
            for agent in all.agents {
                for run in agent.runs {
                    let active = run.lastActivity > 0 ? run.lastActivity : run.startedAt
                    chats.append(Chat(
                        node: node, agent: agent.name, runID: run.runID,
                        title: Self.title(run),
                        lastActivity: Date(timeIntervalSince1970: Double(active) / 1000),
                        running: run.running, starred: run.starred, asking: run.asking, awaiting: run.awaiting))
                }
            }
            byNode[node] = chats
            failures[node] = nil
            answered.insert(node)
            EventLog.shared.succeeded(node, "Chats")
        } catch {
            failures[node] = error.localizedDescription
            EventLog.shared.failed(node, "Chats", error.localizedDescription)
        }
        publish()
    }

    private func publish() {
        // The control panel's own order (`app-chat-sidebar.ts`, `sorted`): what needs its person
        // first, then what is running, then what is waiting, then the starred — and only inside
        // each of those, newest first. Sorting on time alone buried a question asked an hour ago
        // under every run that had moved since.
        chats = byNode.values.joined().sorted {
            $0.activity != $1.activity ? $0.activity < $1.activity : $0.lastActivity > $1.lastActivity
        }
    }

    /// What a chat is called: its title, else how it opened.
    private static func title(_ run: RunInfo) -> String {
        if let title = run.title?.trimmingCharacters(in: .whitespacesAndNewlines), !title.isEmpty {
            return title
        }
        let opening = run.message.trimmingCharacters(in: .whitespacesAndNewlines)
        return opening.isEmpty ? "Untitled chat" : String(opening.prefix(120))
    }

    // MARK: row actions

    /// Run a change on a chat's node and re-read that node. Returns the error to show, if any.
    @discardableResult
    func apply(_ chat: Chat, _ change: (NodeAPI) async throws -> Void) async -> String? {
        do {
            try await change(NodeAPI(node: chat.node))
        } catch {
            return error.localizedDescription
        }
        await refresh(chat.node)
        return nil
    }
}

/// One open conversation: its turns, kept fresh by polling, and everything that can be done to it.
@MainActor
@Observable
final class ChatThread {
    let node: String
    let agent: String
    /// `nil` until the first message is sent — a new chat has no run yet.
    private(set) var runID: String?

    private(set) var turns: [Turn] = []
    private(set) var totalTurns = 0
    private(set) var running = false
    private(set) var ask: Ask?
    private(set) var loaded = false
    private(set) var sending = false
    /// Why the last read or write failed. A failed poll does not clear the transcript: what was
    /// read is still true, and the next poll usually succeeds.
    var failure: String?

    /// The expanded tool calls, keyed by `turn/from`. Absent means collapsed.
    private(set) var expanded: [String: [Step]] = [:]
    private(set) var expanding: Set<String> = []

    private var limit = NodeAPI.page
    private var api: NodeAPI { NodeAPI(node: node) }

    init(node: String, agent: String, runID: String?) {
        self.node = node
        self.agent = agent
        self.runID = runID
        loaded = runID == nil
    }

    var hasEarlier: Bool { turns.count < totalTurns }
    /// Whether anything is still moving, which is what sets the polling pace.
    var live: Bool { running || turns.contains { $0.pending || $0.queued } }

    /// Poll until the view goes away. Once a second while the agent is working, every five
    /// otherwise — the panel's own web client keeps the same two paces.
    func follow() async {
        while !Task.isCancelled {
            if runID != nil, !sending { await read() }
            try? await Task.sleep(for: .seconds(live ? 1 : 5))
        }
    }

    private func read() async {
        guard let runID else { return }
        do {
            take(try await api.peek(agent: agent, run: runID, limit: limit))
            if failure != nil { failure = nil }
        } catch is CancellationError {
        } catch let error as URLError where error.code == .cancelled {
        } catch {
            failure = error.localizedDescription
        }
    }

    private func take(_ peek: Peek) {
        // Assigned only when different: every assignment redraws the transcript, and an idle chat
        // is read every five seconds for as long as it is open.
        if turns != peek.turns { turns = peek.turns }
        if totalTurns != peek.totalTurns { totalTurns = peek.totalTurns }
        if running != peek.running { running = peek.running }
        if ask != peek.ask { ask = peek.ask }
        loaded = true
        refreshRunningCalls()
    }

    /// Send a message: the first one starts the run, every later one is a reply. While an answer
    /// is in flight the node queues it, and it comes back as a turn marked `queued`.
    func send(_ text: String) async -> Bool {
        let message = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !message.isEmpty, !sending else { return false }
        sending = true
        defer { sending = false }
        do {
            if let runID {
                take(try await api.reply(agent: agent, run: runID, message: message, limit: limit))
            } else {
                let started = try await api.start(agent: agent, message: message)
                guard !started.runID.isEmpty else {
                    failure = "\(agent) runs in a terminal, so it has no chat to open here."
                    return false
                }
                runID = started.runID
                running = true
                await read()
            }
            failure = nil
            return true
        } catch {
            failure = error.localizedDescription
            return false
        }
    }

    func stop() async {
        guard let runID else { return }
        do {
            try await api.stop(agent: agent, run: runID)
            await read()
        } catch {
            failure = error.localizedDescription
        }
    }

    func answer(_ ask: Ask, replies: [String]) async {
        guard let runID else { return }
        do {
            take(try await api.answer(agent: agent, run: runID, ask: ask.id, replies: replies, limit: limit))
        } catch {
            failure = error.localizedDescription
        }
    }

    /// Take a queued message back. `index` is its position among the queued turns.
    func unqueue(_ turn: Turn) async {
        guard let runID, let index = turns.filter(\.queued).firstIndex(of: turn) else { return }
        do {
            take(try await api.unqueue(agent: agent, run: runID, index: index, limit: limit))
        } catch {
            failure = error.localizedDescription
        }
    }

    func loadEarlier() async {
        limit += NodeAPI.page
        await read()
    }

    // MARK: folded tool calls

    static func key(turn: Int, from: Int) -> String { "\(turn)/\(from)" }

    func toggle(turn: Int, from: Int, to: Int) async {
        let key = Self.key(turn: turn, from: from)
        if expanded[key] != nil {
            expanded[key] = nil
            return
        }
        guard let runID, !expanding.contains(key) else { return }
        expanding.insert(key)
        defer { expanding.remove(key) }
        do {
            expanded[key] = try await api.steps(agent: agent, run: runID, turn: turn, from: from, to: to)
        } catch {
            failure = error.localizedDescription
        }
    }

    /// An open group whose calls are still running has to be re-read: its steps grow, and `to` on
    /// the folded step moves with them.
    private func refreshRunningCalls() {
        guard let runID, !expanded.isEmpty else { return }
        for turn in turns where turn.pending {
            for case let .calls(from, to, _, _, _, .running) in turn.steps {
                let key = Self.key(turn: turn.seq, from: from)
                guard expanded[key] != nil else { continue }
                Task {
                    if let steps = try? await api.steps(
                        agent: agent, run: runID, turn: turn.seq, from: from, to: to),
                        expanded[key] != nil, expanded[key] != steps
                    {
                        expanded[key] = steps
                    }
                }
            }
        }
    }
}
