import SwiftUI

/// Where a tap on a chat goes. `runID == nil` is a chat that has not been started yet.
struct ChatRoute: Hashable {
    let node: String
    let agent: String
    let runID: String?
    let title: String
}

/// A page of a service on a node — where a link inside a chat goes when it names one.
struct AppRoute: Hashable {
    let node: String
    let service: String
    var path = "/"
}

/// Every conversation on every paired machine, newest first.
///
/// One list on the page surface (§2.5). A row is a chat's title over one dim line — the agent,
/// the machine when there is more than one, and when it last moved. A 6px dot before the title
/// says what it is doing, and the list is ordered by that before it is ordered by time.
struct ChatsView: View {
    let model: FleetModel
    let store: ChatStore
    /// The agent picker. Owned by the root, because the apps screen can ask for a new chat too.
    @Binding var starting: Bool

    @Environment(\.scenePhase) private var scenePhase
    @State private var path = NavigationPath()
    @State private var pairing = false
    @State private var query = ""
    @State private var renaming: Chat?
    @State private var title = ""
    @State private var failure: String?

    var body: some View {
        NavigationStack(path: $path) {
            Group {
                if model.nodes.isEmpty {
                    unpaired
                } else if store.chats.isEmpty {
                    quiet
                } else {
                    list
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(ADI.bg)
            .navigationTitle("Chats")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Menu {
                        Picker("Show sessions", selection: Binding(
                            get: { store.filter },
                            set: { choice in
                                store.filter = choice
                                Task { await store.refresh(model.nodes) }
                            }
                        )) {
                            ForEach(ChatFilter.allCases) { Text($0.label).tag($0) }
                        }
                    } label: {
                        Label {
                            Text("Show sessions")
                        } icon: {
                            // Ink when a filter is narrowing the list, dim when it is not: the
                            // state is the control's own tone, never the accent.
                            LucideIcon(icon: .listFilter, label: "Show sessions: \(store.filter.label)")
                                .foregroundStyle(store.filter == .all ? ADI.ink3 : ADI.ink)
                        }
                        .labelStyle(.iconOnly)
                    }
                    .disabled(model.nodes.isEmpty)
                    .accessibilityIdentifier("filter")
                }
                ToolbarItem(placement: .primaryAction) {
                    Button {
                        starting = true
                    } label: {
                        Label {
                            Text("New chat")
                        } icon: {
                            LucideIcon(icon: .plus)
                        }
                    }
                    .disabled(model.nodes.isEmpty)
                }
            }
            .navigationDestination(for: ChatRoute.self) { route in
                ChatView(route: route, model: model, store: store) { path.append($0) }
            }
            .appDestination(model)
            .sheet(isPresented: $pairing) { PairView(model: model) }
            .sheet(isPresented: $starting) {
                NewChatView(store: store, nodes: model.nodes) { agent in
                    starting = false
                    path.append(ChatRoute(node: agent.node, agent: agent.name, runID: nil, title: agent.name))
                }
            }
            .alert("Rename chat", isPresented: .init(get: { renaming != nil }, set: { if !$0 { renaming = nil } })) {
                TextField("Title", text: $title)
                Button("Rename") {
                    guard let chat = renaming else { return }
                    act(on: chat) { try await $0.rename(agent: chat.agent, run: chat.runID, title: title) }
                }
                Button("Cancel", role: .cancel) {}
            }
            .alert(
                "Something went wrong",
                isPresented: .init(get: { failure != nil }, set: { if !$0 { failure = nil } })
            ) {
                Button("OK", role: .cancel) {}
            } message: {
                Text(failure ?? "")
            }
        }
        // Re-read every five seconds while this list is what is on screen — the pace the panel's
        // own web client keeps. Pushing a chat or leaving the app restarts the task, which is what
        // stops it: an open chat polls for itself, and a backgrounded app polls for nobody.
        .task(id: Watch(nodes: model.nodes.map(\.petname), front: path.isEmpty && scenePhase == .active)) {
            guard path.isEmpty, scenePhase == .active else { return }
            while !Task.isCancelled {
                await store.refresh(model.nodes)
                try? await Task.sleep(for: .seconds(5))
            }
        }
    }

    private struct Watch: Equatable {
        let nodes: [String]
        let front: Bool
    }

    /// Nothing paired: the same sentence the fleet opens on, because it is the same fact.
    private var unpaired: some View {
        VStack(spacing: 12) {
            LucideIcon(icon: .network, size: .xl)
                .foregroundStyle(ADI.ink3)
            Text("No nodes yet")
                .font(ADI.TextStyle.section)
                .foregroundStyle(ADI.ink)
            Text("Pair a machine and the chats with its agents show up here — reached by key over the mesh, with no port open on either side.")
                .font(ADI.TextStyle.small)
                .foregroundStyle(ADI.ink2)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 320)
            Button("Pair a node") { pairing = true }
                .buttonStyle(.adi(.accent))
                .disabled(!model.ready)
                .padding(.top, 8)
        }
        .padding(32)
    }

    /// Paired, and nothing to list: still asking, refused, or genuinely empty — three different
    /// sentences, because they call for three different things from the reader.
    private var quiet: some View {
        let waiting = model.nodes.contains { !store.answered.contains($0.petname) && store.failures[$0.petname] == nil }
        return VStack(spacing: 12) {
            if waiting {
                ProgressView().controlSize(.large)
                Text("Asking your machines for their chats…")
                    .font(ADI.TextStyle.small)
                    .foregroundStyle(ADI.ink3)
            } else {
                LucideIcon(icon: .messageSquare, size: .xl)
                    .foregroundStyle(ADI.ink3)
                Text("No chats yet")
                    .font(ADI.TextStyle.section)
                    .foregroundStyle(ADI.ink)
                if !store.failures.isEmpty {
                    Text("Some machines could not be reached. The event log on the Fleet tab says why.")
                        .font(ADI.TextStyle.small)
                        .foregroundStyle(ADI.ink2)
                        .multilineTextAlignment(.center)
                        .frame(maxWidth: 320)
                }
                if store.failures.isEmpty {
                    if store.filter != .all {
                        Text("Nothing matches “\(store.filter.label)”.")
                            .font(ADI.TextStyle.small)
                            .foregroundStyle(ADI.ink2)
                        Button("Show all sessions") {
                            store.filter = .all
                            Task { await store.refresh(model.nodes) }
                        }
                        .buttonStyle(.adi())
                    }
                    Button("New chat") { starting = true }
                        .buttonStyle(.adi(.accent))
                        .padding(.top, 8)
                }
            }
        }
        .padding(32)
    }

    private var list: some View {
        List {
            ForEach(shown) { chat in
                NavigationLink(value: ChatRoute(
                    node: chat.node, agent: chat.agent, runID: chat.runID, title: chat.title)
                ) {
                    ChatRow(chat: chat, showsNode: model.nodes.count > 1)
                }
                .accessibilityIdentifier("chat")
                .adiRow()
                .swipeActions {
                    Button("Hide") {
                        act(on: chat) { try await $0.hide(agent: chat.agent, run: chat.runID) }
                    }
                    .tint(ADI.bgActive)
                }
                .contextMenu {
                    Button("Rename…") {
                        title = chat.title
                        renaming = chat
                    }
                    Button(chat.starred ? "Remove star" : "Star") {
                        act(on: chat) {
                            try await $0.star(agent: chat.agent, run: chat.runID, starred: !chat.starred)
                        }
                    }
                    Button("Hide") {
                        act(on: chat) { try await $0.hide(agent: chat.agent, run: chat.runID) }
                    }
                }
            }
        }
        .listStyle(.plain)
        .scrollContentBackground(.hidden)
        .searchable(text: $query, prompt: "Search chats")
        .refreshable { await store.refresh(model.nodes) }
    }

    private var shown: [Chat] {
        let needle = query.trimmingCharacters(in: .whitespaces)
        guard !needle.isEmpty else { return store.chats }
        return store.chats.filter {
            $0.title.localizedCaseInsensitiveContains(needle)
                || $0.agent.localizedCaseInsensitiveContains(needle)
                || $0.node.localizedCaseInsensitiveContains(needle)
        }
    }

    private func act(on chat: Chat, _ change: @escaping (NodeAPI) async throws -> Void) {
        Task { failure = await store.apply(chat, change) }
    }
}

/// One chat, as a row.
private struct ChatRow: View {
    let chat: Chat
    let showsNode: Bool

    /// The dot before the title, in the control panel's own three colours: yellow for a question
    /// waiting on its person, blue for running, grey for waiting on a wake. The blue is the
    /// panel's (`blue-400`), restated here because the token file has no blue — it is the one
    /// place the app needs to tell "working" from "needs you" at a glance, in a list.
    private var status: (color: Color, label: String)? {
        switch chat.activity {
        case .asking: (ADI.warn, "Needs your answer")
        case .running: (Color(hex: 0x60A5FA), "Running")
        case .awaiting: (ADI.ink3, "Awaiting")
        case .starred, .idle: nil
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 8) {
                if let status {
                    StatusDot(color: status.color)
                        .accessibilityLabel(status.label)
                        .accessibilityHidden(false)
                }
                Text(chat.title)
                    .font(ADI.sans(15))
                    .foregroundStyle(ADI.ink)
                    .lineLimit(1)
                if chat.starred {
                    LucideIcon(icon: .star, size: .sm, label: "Starred")
                        .foregroundStyle(ADI.ink3)
                }
            }
            HStack(spacing: 6) {
                Text(chat.agent)
                    .font(ADI.sans(12, .medium))
                    .foregroundStyle(ADI.ink2)
                    .lineLimit(1)
                if showsNode {
                    Text(chat.node)
                        .lineLimit(1)
                }
                Text(chat.lastActivity, format: .relative(presentation: .named, unitsStyle: .abbreviated))
                    .monospacedDigit()
                    .layoutPriority(1)
            }
            .font(ADI.TextStyle.label)
            .foregroundStyle(ADI.ink3)
        }
        .padding(.vertical, 4)
    }
}

/// Pick the agent a new chat is with — the control panel's own picker.
///
/// It leads with the **starred** agents, because on a machine with dozens of agents those are the
/// ones somebody decided are worth reaching for. A star is the machine's and the same for everyone
/// paired with it, so the personal half is here too: *Unstar for me* takes one out of this phone's
/// list without touching the machine, and *Unstarred for me* is where it goes and comes back from.
/// Every other agent is one disclosure, or one search, away.
struct NewChatView: View {
    let store: ChatStore
    let nodes: [Node]
    let pick: (ChatAgent) -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var query = ""
    @State private var showingUnstarred = false
    @State private var showingAll = false
    @State private var loading = true

    var body: some View {
        NavigationStack {
            Group {
                if store.agents.isEmpty {
                    VStack(spacing: 12) {
                        if loading {
                            ProgressView().controlSize(.large)
                            Text("Asking your machines for their agents…")
                                .font(ADI.TextStyle.small)
                                .foregroundStyle(ADI.ink3)
                        } else {
                            LucideIcon(icon: .bot, size: .xl)
                                .foregroundStyle(ADI.ink3)
                            Text(store.agentFailures.isEmpty
                                ? "No agents on your machines yet."
                                : "Your machines could not be reached. The event log on the Fleet tab says why.")
                                .font(ADI.TextStyle.small)
                                .foregroundStyle(ADI.ink2)
                                .multilineTextAlignment(.center)
                                .frame(maxWidth: 320)
                        }
                    }
                    .padding(32)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                } else {
                    list
                }
            }
            .background(ADI.bg)
            .navigationTitle("New chat")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
            }
            .task {
                await store.refreshAgents(nodes)
                loading = false
            }
        }
    }

    private var starred: [ChatAgent] {
        store.agents.filter { $0.starred && !store.unstarred.contains($0.id) }
    }

    private var unstarred: [ChatAgent] {
        store.agents.filter { $0.starred && store.unstarred.contains($0.id) }
    }

    private var found: [ChatAgent] {
        let needle = query.trimmingCharacters(in: .whitespaces)
        return store.agents.filter {
            $0.name.localizedCaseInsensitiveContains(needle) || $0.node.localizedCaseInsensitiveContains(needle)
        }
    }

    private var list: some View {
        List {
            if !query.trimmingCharacters(in: .whitespaces).isEmpty {
                // A search is over every agent: what is being looked for is usually the one that
                // is not starred.
                ForEach(found) { row($0) }
                if found.isEmpty {
                    hint("No agents match your search.")
                }
            } else {
                header("Starred agents")
                ForEach(starred) { agent in
                    row(agent)
                        .swipeActions {
                            Button("Unstar for me") { store.setUnstarred(agent, true) }
                                .tint(ADI.bgActive)
                        }
                        .contextMenu {
                            Button("Unstar for me") { store.setUnstarred(agent, true) }
                        }
                }
                if starred.isEmpty {
                    hint("No starred agents on your machines. Choose one from all agents below.")
                }
                if !unstarred.isEmpty {
                    disclosure("Unstarred for me", count: unstarred.count, open: $showingUnstarred)
                    if showingUnstarred {
                        ForEach(unstarred) { agent in
                            row(agent, dim: true)
                                .swipeActions {
                                    Button("Restore star") { store.setUnstarred(agent, false) }
                                        .tint(ADI.bgActive)
                                }
                                .contextMenu {
                                    Button("Restore star for me") { store.setUnstarred(agent, false) }
                                }
                        }
                    }
                }
                disclosure("All agents", count: store.agents.count, open: $showingAll)
                    .accessibilityIdentifier("all-agents")
                if showingAll {
                    ForEach(store.agents) { row($0) }
                }
            }
        }
        .listStyle(.plain)
        .scrollContentBackground(.hidden)
        .searchable(text: $query, prompt: "Search agents")
    }

    private func row(_ agent: ChatAgent, dim: Bool = false) -> some View {
        Button {
            pick(agent)
        } label: {
            HStack(spacing: 10) {
                // The star says why it is in this list; a personal un-star dims it rather than
                // hiding that the machine still stars it.
                LucideIcon(icon: agent.starred ? .star : .bot)
                    .foregroundStyle(dim ? ADI.ink3 : ADI.ink2)
                Text(agent.name)
                    .font(ADI.sans(15))
                    .foregroundStyle(dim ? ADI.ink2 : ADI.ink)
                    .lineLimit(1)
                Spacer(minLength: 8)
                if nodes.count > 1 {
                    Text(agent.node)
                        .font(ADI.TextStyle.label)
                        .foregroundStyle(ADI.ink3)
                        .lineLimit(1)
                }
            }
            .padding(.vertical, 4)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .adiRow()
        .accessibilityIdentifier("agent")
    }

    private func header(_ title: String) -> some View {
        Text(title)
            .font(ADI.TextStyle.label)
            .foregroundStyle(ADI.ink3)
            .adiRow()
            .listRowSeparator(.hidden, edges: .top)
    }

    private func hint(_ text: String) -> some View {
        Text(text)
            .font(ADI.TextStyle.small)
            .foregroundStyle(ADI.ink2)
            .adiRow()
    }

    /// A section that stays folded until asked for, with how much is in it.
    private func disclosure(_ title: String, count: Int, open: Binding<Bool>) -> some View {
        Button {
            open.wrappedValue.toggle()
        } label: {
            HStack(spacing: 8) {
                LucideIcon(icon: open.wrappedValue ? .chevronDown : .chevronRight, size: .sm)
                Text(title)
                Text("\(count)")
                    .monospacedDigit()
                Spacer(minLength: 0)
            }
            .font(ADI.TextStyle.label)
            .foregroundStyle(ADI.ink3)
            .padding(.vertical, 4)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .adiRow()
    }
}
