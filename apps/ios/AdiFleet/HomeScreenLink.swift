import SwiftUI

/// A link that opens one service on one node — and the sheet that explains how to put it on the
/// Home Screen.
///
/// **iOS gives an app no way to place a Home Screen icon.** Only Safari and the Shortcuts app may,
/// and Apple has said there are no plans to open that up. So this does not pretend to: it produces
/// the one thing a Shortcut needs — a URL — and tells the reader the three taps that turn it into
/// an icon. Dressing that up as a one-tap button the app cannot honour would be worse than saying
/// what it is.
///
/// The URL is a private scheme rather than a universal link because a universal link needs a domain
/// serving an `apple-app-site-association` file, and a viewer that reaches its machines by key over
/// the mesh has no business requiring a website to open one of them.
enum HomeScreenLink {
    /// The scheme declared in `Info.plist`.
    static let scheme = "adifleet"

    /// The link that opens `service` on `node`.
    ///
    /// Both are percent-encoded even though a petname is one DNS label and a service label is
    /// nearly always one too: `AnyServiceView` lets a person type the service by hand, and a
    /// stray space there must not produce a URL that silently fails to parse.
    static func url(node: String, service: String) -> URL? {
        var components = URLComponents()
        components.scheme = scheme
        components.host = "open"
        components.queryItems = [
            URLQueryItem(name: "node", value: node),
            URLQueryItem(name: "service", value: service),
        ]
        return components.url
    }

    /// The `(node, service)` a link names, or `nil` if it is not one of ours.
    ///
    /// Deliberately strict about the host: an `adifleet://` URL that is not `open` is a link from a
    /// future version of this app, and guessing at what it meant is how a deep link ends up opening
    /// the wrong machine.
    static func parse(_ url: URL) -> (node: String, service: String)? {
        guard url.scheme?.lowercased() == scheme, url.host?.lowercased() == "open" else {
            return nil
        }
        let items = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems ?? []
        let value = { (name: String) in
            items.first { $0.name == name }?.value?.trimmingCharacters(in: .whitespaces)
        }
        guard let node = value("node"), !node.isEmpty,
              let service = value("service"), !service.isEmpty
        else {
            return nil
        }
        return (node, service)
    }
}

/// What to do with the link, said in the order a person does it.
struct HomeScreenLinkSheet: View {
    let node: String
    let service: String
    let title: String

    @Environment(\.dismiss) private var dismiss
    @State private var copied = false

    private var link: URL? { HomeScreenLink.url(node: node, service: service) }

    var body: some View {
        NavigationStack {
            List {
                Section {
                    Text(link?.absoluteString ?? "—")
                        .font(.system(.footnote, design: .monospaced))
                        .textSelection(.enabled)
                    Button {
                        UIPasteboard.general.string = link?.absoluteString
                        copied = true
                    } label: {
                        Label(copied ? "Copied" : "Copy link", systemImage: copied ? "checkmark" : "doc.on.doc")
                    }
                    .disabled(link == nil)
                } header: {
                    Text("The link")
                } footer: {
                    Text("Opens “\(title)” on \(node) in this app, from anywhere on the phone.")
                }

                Section {
                    step(1, "Open Shortcuts and add a shortcut.")
                    step(2, "Add the action **Open URL** and paste the link.")
                    step(3, "Share the shortcut → **Add to Home Screen**, then name it and pick an icon.")
                } header: {
                    Text("Putting it on the Home Screen")
                } footer: {
                    Text("iOS lets only Safari and Shortcuts place an icon, so this app cannot do it for you — but a shortcut that opens this link is indistinguishable once it is there.")
                }
            }
            .navigationTitle("Add to Home Screen")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
    }

    private func step(_ n: Int, _ text: LocalizedStringKey) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Text("\(n)")
                .font(.caption.weight(.semibold).monospacedDigit())
                .foregroundStyle(.secondary)
                .frame(width: 16, alignment: .trailing)
            Text(text)
        }
    }
}
