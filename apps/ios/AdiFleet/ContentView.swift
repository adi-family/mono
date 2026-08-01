import SwiftUI

/// The fleet: every node this phone has paired with, and a way to add one.
struct ContentView: View {
    @State private var model = FleetModel()
    @State private var pairing = false

    var body: some View {
        NavigationStack {
            Group {
                if model.nodes.isEmpty {
                    empty
                } else {
                    list
                }
            }
            .navigationTitle("Fleet")
            .toolbar {
                ToolbarItem(placement: .primaryAction) {
                    Button {
                        pairing = true
                    } label: {
                        Label("Pair a node", systemImage: "plus")
                    }
                    .disabled(!model.ready)
                }
                ToolbarItem(placement: .status) {
                    identity
                }
            }
            .refreshable { await model.refresh() }
            .sheet(isPresented: $pairing) { PairView(model: model) }
            .alert(
                "Something went wrong",
                isPresented: .init(get: { model.failure != nil }, set: { if !$0 { model.failure = nil } })
            ) {
                Button("OK", role: .cancel) { model.failure = nil }
            } message: {
                Text(model.failure ?? "")
            }
        }
        .task { await model.start() }
    }

    private var empty: some View {
        ContentUnavailableView {
            Label("No nodes yet", systemImage: "point.3.connected.trianglepath.dotted")
        } description: {
            Text("Pair a machine and its services show up here — reached by key over the mesh, with no port open on either side.")
        } actions: {
            Button("Pair a node") { pairing = true }
                .buttonStyle(.borderedProminent)
                .disabled(!model.ready)
        }
    }

    private var list: some View {
        List {
            ForEach(model.nodes) { node in
                Section {
                    ForEach(node.services, id: \.self) { service in
                        NavigationLink(value: Route(node: node, service: service)) {
                            Label(service, systemImage: icon(for: service))
                        }
                    }
                    if node.any_service {
                        NavigationLink(value: Route(node: node, service: nil)) {
                            Label("Open another service…", systemImage: "ellipsis.curlybraces")
                                .foregroundStyle(.secondary)
                        }
                    }
                } header: {
                    header(for: node)
                } footer: {
                    if node.renamedItself, let pending = node.pending_nickname {
                        // §2 rule 4: a node that renames itself files a notice; it never takes over
                        // the links this device already has.
                        Label("This node now calls itself “\(pending)”. Your name for it is unchanged.",
                              systemImage: "info.circle")
                    }
                }
                .swipeActions {
                    Button("Unpair", role: .destructive) {
                        Task { await model.forget(node) }
                    }
                }
            }
        }
        .navigationDestination(for: Route.self) { route in
            if let service = route.service {
                ServiceView(node: route.node, service: service)
            } else {
                AnyServiceView(node: route.node)
            }
        }
    }

    private func header(for node: Node) -> some View {
        HStack {
            Text(node.petname)
            if node.petname == model.justPaired {
                Text("new")
                    .font(.caption2.weight(.semibold))
                    .padding(.horizontal, 6)
                    .padding(.vertical, 2)
                    .background(Color.accentColor.opacity(0.15))
                    .clipShape(Capsule())
            }
            Spacer()
            Text(node.shortKey)
                .font(.system(.caption2, design: .monospaced))
                .foregroundStyle(.tertiary)
        }
    }

    /// This device's own key, which is what a node's operator authorizes.
    private var identity: some View {
        VStack(spacing: 2) {
            Text(model.ready ? "on the mesh" : "connecting…")
                .font(.caption2)
                .foregroundStyle(model.ready ? .secondary : .tertiary)
            if !model.key.isEmpty {
                Text(model.key.prefix(12) + "…")
                    .font(.system(size: 9, design: .monospaced))
                    .foregroundStyle(.tertiary)
            }
        }
    }

    /// A guess at the right symbol. Cosmetic only — a service the app has never heard of gets the
    /// generic one and works exactly the same.
    private func icon(for service: String) -> String {
        switch service {
        case "app": "square.grid.2x2"
        case "api": "chevron.left.forwardslash.chevron.right"
        default: "globe"
        }
    }
}

/// Where a tap goes. `service == nil` means "let me name one", which is what a `http:*` grant
/// allows and what the protocol cannot enumerate for us.
private struct Route: Hashable {
    let node: Node
    let service: String?
}

/// For a node that granted `http:*`: ask which service, since nothing on the wire can list them.
private struct AnyServiceView: View {
    let node: Node
    @State private var service = ""

    var body: some View {
        Form {
            Section {
                TextField("service label", text: $service)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .font(.system(.body, design: .monospaced))
                NavigationLink("Open") {
                    ServiceView(node: node, service: service)
                }
                .disabled(service.isEmpty)
            } header: {
                Text("Service on \(node.petname)")
            } footer: {
                Text("The label a service has on the node — what it answers to as `<label>.adi` there. This node granted every one of them, and the mesh has no way to list them, so it has to be named.")
            }
        }
        .navigationTitle(node.petname)
        .navigationBarTitleDisplayMode(.inline)
    }
}
