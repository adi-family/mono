import SwiftUI
import WebKit

/// One service on one node, in a web view.
///
/// The URL is `http://127.0.0.1:<port>` — a loopback listener inside this app that carries the
/// request to the node over the mesh. That is a real origin, which is the point: relative URLs,
/// cookies, `localStorage` and WebSocket upgrades all behave exactly as they do when the same
/// dashboard is opened on a Mac, because the fleet design already forbids a page from knowing its
/// own address.
struct ServiceView: View {
    let node: Node
    let service: String
    /// True when the node has not granted this label yet, so it is asked to before the port is
    /// bound. Doing it in this order matters: a listener bound to a service the node refuses would
    /// answer every request with the same refusal page, and reloading would never clear it.
    var share = false
    /// Where on the service to land. A path and query, never an address: the origin is whatever
    /// port the mesh bound.
    var path = "/"
    let model: FleetModel

    @State private var port: UInt16?
    /// Set instead of `port` for the debug panel, whose services are not behind the mesh.
    @State private var direct: URL?
    @State private var failure: String?
    @State private var asking = false
    @State private var challenge = AuthChallenge()

    var body: some View {
        Group {
            if failure == nil, let url = direct ?? port.flatMap({ URL(string: "http://127.0.0.1:\($0)\(path)") ?? URL(string: "http://127.0.0.1:\($0)/") }) {
                WebView(
                    url: url,
                    node: node.petname,
                    challenge: challenge,
                    // A page that never arrived is a dark, empty screen with nothing to say why —
                    // which reads as the app being broken rather than the service being down.
                    failed: { failure = $0 }
                )
            } else if let failure {
                VStack(spacing: 12) {
                    LucideIcon(icon: .triangleAlert, size: .xl)
                        .foregroundStyle(ADI.ink3)
                    Text("Cannot open \(service)")
                        .font(ADI.TextStyle.section)
                        .foregroundStyle(ADI.ink)
                    Text(failure)
                        .font(ADI.TextStyle.small)
                        .foregroundStyle(ADI.ink2)
                        .multilineTextAlignment(.center)
                        .frame(maxWidth: 320)
                }
                .padding(32)
            } else {
                VStack(spacing: 12) {
                    ProgressView().controlSize(.large)
                    if asking {
                        Text("Asking \(node.petname) to share \(service)…")
                            .font(ADI.TextStyle.small)
                            .foregroundStyle(ADI.ink3)
                    }
                }
            }
        }
        // The page surface behind every state, and behind the safe areas the page is allowed to
        // run under. Without it the screen is dark only where the page has painted, and the strip
        // under the home indicator stays the system background — which reads as a rendering bug.
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(ADI.bg)
        .ignoresSafeArea(edges: .bottom)
        // A page gets the whole screen under the bar; the tabs come back with the list.
        .toolbar(.hidden, for: .tabBar)
        .colorScheme(.dark)
        .navigationTitle("\(service) · \(node.petname)")
        .navigationBarTitleDisplayMode(.inline)
        // The bar stays, page or no page. It used to be hidden once the port was bound, to spend
        // the ~44pt on the dashboard instead — a dashboard is one origin with its own header
        //, so the second bar restated the row that had just been tapped.
        //
        // That trade was wrong, and it was wrong in the direction that matters. What it bought was
        // a strip of pixels; what it cost was the way out: an opened dashboard became a full-screen
        // page of somebody else's making with no back button and no title, leaving only the edge
        // swipe — a gesture a person has to already know, that a page handling its own horizontal
        // drags can swallow, and that nothing on screen suggests. Whatever is behind that bar, the
        // app has to remain obviously escapable from it.
        .task {
            #if DEBUG
            if let panel = DevPanel.current, panel.node.petname == node.petname {
                await panel.wake(service)
                direct = panel.address(of: service, path: path)
                if direct == nil { failure = "\(service) has no port on the debug panel." }
                return
            }
            #endif
            if share {
                asking = true
                let refusal = await model.share(service, on: node)
                if let refusal {
                    asking = false
                    failure = refusal
                    return
                }
                // The grant is written, and the node still refuses for a moment.
                //
                // Its gateway serves from an in-memory snapshot of the registry and re-reads it
                // every five seconds (`adi-mesh/src/gateway.rs`, `RELOAD_INTERVAL`), so a request
                // sent the instant the grant lands is judged against a registry that has never
                // heard of it. The fleet design says this in as many words: *a client must wait
                // out that window before it reports a pairing as done, or the operator pairs
                // successfully and is then told they are not authorized.*
                //
                // Without this wait the page loads, the node answers "this machine holds no grant
                // for that service", and that error is what stays on screen — the web view has no
                // reason to retry and the app cannot see the refusal, because it happens inside
                // the tunnel after `open` has already succeeded. It was a race the phone usually
                // won and the iPad usually lost, which is the worst kind: it looks like a flake.
                try? await Task.sleep(for: .seconds(6))
                asking = false
            }
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
    /// Called with the reason when the page itself could not be loaded.
    let failed: (String) -> Void

    func makeCoordinator() -> Coordinator { Coordinator(node: node, challenge: challenge, failed: failed) }

    func makeUIView(context: Context) -> WKWebView {
        // The default, persistent data store: the port a service is served on is stable across
        // launches precisely so the page's own state survives with it (see `viewer::ports`), and a
        // non-persistent store would throw that away on every launch.
        let view = WKWebView(frame: .zero, configuration: WKWebViewConfiguration())
        view.navigationDelegate = context.coordinator
        // The page surface under the page, in three places because a white flash can come from any
        // of them: the view itself before the first paint, the scroll view when a page is
        // rubber-banded past its own end, and the default opaque white a WKWebView starts with. A
        // dashboard paints `--bg` itself; the one frame before it does is what is being removed.
        let ground = UIColor(ADI.bg)
        view.isOpaque = false
        view.backgroundColor = ground
        view.scrollView.backgroundColor = ground
        // Off, and that is what makes hiding the navigation bar safe. Both this and the stack's
        // interactive pop want the same left-edge swipe, and with no bar on screen the pop is the
        // only way out — so the two cannot both have it. In-page history is the smaller loss: a
        // dashboard is a single-origin app with its own controls, while being unable to leave it is
        // a dead end.
        view.allowsBackForwardNavigationGestures = false
        view.load(URLRequest(url: url))
        return view
    }

    func updateUIView(_ view: WKWebView, context: Context) {}

    final class Coordinator: NSObject, WKNavigationDelegate {
        private let node: String
        private let challenge: AuthChallenge
        private let failed: (String) -> Void

        init(node: String, challenge: AuthChallenge, failed: @escaping (String) -> Void) {
            self.node = node
            self.challenge = challenge
            self.failed = failed
        }

        /// The first load failing: nothing answered on the port, or the connection dropped before
        /// a page came back. A failure *inside* a loaded page is the page's own business.
        func webView(
            _ webView: WKWebView,
            didFailProvisionalNavigation navigation: WKNavigation!,
            withError error: Error
        ) {
            // Cancelled is what a redirect or a second load looks like, not a failure.
            guard (error as NSError).code != NSURLErrorCancelled else { return }
            failed(error.localizedDescription)
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
