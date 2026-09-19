import AppKit
import SwiftUI
import WebKit

/// The page behind `PanelWindow`: a `WKWebView`, everything the bar above it needs to know, and
/// the two decisions a web view has to make on its own — what leaves for the default browser, and
/// what a page's `confirm()` is answered with.
///
/// This is a viewer, not a second control panel. Every button in it is a browser button, and the
/// thing it browses is the panel `adi-app` already serves; the house rule that control logic lives
/// in `adi-core` is not bent by it, because nothing here decides anything about the install.
///
/// Deliberately *not* `@MainActor`. WebKit's delegate protocols are not actor-isolated, and a
/// main-actor class satisfying them warns today and is rejected under Swift 6. Everything here is
/// main-thread work regardless: WebKit calls its delegates there, and the views are on the main
/// actor already.
final class WebPanel: NSObject, ObservableObject, Identifiable {
    let id = UUID()

    /// Where this tab opens: the panel for the first one, a dashboard's host for the rest.
    let home: URL

    /// Whether this tab is the app itself. The app's tab cannot be closed, and cannot be navigated
    /// off its own host — a link that would take it somewhere else opens a tab instead, which is
    /// what makes "the first tab is always the control panel" true rather than merely usual.
    ///
    /// Not the same thing as `pinned` below, though both keep a tab where it is: this one is a fact
    /// about the window and is decided when the tab is made; that one is something the operator did.
    let isApp: Bool

    /// Whether the operator asked to keep this tab (`PanelTabs.togglePin`).
    ///
    /// A pinned tab sits at the front of the strip, so it keeps the same ⌘-number as tabs open and
    /// close around it, and it stops answering the two gestures that close a tab *under the pointer*
    /// — the ⨯ on the chip, which it no longer draws, and the wheel-click. ⌘W and the tab menu's own
    /// *Close Tab* still close it: both name what they are about to do, which is exactly what the
    /// other two cannot.
    @Published var pinned = false

    /// What the operator called this tab, if they called it anything (the tab's own menu, through
    /// `rename`). It beats the page's own title in `label` — a dashboard that titles every one of
    /// its pages "Grafana" is the case this exists for.
    @Published private(set) var name: String?

    /// The page's own title, for the tab to wear. Empty until the page says.
    @Published private(set) var title = ""
    @Published private(set) var canGoBack = false
    @Published private(set) var canGoForward = false
    @Published private(set) var loading = false
    /// Why the last navigation failed, until the next one starts.
    @Published private(set) var failure: String?

    /// Where a link that wants a window of its own goes. Set by `PanelTabs`, which is the only
    /// thing that can make one — a page cannot open a tab, it can only ask for one.
    var openInNewTab: ((URL) -> Void)?

    let webView: WKWebView

    /// Held for as long as the tab is: dropping these stops the observations.
    private var observations: [NSKeyValueObservation] = []

    init(home: URL, isApp: Bool = false) {
        self.home = home
        self.isApp = isApp

        let configuration = WKWebViewConfiguration()
        // The default, persistent data store. The panel keeps view state in `localStorage` and
        // the front door's hosts are stable across launches on purpose, so a window that forgot
        // where it was every time would be a demo of the panel rather than the panel.
        configuration.websiteDataStore = .default()
        configuration.userContentController.addUserScript(Self.nativeFlag)
        webView = PageView(frame: .zero, configuration: configuration)
        super.init()

        webView.navigationDelegate = self
        webView.uiDelegate = self
        // The two-finger swipe, which is the only back gesture anyone tries. Safe here in a way it
        // is not on the phone: this window has a back button on screen as well.
        webView.allowsBackForwardNavigationGestures = true
        // The page surface behind the page — the frame before the panel paints, and the rubber
        // band past its end. Without it both are the opaque white a `WKWebView` starts on, which
        // in a dark window reads as a flash of something broken.
        webView.underPageBackgroundColor = NSColor(ADI.bg)
        if #available(macOS 13.3, *) {
            // Right-click → Inspect Element. This is a window onto your own machine's panel, and
            // the person most likely to open it is the one changing that panel.
            webView.isInspectable = true
        }

        // The title is watched rather than sampled. A page sets `document.title` when it likes —
        // at parse time, or after its script has run, which for a dashboard is *after* the load
        // finished — so reading it in the navigation callbacks catches it only sometimes. Sampling
        // it was flaky in exactly the way that matters: the same tab came up "ADI — agents on your
        // own machine" one run and "landing.adi" the next, and the tab's name is the only thing
        // there is to click on.
        observations = [
            webView.observe(\.title) { [weak self] view, _ in
                self?.title = view.title ?? ""
            },
        ]
    }

    // MARK: what the bar does

    /// Load the panel, once. Called when the window appears, which SwiftUI may do more than once
    /// per window — the guard is what keeps a redraw from throwing away where you had navigated
    /// to.
    func start() {
        guard webView.url == nil else { return }
        load(home)
    }

    func goBack() { webView.goBack() }
    func goForward() { webView.goForward() }
    func stop() { webView.stopLoading() }

    /// Reload — or load this tab's page, if the first attempt failed and there is nothing to
    /// reload.
    func reload() {
        if webView.url == nil { load(home) } else { webView.reload() }
    }

    /// ⇧⌘R: reload without the cache, the way a browser's hard reload does.
    ///
    /// Worth its own keystroke here rather than being a browser habit copied for the sake of it. A
    /// dashboard is a bun-served bundle that somebody may have just rebuilt, and an ordinary reload
    /// is entitled to hand back the JavaScript it already has — which is precisely the case where
    /// pressing ⌘R twice and seeing no change is most confusing.
    func reloadFromOrigin() {
        if webView.url == nil { load(home) } else { webView.reloadFromOrigin() }
    }

    /// Send this tab somewhere. Used when a link asks for a host that is already open in a tab:
    /// the tab is brought forward and taken to the page the link named, rather than a second tab
    /// for the same dashboard appearing every time somebody clicks it.
    func load(_ url: URL) {
        failure = nil
        webView.load(URLRequest(url: url))
    }

    /// Hand the current page to the default browser — the escape hatch out of this window, for
    /// the extension, the profile or the devtools somebody would rather use.
    func openInDefaultBrowser() {
        NSWorkspace.shared.open(webView.url ?? home)
    }

    // MARK: zoom

    /// How far this page is blown up. Per tab, and not remembered across launches: a dashboard
    /// zoomed in to read one number is not a preference, and a window that came back at 175% with
    /// no visible reason why would be a bug report.
    @Published private(set) var zoom: Double = 1

    /// The stops ⌘+ and ⌘− walk, which are a browser's. A multiplier per press would take the same
    /// number of presses to get somewhere useful and land on values nobody chose.
    private static let zoomStops: [Double] = [0.5, 0.67, 0.75, 0.9, 1, 1.1, 1.25, 1.5, 1.75, 2, 2.5, 3]

    func zoomIn() { setZoom(Self.zoomStops.first { $0 > zoom + 0.001 } ?? Self.zoomStops[Self.zoomStops.count - 1]) }
    func zoomOut() { setZoom(Self.zoomStops.last { $0 < zoom - 0.001 } ?? Self.zoomStops[0]) }
    func resetZoom() { setZoom(1) }

    /// Whether ⌘0 has anything to undo — so *Actual Size* greys out on a page that is already at it.
    var zoomed: Bool { abs(zoom - 1) > 0.001 }

    private func setZoom(_ value: Double) {
        zoom = value
        webView.pageZoom = value
    }

    // MARK: find in page

    /// Whether the find bar is open on this tab, and what is in it. Per tab, because a search is
    /// about the page you are looking at — switching tabs and finding the other tab's query still
    /// in the box would be a search of the wrong document.
    @Published private(set) var finding = false
    @Published var query = ""

    /// Whether the last search hit anything — nil before there has been one. There is deliberately
    /// no "3 of 17": `WKFindResult` reports `matchFound` and nothing else, and a count computed
    /// separately in JavaScript would disagree with WebKit's own highlighting often enough
    /// (shadow DOM, `visibility: hidden`, text split across nodes) to be worse than no count.
    @Published private(set) var found: Bool?

    /// Bumped by every ⌘F, so the bar takes the keyboard again when it is already open — which is
    /// what pressing ⌘F a second time is *for*.
    @Published private(set) var findFocus = 0

    func startFind() {
        finding = true
        findFocus += 1
    }

    /// Close the bar and take WebKit's highlight off the page with it. Leaving the match selected
    /// behind a closed bar is a page that looks like something is still selected for no reason.
    func endFind() {
        finding = false
        found = nil
        webView.evaluateJavaScript("window.getSelection().removeAllRanges()", completionHandler: nil)
    }

    /// Search, from wherever the last match left off. Wrapping, because a find that stops at the
    /// bottom of the document makes you scroll up to search the top again.
    ///
    /// Not trimmed: a search for a word with a space either side of it is a real search, and one
    /// for `  ` is the user's business.
    func find(backwards: Bool = false) {
        guard !query.isEmpty else {
            found = nil
            return
        }
        let configuration = WKFindConfiguration()
        configuration.backwards = backwards
        configuration.caseSensitive = false
        configuration.wraps = true
        webView.find(query, configuration: configuration) { [weak self] result in
            self?.found = result.matchFound
        }
    }

    /// Tell the page it is in the app, before any of it runs.
    ///
    /// The app wears the mark and the wordmark as its first tab, so the page must not draw them
    /// again a few pixels below (`adi-webapp`'s `native::in_app`, read by `launcher::brand` and
    /// `launcher::floating`). A user script rather than a User-Agent suffix: the UA is sent to
    /// every host this window opens and to every request they make, and this is nobody's business
    /// but the page's.
    ///
    /// The menu those triggers opened is still there and still ⌘K — the page listens for the
    /// keystroke itself, and nothing here touches that.
    ///
    /// `atDocumentStart`, `forMainFrameOnly: false`, so it is true before the WASM boots and in
    /// whatever the page frames.
    private static let nativeFlag = WKUserScript(
        source: "window.__adiNative = true;",
        injectionTime: .atDocumentStart,
        forMainFrameOnly: false
    )

    /// What this tab says it is: the name it was given, else the page's own title, else the host it
    /// is on until there is one.
    var label: String {
        if let name { return name }
        if !title.isEmpty { return title }
        return webView.url?.host ?? home.host ?? "…"
    }

    /// Call this tab something. Blank puts it back to whatever the page calls itself — which is the
    /// only undo a rename needs, and is what an emptied box means rather than a tab with no name.
    func rename(_ text: String) {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        name = trimmed.isEmpty ? nil : trimmed
    }

    /// Read the page's own state back into the bar.
    private func sync() {
        canGoBack = webView.canGoBack
        canGoForward = webView.canGoForward
        title = webView.title ?? ""
    }

    /// Whether a URL belongs to this install: its own zone (`app.adi`, `<project>.adi`, and the
    /// `….n.adi` names a node's services are fronted under) or a loopback port, which is how a
    /// service is reached before it has a host of its own.
    ///
    /// The zone is the flavour's, not the literal `adi`: a dev build browses `.adi-dev`, and
    /// sending its links to the release install's zone is exactly the mix-up `ADIDomain` exists
    /// to prevent.
    static func isLocal(_ url: URL) -> Bool {
        guard let host = url.host?.lowercased() else { return url.isFileURL }
        if host == Core.domain || host.hasSuffix(".\(Core.domain)") { return true }
        return ["localhost", "127.0.0.1", "::1"].contains(host)
    }
}

// MARK: - the one key the page does not get

/// The tab's web view, with a single keystroke handed back to the window.
///
/// WebKit claims ⌃⇥ as a key equivalent of its own — inside a page it walks the focus ring — and a
/// *view's* key equivalent is offered before the menu bar's, so **Show Next Tab** never saw the
/// keystroke while a page had the keyboard. Measured, with the web view first responder: ⌃⇥ moved
/// nothing, the very same event handed straight to the main menu switched tabs, and a ⌃-letter item
/// on that menu fired normally — so what WebKit takes is the Tab key, not the modifier.
///
/// Between the window's tabs and the page's focus ring the window wins: ⌃⇥ is declined here and
/// falls through to the menu bar, and everything else is passed on untouched. Tab on its own — which
/// is how a form is actually walked — is not a key equivalent and never comes through here at all.
final class PageView: WKWebView {
    /// Tab.
    private static let tabKey: UInt16 = 48

    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        let mods = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        if event.keyCode == Self.tabKey, mods.contains(.control), !mods.contains(.command) {
            return false
        }
        return super.performKeyEquivalent(with: event)
    }
}

// MARK: - navigation

extension WebPanel: WKNavigationDelegate {
    func webView(_ webView: WKWebView,
                 decidePolicyFor navigationAction: WKNavigationAction,
                 decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
        guard let url = navigationAction.request.url else {
            decisionHandler(.allow)
            return
        }

        // A click that leaves this machine leaves this window: an issue tracker, a docs site,
        // somebody's repository. A *click* only — a redirect, a form post or a subframe is the
        // page doing its job, and a page whose own hop through some other origin opened in Safari
        // would never come back to finish.
        if navigationAction.navigationType == .linkActivated, !Self.isLocal(url) {
            NSWorkspace.shared.open(url)
            decisionHandler(.cancel)
            return
        }

        // Two ways a navigation on this machine becomes a tab instead.
        //
        // Anything that would carry the *app's* tab off the panel's own host — not clicks only:
        // a `window.location`, a meta refresh and a redirect are navigations too, and an invariant
        // that holds for one kind of navigation and not the others is not an invariant. It is what
        // makes "the first tab is the control panel" a fact rather than a habit.
        //
        // And a ⌘-click, anywhere, because that is what a ⌘-click has always meant.
        let mainFrame = navigationAction.targetFrame?.isMainFrame ?? true
        let commandClick = navigationAction.navigationType == .linkActivated
            && navigationAction.modifierFlags.contains(.command)
        if mainFrame, leavesAppHost(url) || commandClick, let openInNewTab {
            openInNewTab(url)
            decisionHandler(.cancel)
            return
        }

        decisionHandler(.allow)
    }

    /// Whether this navigation would carry the app's own tab off the host it opens on.
    private func leavesAppHost(_ url: URL) -> Bool {
        isApp && url.host != home.host
    }

    func webView(_ webView: WKWebView, didStartProvisionalNavigation navigation: WKNavigation!) {
        loading = true
        failure = nil
        sync()
    }

    func webView(_ webView: WKWebView, didCommit navigation: WKNavigation!) {
        sync()
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        loading = false
        sync()
    }

    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        report(error)
    }

    func webView(_ webView: WKWebView,
                 didFailProvisionalNavigation navigation: WKNavigation!,
                 withError error: Error) {
        report(error)
    }

    private func report(_ error: Error) {
        loading = false
        sync()
        // `NSURLErrorCancelled` is "another navigation replaced this one" — a link pressed while
        // the previous page was still loading. Nothing went wrong, and putting an error sheet over
        // a page that is loading correctly would be the only thing that had.
        let failure = error as NSError
        guard !(failure.domain == NSURLErrorDomain && failure.code == NSURLErrorCancelled) else {
            return
        }
        self.failure = error.localizedDescription
    }
}

// MARK: - what a page asks of its window

extension WebPanel: WKUIDelegate {
    /// `target="_blank"` and `window.open`, which is how the panel links to every dashboard and
    /// every hive service it lists. A page asking for a window of its own gets a **tab** when the
    /// thing it names is on this machine, and the default browser when it is not.
    ///
    /// Returning nil is what tells WebKit not to expect a web view back — the tab makes its own.
    func webView(_ webView: WKWebView,
                 createWebViewWith configuration: WKWebViewConfiguration,
                 for navigationAction: WKNavigationAction,
                 windowFeatures: WKWindowFeatures) -> WKWebView? {
        if let url = navigationAction.request.url {
            if !Self.isLocal(url) {
                NSWorkspace.shared.open(url)
            } else if let openInNewTab {
                openInNewTab(url)
            } else {
                webView.load(navigationAction.request)
            }
        }
        return nil
    }

    // The three JavaScript panels, native. Not optional politeness: the panel guards every
    // destructive action with `confirm()` (`adi-webapp`'s `ui::confirm`), and WebKit's answer to a
    // `WKUIDelegate` that does not implement this is *false* — so "Delete" would silently do
    // nothing, over and over, with no error anywhere to explain it.

    func webView(_ webView: WKWebView,
                 runJavaScriptAlertPanelWithMessage message: String,
                 initiatedByFrame frame: WKFrameInfo,
                 completionHandler: @escaping () -> Void) {
        let alert = sheet(message)
        alert.addButton(withTitle: "OK")
        present(alert) { _ in completionHandler() }
    }

    func webView(_ webView: WKWebView,
                 runJavaScriptConfirmPanelWithMessage message: String,
                 initiatedByFrame frame: WKFrameInfo,
                 completionHandler: @escaping (Bool) -> Void) {
        let alert = sheet(message)
        alert.addButton(withTitle: "OK")
        alert.addButton(withTitle: "Cancel")
        present(alert) { completionHandler($0 == .alertFirstButtonReturn) }
    }

    func webView(_ webView: WKWebView,
                 runJavaScriptTextInputPanelWithPrompt prompt: String,
                 defaultText: String?,
                 initiatedByFrame frame: WKFrameInfo,
                 completionHandler: @escaping (String?) -> Void) {
        let alert = sheet(prompt)
        let field = NSTextField(frame: NSRect(x: 0, y: 0, width: 280, height: 22))
        field.stringValue = defaultText ?? ""
        alert.accessoryView = field
        alert.addButton(withTitle: "OK")
        alert.addButton(withTitle: "Cancel")
        alert.window.initialFirstResponder = field
        present(alert) { completionHandler($0 == .alertFirstButtonReturn ? field.stringValue : nil) }
    }

    /// The file picker, for the same reason the three above are here. A page cannot open one by
    /// itself — `<input type=file>` asks its embedder — and WebKit's answer to a `WKUIDelegate`
    /// that does not implement this is *nothing at all*: the composer's paperclip opens no window,
    /// logs no error, and looks like a dead button. Verified against WebKit directly: it asks for
    /// the panel even though the input behind the paperclip is `display:none`, so nothing on the
    /// page's side needs changing.
    func webView(_ webView: WKWebView,
                 runOpenPanelWith parameters: WKOpenPanelParameters,
                 initiatedByFrame frame: WKFrameInfo,
                 completionHandler: @escaping ([URL]?) -> Void) {
        let panel = NSOpenPanel()
        panel.allowsMultipleSelection = parameters.allowsMultipleSelection
        panel.canChooseDirectories = parameters.allowsDirectories
        panel.canChooseFiles = true
        panel.canCreateDirectories = false
        // No `allowedContentTypes`: the composer sends no `accept` on purpose — a PDF, a CSV or a
        // log is as attachable as a screenshot — and a filter invented here would grey out the
        // file the operator opened this panel to send.
        if let window = webView.window {
            panel.beginSheetModal(for: window) { response in
                completionHandler(response == .OK ? panel.urls : nil)
            }
        } else {
            completionHandler(panel.runModal() == .OK ? panel.urls : nil)
        }
    }

    private func sheet(_ message: String) -> NSAlert {
        let alert = NSAlert()
        alert.messageText = message
        alert.alertStyle = .informational
        return alert
    }

    /// As a sheet on the window the page is in, so it is obvious *which* page is asking — and
    /// modal only to that window, so the rest of the app keeps working. Falls back to a modal run
    /// in the rare moment the view has no window yet.
    private func present(_ alert: NSAlert, then answer: @escaping (NSApplication.ModalResponse) -> Void) {
        if let window = webView.window {
            alert.beginSheetModal(for: window, completionHandler: answer)
        } else {
            answer(alert.runModal())
        }
    }
}
