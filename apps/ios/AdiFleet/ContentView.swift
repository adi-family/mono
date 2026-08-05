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
                    ForEach(model.dashboards[node.petname] ?? []) { dashboard in
                        DashboardRow(node: node, dashboard: dashboard)
                    }
                    if model.listing.contains(node.petname) && model.dashboards[node.petname] == nil {
                        HStack(spacing: 8) {
                            ProgressView()
                            Text("Asking \(node.petname) what it has…")
                                .font(.callout)
                                .foregroundStyle(.secondary)
                        }
                    }
                    // A dashboard is a service, so anything already listed above would otherwise
                    // appear twice — once by name and once by label.
                    ForEach(services(of: node), id: \.self) { service in
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
                    footer(for: node)
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
                ServiceView(node: route.node, service: service, share: route.share, model: model)
            } else {
                AnyServiceView(node: route.node, model: model)
            }
        }
    }

    /// A node's plain services, minus the ones already on screen as a dashboard by name.
    private func services(of node: Node) -> [String] {
        let named = Set((model.dashboards[node.petname] ?? []).compactMap(\.service))
        return node.services.filter { !named.contains($0) }
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

    /// What a node has to say for itself under its section: a nickname it has declared, and why
    /// its dashboards could not be listed.
    @ViewBuilder
    private func footer(for node: Node) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            if node.renamedItself, let pending = node.pending_nickname {
                // §2 rule 4: a node that renames itself files a notice; it never takes over the
                // links this device already has.
                Label("This node now calls itself “\(pending)”. Your name for it is unchanged.",
                      systemImage: "info.circle")
            }
            if let problem = model.listingFailure[node.petname] {
                Label(problem, systemImage: "exclamationmark.triangle")
            }
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

/// One dashboard, as a row.
///
/// The row is a link whether or not the node has shared it: a dashboard this phone may not open
/// yet is one tap away from being one it may, and hiding that behind a separate control would make
/// the common case — your own machine, which you have the password for — the awkward one. What the
/// subtitle owes the reader is that the tap will ask.
private struct DashboardRow: View {
    let node: Node
    let dashboard: NodeDashboard

    var body: some View {
        if let service = dashboard.service {
            NavigationLink(value: Route(node: node, service: service, share: !dashboard.allowed)) {
                label
            }
        } else {
            // Nothing to route to: a dashboard with no `<label>.adi` host of its own is reachable
            // on the node and nowhere else (`docs/fleet.md` §4).
            label.foregroundStyle(.secondary)
        }
    }

    private var label: some View {
        Label {
            VStack(alignment: .leading, spacing: 2) {
                Text(dashboard.name)
                if let note = subtitle {
                    Text(note)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
        } icon: {
            Image(systemName: "rectangle.3.group")
        }
    }

    /// The one thing worth saying about this dashboard, in the order it matters: it cannot be
    /// opened, it has not been shared, it is not running, or what it is for.
    private var subtitle: String? {
        if dashboard.service == nil {
            return "No address on \(node.petname) yet, so there is nothing to open"
        }
        if !dashboard.allowed {
            return "Not shared with this phone yet — tap to ask \(node.petname)"
        }
        if !dashboard.running {
            return "Not running on \(node.petname)"
        }
        return dashboard.description
    }
}

/// Where a tap goes.
///
/// `service == nil` means "let me name one", which is what a `http:*` grant allows and what the
/// protocol cannot enumerate for us. `share` means the node has not granted this label yet, so
/// opening it asks first.
private struct Route: Hashable {
    let node: Node
    let service: String?
    var share = false
}

/// For a node that granted `http:*`: ask which service, since nothing on the wire can list them.
private struct AnyServiceView: View {
    let node: Node
    let model: FleetModel
    @State private var service = ""

    var body: some View {
        Form {
            Section {
                TextField("service label", text: $service)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .font(.system(.body, design: .monospaced))
                NavigationLink("Open") {
                    ServiceView(node: node, service: service, model: model)
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
