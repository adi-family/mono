import SwiftUI

/// One conversation with an agent.
///
/// The transcript is the product (§2.1): it sits on the page surface in the largest body type the
/// app has, at a reading measure, and everything else on the screen is dimmer than it. What the
/// agent *did* — its tool calls, its thinking — is folded into one-line receipts between the
/// things it said. The send button is the screen's one orange.
struct ChatView: View {
    let route: ChatRoute
    let model: FleetModel
    let store: ChatStore
    /// Push a page of a service on a node — where a link to one of the machine's own apps goes.
    let open: (AppRoute) -> Void

    @State private var thread: ChatThread
    @State private var draft = ""
    @FocusState private var typing: Bool

    init(route: ChatRoute, model: FleetModel, store: ChatStore, open: @escaping (AppRoute) -> Void) {
        self.route = route
        self.model = model
        self.store = store
        self.open = open
        _thread = State(initialValue: ChatThread(node: route.node, agent: route.agent, runID: route.runID))
    }

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                // Not lazy, on purpose. A lazy stack estimates the heights of rows it has not built,
                // and with the scroll anchored to the bottom every correction to an estimate moves
                // the content and asks for another: scrolling up through a long answer spun the main
                // thread without ever settling. A read holds one page of turns, which is cheap to
                // lay out whole.
                VStack(alignment: .leading, spacing: 20) {
                    if !thread.loaded {
                        ProgressView()
                            .controlSize(.large)
                            .frame(maxWidth: .infinity)
                            .padding(.top, 48)
                    } else if thread.turns.isEmpty, thread.runID == nil {
                        opening
                    }
                    if thread.hasEarlier {
                        Button("Earlier messages") { Task { await thread.loadEarlier() } }
                            .buttonStyle(.adi(.quiet, .small))
                            .frame(maxWidth: .infinity)
                    }
                    ForEach(thread.turns) { turn in
                        TurnView(turn: turn, thread: thread)
                    }
                    if let ask = thread.ask {
                        AskCard(ask: ask) { replies in await thread.answer(ask, replies: replies) }
                            // A new question is a new form: answers ticked for the last one must
                            // not carry over to it.
                            .id(ask.id)
                    }
                    Color.clear.frame(height: 1).id(Self.end)
                }
                .padding(.horizontal, 16)
                .padding(.top, 16)
                .frame(maxWidth: 720)
                .frame(maxWidth: .infinity)
            }
            // Opens at the latest message and stays there while an answer grows — unless the
            // reader has scrolled up, which the anchor leaves alone.
            .defaultScrollAnchor(.bottom)
            .scrollDismissesKeyboard(.interactively)
            .safeAreaInset(edge: .bottom, spacing: 0) {
                composer { withAnimation(nil) { proxy.scrollTo(Self.end, anchor: .bottom) } }
            }
        }
        .background(ADI.bg)
        // The bottom edge belongs to the composer here; the tabs come back with the list.
        .toolbar(.hidden, for: .tabBar)
        .navigationTitle(route.title)
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Menu {
                    if let runID = thread.runID {
                        Button("Open in the control panel") {
                            open(AppRoute(node: route.node, service: "app", path: Self.panelPath(route.agent, runID)))
                        }
                    }
                    Button("Open \(route.node)’s control panel") {
                        open(AppRoute(node: route.node, service: "app"))
                    }
                } label: {
                    LucideIcon(icon: .ellipsis, label: "More")
                }
            }
        }
        // A link an agent writes to one of its machine's own apps (`http://shop.adi/…`) names a
        // host only that machine can resolve. Opened here it becomes the same service over the
        // mesh; every other link goes to the browser as usual.
        .environment(\.openURL, OpenURLAction { url in
            guard let target = appRoute(for: url) else { return .systemAction }
            open(target)
            return .handled
        })
        .task { await thread.follow() }
    }

    private static let end = "end"

    /// The conversation's own page on the panel's web UI.
    private static func panelPath(_ agent: String, _ run: String) -> String {
        var components = URLComponents()
        components.path = "/interface"
        components.queryItems = [URLQueryItem(name: "agent", value: agent), URLQueryItem(name: "run", value: run)]
        return components.string ?? "/interface"
    }

    /// The service a URL names, if it names one on a machine this phone can reach:
    /// `<label>.adi` on the chat's own node, or `<label>.<node>.n.adi` on a paired one.
    private func appRoute(for url: URL) -> AppRoute? {
        guard ["http", "https"].contains(url.scheme?.lowercased() ?? ""), let host = url.host?.lowercased() else {
            return nil
        }
        let labels = host.split(separator: ".").map(String.init)
        let node: String
        if labels.count == 2, labels[1] == "adi" {
            node = route.node
        } else if labels.count == 4, labels[2] == "n", labels[3] == "adi",
                  model.nodes.contains(where: { $0.petname == labels[1] })
        {
            node = labels[1]
        } else {
            return nil
        }
        var path = url.path.isEmpty ? "/" : url.path
        if let query = url.query { path += "?\(query)" }
        return AppRoute(node: node, service: labels[0], path: path)
    }

    /// A chat that has not started: who it is with, and where.
    private var opening: some View {
        VStack(spacing: 12) {
            LucideIcon(icon: .bot, size: .xl)
                .foregroundStyle(ADI.ink3)
            Text(route.agent)
                .font(ADI.TextStyle.section)
                .foregroundStyle(ADI.ink)
            Text("A new chat on \(route.node). It starts with your first message.")
                .font(ADI.TextStyle.small)
                .foregroundStyle(ADI.ink2)
                .multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity)
        .padding(.top, 48)
    }

    /// The composer (§6): the raised surface, a strong hairline, radius 10, and the orange square.
    private func composer(scroll: @escaping () -> Void) -> some View {
        let empty = draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        // With nothing typed and the agent working, the button's job is to stop it. Typing brings
        // the send arrow back: a message sent mid-answer is queued behind it.
        let stops = empty && thread.running
        return VStack(spacing: 8) {
            if let failure = thread.failure {
                Note(icon: .triangleAlert, text: failure)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            HStack(alignment: .bottom, spacing: 8) {
                TextField("Message \(route.agent)", text: $draft, axis: .vertical)
                    .lineLimit(1...6)
                    .font(ADI.TextStyle.body)
                    .foregroundStyle(ADI.ink)
                    .focused($typing)
                    .accessibilityIdentifier("composer")
                    .padding(.vertical, 6)
                Button {
                    if stops {
                        Task { await thread.stop() }
                    } else {
                        let text = draft
                        draft = ""
                        Task {
                            if await thread.send(text) {
                                scroll()
                            } else if draft.isEmpty {
                                // Not sent: hand the words back rather than losing them.
                                draft = text
                            }
                        }
                    }
                } label: {
                    LucideIcon(icon: stops ? .square : .arrowUp, label: stops ? "Stop" : "Send")
                        .foregroundStyle(stops ? ADI.ink : ADI.onAccent)
                        .frame(width: 32, height: 32)
                        .background(stops ? ADI.btn : ADI.accent, in: RoundedRectangle(cornerRadius: ADI.Radius.md))
                }
                .buttonStyle(.plain)
                .disabled((empty && !stops) || thread.sending)
                .opacity((empty && !stops) || thread.sending ? 0.5 : 1)
            }
            .padding(.leading, 12)
            .padding(.trailing, 6)
            .padding(.vertical, 6)
            .background(ADI.bgRaise, in: RoundedRectangle(cornerRadius: ADI.Radius.lg))
            .overlay(RoundedRectangle(cornerRadius: ADI.Radius.lg).stroke(ADI.lineStrong, lineWidth: 1))
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .frame(maxWidth: 744)
        .frame(maxWidth: .infinity)
        .background(ADI.bg)
    }
}

// MARK: - Turns

/// One message: ours in a tone-shifted block at the right, the agent's as the page itself.
private struct TurnView: View {
    let turn: Turn
    let thread: ChatThread

    var body: some View {
        if turn.isUser {
            VStack(alignment: .trailing, spacing: 4) {
                Text(turn.text)
                    .font(ADI.TextStyle.body)
                    .foregroundStyle(ADI.ink)
                    .lineSpacing(4)
                    .textSelection(.enabled)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                    .background(ADI.bgActive, in: RoundedRectangle(cornerRadius: ADI.Radius.lg))
                if turn.attachments > 0 || turn.queued {
                    HStack(spacing: 8) {
                        if turn.attachments > 0 {
                            Text(turn.attachments == 1 ? "1 attachment" : "\(turn.attachments) attachments")
                        }
                        if turn.queued {
                            Text("Queued")
                            Button("Remove") { Task { await thread.unqueue(turn) } }
                                .buttonStyle(.plain)
                                .foregroundStyle(ADI.ink2)
                        }
                    }
                    .font(ADI.TextStyle.label)
                    .foregroundStyle(ADI.ink3)
                }
            }
            .opacity(turn.queued ? 0.6 : 1)
            .padding(.leading, 48)
            .frame(maxWidth: .infinity, alignment: .trailing)
        } else {
            VStack(alignment: .leading, spacing: 12) {
                ForEach(Array(turn.steps.enumerated()), id: \.offset) { _, step in
                    StepView(step: step, turn: turn.seq, thread: thread)
                }
                // The closing text is the answer; the same words as the last thing it said
                // mid-turn would be the answer twice.
                if !turn.text.isEmpty, turn.steps.last != .message(turn.text) {
                    MarkdownView(text: turn.text).equatable()
                }
                if turn.pending, turn.text.isEmpty, turn.steps.isEmpty {
                    HStack(spacing: 8) {
                        StatusDot(color: ADI.accent)
                        Text("Working")
                            .font(ADI.TextStyle.small)
                            .foregroundStyle(ADI.ink3)
                    }
                } else if let meta {
                    Text(meta)
                        .font(ADI.TextStyle.label.monospacedDigit())
                        .foregroundStyle(ADI.ink3)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    /// How long the answer took and what it cost — minutes rather than raw seconds, money to cents.
    private var meta: String? {
        guard !turn.pending else { return nil }
        var parts: [String] = []
        if let ms = turn.durationMs, ms >= 1000 {
            let seconds = Int(ms / 1000)
            parts.append(seconds >= 60 ? "\(seconds / 60)m \(seconds % 60)s" : "\(seconds)s")
        }
        if let micro = turn.costMicroUSD, micro > 0 {
            parts.append(String(format: "$%.2f", Double(micro) / 1_000_000))
        }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }
}

/// One thing the agent did inside an answer.
private struct StepView: View {
    let step: Step
    let turn: Int
    let thread: ChatThread

    var body: some View {
        switch step {
        case let .message(text):
            MarkdownView(text: text).equatable()
        case let .thinking(text):
            ThinkingRow(text: text)
        case let .tool(name, input, output, status):
            Receipt {
                ToolDetail(name: name, input: input, output: output, status: status)
                    .padding(10)
            }
        case let .calls(from, to, count, tools, preview, status):
            let key = ChatThread.key(turn: turn, from: from)
            Receipt {
                Button {
                    Task { await thread.toggle(turn: turn, from: from, to: to) }
                } label: {
                    HStack(spacing: 8) {
                        if thread.expanding.contains(key) {
                            ProgressView().controlSize(.mini)
                        } else {
                            LucideIcon(icon: thread.expanded[key] == nil ? .chevronRight : .chevronDown, size: .sm)
                        }
                        if status == .running { StatusDot(color: ADI.accent) }
                        if status == .error { StatusDot(color: ADI.err) }
                        Text("\(count == 1 ? "1 call" : "\(count) calls") · \(tools.map(toolName).joined(separator: ", "))")
                            .font(ADI.TextStyle.small)
                            .lineLimit(1)
                            .layoutPriority(1)
                        Text(preview.replacingOccurrences(of: "\n", with: " "))
                            .font(ADI.mono(12))
                            .lineLimit(1)
                            .truncationMode(.tail)
                        Spacer(minLength: 0)
                    }
                    .foregroundStyle(ADI.ink3)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 8)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityIdentifier("calls")
                if let steps = thread.expanded[key] {
                    ForEach(Array(steps.enumerated()), id: \.offset) { _, inner in
                        if case let .tool(name, input, output, status) = inner {
                            Hairline()
                            ToolDetail(name: name, input: input, output: output, status: status)
                                .padding(10)
                        }
                    }
                }
            }
        case .unknown:
            EmptyView()
        }
    }
}

/// The frame a tool call sits in (§6): a hairline, radius 8, dim. A receipt, not a message.
private struct Receipt<Content: View>: View {
    @ViewBuilder let content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 0) { content }
            .frame(maxWidth: .infinity, alignment: .leading)
            .overlay(RoundedRectangle(cornerRadius: 8).stroke(ADI.line, lineWidth: 1))
    }
}

/// A tool's name as people say it: `mcp__adi__Bash` is Bash.
private func toolName(_ raw: String) -> String {
    raw.components(separatedBy: "__").last ?? raw
}

/// One tool call opened up: what it was asked and what came back.
private struct ToolDetail: View {
    let name: String
    let input: String
    let output: String
    let status: ToolStatus
    @State private var full = false

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                if status == .running { StatusDot(color: ADI.accent) }
                if status == .error { StatusDot(color: ADI.err) }
                Text(toolName(name))
                    .font(ADI.sans(13, .medium))
                    .foregroundStyle(ADI.ink2)
            }
            if !argument.isEmpty {
                Text(argument)
                    .font(ADI.mono(12))
                    .foregroundStyle(ADI.code)
                    .lineLimit(full ? nil : 4)
                    .textSelection(.enabled)
            }
            if !output.isEmpty {
                Text(output.trimmingCharacters(in: .whitespacesAndNewlines))
                    .font(ADI.mono(12))
                    .foregroundStyle(ADI.ink3)
                    .lineLimit(full ? 200 : 6)
                    .textSelection(.enabled)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .contentShape(Rectangle())
        .onTapGesture { full.toggle() }
    }

    /// The input as a line a person would recognise. Tools take JSON; most take one string that is
    /// the whole point — a command, a path, a query — so that string is shown rather than its
    /// envelope.
    private var argument: String {
        guard let data = input.data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else { return input }
        for key in ["command", "file_path", "path", "query", "pattern", "url", "prompt"] {
            if let value = object[key] as? String { return value }
        }
        let strings = object.values.compactMap { $0 as? String }
        return object.count == 1 && strings.count == 1 ? strings[0] : input
    }
}

/// The agent's reasoning, folded: there when it is wanted, never in the way of what it said.
private struct ThinkingRow: View {
    let text: String
    @State private var open = false

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Button {
                open.toggle()
            } label: {
                HStack(spacing: 8) {
                    LucideIcon(icon: open ? .chevronDown : .chevronRight, size: .sm)
                    Text("Thinking")
                        .font(ADI.TextStyle.small)
                }
                .foregroundStyle(ADI.ink3)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            if open {
                Text(text)
                    .font(ADI.TextStyle.small)
                    .foregroundStyle(ADI.ink3)
                    .lineSpacing(3)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }
}

// MARK: - A question from the agent

/// The agent asked, and the run waits (§6, the ask block): a strong hairline down the left, the
/// questions, the offered answers as tone-shifted chips, and room to say something else.
private struct AskCard: View {
    let ask: Ask
    let submit: ([String]) async -> Void

    @State private var chosen: [Int: Set<String>] = [:]
    @State private var typed: [Int: String] = [:]
    @State private var sending = false

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text(ask.headline.isEmpty ? "The agent asked you" : ask.headline)
                .font(ADI.TextStyle.label)
                .foregroundStyle(ADI.ink3)
            if !ask.note.isEmpty {
                MarkdownView(text: ask.note, ink: ADI.ink2).equatable()
            }
            ForEach(Array(ask.questions.enumerated()), id: \.offset) { index, question in
                VStack(alignment: .leading, spacing: 8) {
                    if !question.header.isEmpty {
                        Text(question.header)
                            .font(ADI.TextStyle.label)
                            .foregroundStyle(ADI.ink3)
                    }
                    Text(question.question)
                        .font(ADI.sans(15))
                        .foregroundStyle(ADI.ink)
                        .fixedSize(horizontal: false, vertical: true)
                    ForEach(question.options, id: \.label) { option in
                        let on = chosen[index, default: []].contains(option.label)
                        Button {
                            pick(option.label, for: index, multi: question.multiSelect)
                        } label: {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(option.label)
                                    .font(ADI.sans(14, on ? .medium : .regular))
                                    .foregroundStyle(on ? ADI.ink : ADI.ink2)
                                if !option.description.isEmpty {
                                    Text(option.description)
                                        .font(ADI.TextStyle.label)
                                        .foregroundStyle(ADI.ink3)
                                        .multilineTextAlignment(.leading)
                                }
                            }
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(.horizontal, 10)
                            .padding(.vertical, 8)
                            .background(on ? ADI.bgActive : ADI.chip, in: RoundedRectangle(cornerRadius: ADI.Radius.md))
                            .overlay(
                                RoundedRectangle(cornerRadius: ADI.Radius.md)
                                    .stroke(on ? ADI.lineStrong : Color.clear, lineWidth: 1))
                        }
                        .buttonStyle(.plain)
                        .accessibilityAddTraits(on ? .isSelected : [])
                    }
                    TextField(
                        question.options.isEmpty ? "Your answer" : "Something else",
                        text: .init(get: { typed[index] ?? "" }, set: { typed[index] = $0 }),
                        axis: .vertical
                    )
                    .lineLimit(1...4)
                    .font(ADI.TextStyle.ui)
                    .foregroundStyle(ADI.ink)
                    .adiField()
                }
            }
            Button(sending ? "Sending…" : "Answer") {
                sending = true
                Task {
                    await submit(replies)
                    sending = false
                }
            }
            .buttonStyle(.adi(.strong))
            .disabled(sending || replies.contains(where: \.isEmpty))
        }
        .padding(.leading, 16)
        .overlay(alignment: .leading) {
            Rectangle().fill(ADI.lineStrong).frame(width: 2)
        }
    }

    private func pick(_ label: String, for index: Int, multi: Bool) {
        var set = chosen[index, default: []]
        if set.contains(label) {
            set.remove(label)
        } else {
            if !multi { set.removeAll() }
            set.insert(label)
        }
        chosen[index] = set
    }

    /// One reply per question: what was ticked, in the order offered, then what was typed — joined
    /// the way the panel's own form joins them.
    private var replies: [String] {
        ask.questions.enumerated().map { index, question in
            var parts = question.options.map(\.label).filter { chosen[index, default: []].contains($0) }
            let extra = (typed[index] ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            if !extra.isEmpty { parts.append(extra) }
            return parts.joined(separator: ", ")
        }
    }
}
