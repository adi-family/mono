import SwiftUI
import WebKit

/// One service on one node, in a web view.
///
/// The URL is `http://127.0.0.1:<port>` — a loopback listener inside this app that carries the
/// request to the node over the mesh. That is a real origin, which is the point: relative URLs,
/// cookies, `localStorage` and WebSocket upgrades all behave exactly as they do when the same
/// dashboard is opened on a Mac, because `docs/fleet.md` §4 already forbids a page from knowing its
/// own address.
struct ServiceView: View {
    let node: Node
    let service: String

    @State private var port: UInt16?
    @State private var failure: String?
    @State private var challenge = AuthChallenge()

    var body: some View {
        Group {
            if let port {
                WebView(
                    url: URL(string: "http://127.0.0.1:\(port)/")!,
                    node: node.petname,
                    challenge: challenge
                )
                .ignoresSafeArea(edges: .bottom)
            } else if let failure {
                ContentUnavailableView {
                    Label("Cannot open \(service)", systemImage: "exclamationmark.triangle")
                } description: {
                    Text(failure)
                }
            } else {
                ProgressView().controlSize(.large)
            }
        }
        .navigationTitle("\(service) · \(node.petname)")
        .navigationBarTitleDisplayMode(.inline)
        .task {
            do {
                port = try await Mesh.shared.open(node: node.petname, service: service)
            } catch {
                failure = error.localizedDescription
            }
        }
        .alert("Sign in to \(node.petname)", isPresented: $challenge.asking) {
            TextField("Username", text: $challenge.username)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
            SecureField("Password", text: $challenge.password)
            Button("Sign in") { challenge.submit(saveFor: node.petname) }
            Button("Cancel", role: .cancel) { challenge.cancel() }
        } message: {
            Text("This node asks for a password. It was shown once on the node when it paired.")
        }
    }
}

/// The node's Basic-auth gate, answered from the Keychain and — only when that fails — from a
/// prompt.
///
/// A gate that a viewer could skip would not be a gate (§5), so this never guesses: an empty
/// Keychain or a rejected credential means asking, and a cancel means the request fails rather than
/// retrying without one.
@MainActor
@Observable
final class AuthChallenge {
    var asking = false
    var username = ""
    var password = ""

    /// Held while the alert is up; the web view's request is suspended until it is called.
    private var pending: ((URLCredential?) -> Void)?

    /// Answer a challenge: stored credential first, prompt second.
    ///
    /// `previousFailures` is what distinguishes "we have never been asked" from "what we sent was
    /// refused" — without it, a rotated password would loop forever, re-sending the stale one.
    func answer(node: String, previousFailures: Int, completion: @escaping (URLCredential?) -> Void) {
        if previousFailures == 0, let stored = Keychain.credential(for: node) {
            completion(URLCredential(user: stored.username, password: stored.password, persistence: .forSession))
            return
        }
        username = Keychain.credential(for: node)?.username ?? "adi"
        password = ""
        pending = completion
        asking = true
    }

    /// Send what was typed, and remember it — a password typed once should not be typed twice.
    func submit(saveFor node: String) {
        Keychain.save(.init(username: username, password: password), for: node)
        pending?(URLCredential(user: username, password: password, persistence: .forSession))
        pending = nil
    }

    func cancel() {
        pending?(nil)
        pending = nil
    }
}

/// The `WKWebView` itself.
private struct WebView: UIViewRepresentable {
    let url: URL
    /// Which node this is, for the Keychain. Passed down rather than derived from `url`, because
    /// the URL is a loopback port and says nothing about whose service is behind it.
    let node: String
    let challenge: AuthChallenge

    func makeCoordinator() -> Coordinator { Coordinator(node: node, challenge: challenge) }

    func makeUIView(context: Context) -> WKWebView {
        // The default, persistent data store: the port a service is served on is stable across
        // launches precisely so the page's own state survives with it (see `viewer::ports`), and a
        // non-persistent store would throw that away on every launch.
        let view = WKWebView(frame: .zero, configuration: WKWebViewConfiguration())
        view.navigationDelegate = context.coordinator
        view.allowsBackForwardNavigationGestures = true
        view.load(URLRequest(url: url))
        return view
    }

    func updateUIView(_ view: WKWebView, context: Context) {}

    final class Coordinator: NSObject, WKNavigationDelegate {
        private let node: String
        private let challenge: AuthChallenge

        init(node: String, challenge: AuthChallenge) {
            self.node = node
            self.challenge = challenge
        }

        func webView(
            _ webView: WKWebView,
            didReceive challenge: URLAuthenticationChallenge,
            completionHandler: @escaping (URLSession.AuthChallengeDisposition, URLCredential?) -> Void
        ) {
            guard challenge.protectionSpace.authenticationMethod == NSURLAuthenticationMethodHTTPBasic else {
                completionHandler(.performDefaultHandling, nil)
                return
            }
            // Keyed by petname, not by the realm the node sends: the realm is the node's own
            // nickname, and §2 is explicit that what this device calls a node is its own business.
            let node = self.node
            let failures = challenge.previousFailureCount
            Task { @MainActor in
                self.challenge.answer(node: node, previousFailures: failures) { credential in
                    if let credential {
                        completionHandler(.useCredential, credential)
                    } else {
                        completionHandler(.cancelAuthenticationChallenge, nil)
                    }
                }
            }
        }
    }
}
