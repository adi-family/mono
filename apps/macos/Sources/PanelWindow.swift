import SwiftUI

/// The control panel, in a window of the app's own rather than in whatever browser the Mac opens
/// `.adi` links with.
///
/// The panel is a web app served by `adi-app` over the local front door, and this is that page in a
/// `WKWebView` (`WebPanel`) under a 48px bar — the app shell's own top bar (`design/DESIGN.md` §7),
/// carrying the three controls a page needs and the tabs. It is a viewer: everything it can do, the
/// panel in Safari can do too. What it buys is that the control panel is *in the app* — one icon in
/// the Dock, one window that remembers where it was, and no tab lost in a window of thirty.
///
/// **The first tab is the app.** Dashboards and services open beside it and are closed when they
/// are done with; the panel is neither (`PanelTabs`). There is no address bar: nothing is typed
/// into this window, because everything it can reach is a click away on the page it opens on.
struct PanelWindow: View {
    /// The scene id. `AppModel` asks for the panel and `ContentView` opens it, so the name has to
    /// be one constant rather than a string typed in both places.
    static let sceneID = "panel"

    @StateObject private var tabs: PanelTabs

    /// `.key` while this window has the keyboard, `.active`/`.inactive` otherwise — the one thing
    /// the menu bar has to know that the window does not tell it by existing.
    @Environment(\.controlActiveState) private var active

    init(home: URL) {
        _tabs = StateObject(wrappedValue: PanelTabs(home: home))
    }

    var body: some View {
        VStack(spacing: 0) {
            bar
            Hairline()
            Selected(panel: tabs.selected)
        }
        .frame(minWidth: 720, minHeight: 480)
        .background(ADI.bg)
        .background(WindowChrome())
        .preferredColorScheme(.dark)
        // What ⌘R and ⌘W act on, and when (`PanelCommands`). The window tells the menu bar it is
        // here rather than the menu bar going looking: `controlActiveState` is `.key` exactly while
        // this window has the keyboard, which is the question the two items need answered.
        .onAppear { FrontPanel.shared.opened(tabs, front: active == .key) }
        .onDisappear { FrontPanel.shared.closed() }
        .onChange(of: active) { FrontPanel.shared.front($0 == .key) }
    }

    /// The tab bar: what is open, and the way out to a real browser.
    ///
    /// A band of its own under the window's title bar, rather than tabs sitting *in* the title bar
    /// beside the traffic lights. The lights keep their line and the tabs get theirs.
    ///
    /// There is no back, forward or reload button. They were three glyphs spent on what a page in
    /// this window almost never needs — it opens on the panel and everything it reaches is a click
    /// away on it — and the web view keeps all three on its own context menu for the times it does.
    private var bar: some View {
        HStack(spacing: 12) {
            TabStrip(tabs: tabs)
            OpenElsewhere(panel: tabs.selected)
        }
        .padding(.horizontal, 12)
        .frame(height: 48)
        .background(ADI.bgSide)
    }
}

/// The tab in front: its page, the find bar when it has one, and the name the window wears.
///
/// A view of its own, and that is the whole point of it. `PanelWindow` observes `PanelTabs`, which
/// publishes when tabs are opened, closed or switched — and *not* when the tab in front changes
/// something about itself. So `tabs.selected.finding` read from the window's own body was a value
/// nothing had subscribed to: ⌘F set the flag and the bar never appeared (measured — with the bar
/// "open", there was no text field anywhere in the window). The page's title had the same fault, and
/// looked less like one: the window's name was right whenever anything *else* had forced a redraw.
///
/// `@ObservedObject` here is the subscription to the tab itself, which is what both need.
private struct Selected: View {
    @ObservedObject var panel: WebPanel

    var body: some View {
        VStack(spacing: 0) {
            // Under the strip and above the page, the way Safari's is — not floating over the
            // page's top-right corner, which is where a browser puts it to avoid reflowing a
            // document it does not own. This window owns its layout, and a band that pushes the
            // page down never covers the thing being searched for.
            if panel.finding {
                FindBar(panel: panel)
                Hairline()
            }
            Page(panel: panel)
        }
        .navigationTitle(panel.label)
    }
}

/// What is open, in the order it was opened, starting with the app.
///
/// Scrolls rather than shrinks: a strip that divided the width by the number of tabs would answer
/// "which of these is the deploy dashboard?" with eight chips reading "da…". Tabs keep their names
/// and the strip slides.
private struct TabStrip: View {
    @ObservedObject var tabs: PanelTabs

    var body: some View {
        ScrollView(.horizontal) {
            HStack(spacing: 4) {
                ForEach(Array(tabs.tabs.enumerated()), id: \.element.id) { index, tab in
                    TabChip(
                        panel: tab,
                        active: tab.id == tabs.selection,
                        index: index,
                        select: { tabs.select(tab) },
                        close: { tabs.close(tab) },
                        pin: { tabs.togglePin(tab) }
                    )
                }
            }
            .padding(.vertical, 6)
        }
        .scrollIndicators(.hidden)
    }
}

/// One tab: what the page calls itself, and — unless it is the app's, or kept — a way to be rid of
/// it.
///
/// A chosen segment is a tone change and not an outline or a colour (§6): `--bg-active` under ink,
/// against `--ink-2` on nothing for the rest.
///
/// **Three gestures, and they are a browser's**, because a strip of tabs is a thing people already
/// know how to use: click shows a tab, wheel-click closes it, right-click opens the menu of what
/// there is no room to put on a chip. The app's own tab answers the first and nothing else — it
/// cannot be closed, the brand is not renamed, and it is already where pinning would put a tab — so
/// it has no menu rather than a menu of three dead items.
private struct TabChip: View {
    @ObservedObject var panel: WebPanel
    let active: Bool
    /// Its place in the strip, which is also its ⌘-number for the first nine.
    let index: Int
    let select: () -> Void
    let close: () -> Void
    let pin: () -> Void

    @State private var hovering = false
    /// Whether this tab is being renamed, and what has been typed so far.
    @State private var renaming = false
    @State private var draft = ""
    /// Bumped every time the box opens rather than fixed, because *Rename…* can be chosen while the
    /// box is already open — the menu is still on the chip when the chip is a field — and the field
    /// takes the keyboard only when this number changes.
    @State private var renameFocus = 0

    var body: some View {
        if panel.isApp {
            chip
        } else {
            chip
                .contextMenu { menu }
                // The wheel-click, and only where the ⨯ is: the two are the same gesture — close
                // this, now, from under the pointer — so a tab that declines one declines the other.
                .overlay { if closableHere { MiddleClick(action: close) } }
        }
    }

    /// The tab as a control: a button, or the box its name is being typed into.
    @ViewBuilder
    private var chip: some View {
        if renaming {
            nameField
        } else {
            button
        }
    }

    private var button: some View {
        Button(action: select) {
            label
                // Room for the close button to sit over, so a long title is cut by the glyph
                // rather than running under it.
                .padding(.trailing, closableHere ? 18 : 0)
                .frame(maxWidth: 200, alignment: .leading)
                .fixedSize(horizontal: false, vertical: true)
        }
        .buttonStyle(TabStyle(active: active, hovering: hovering))
        .modifier(TabShortcut(index: index))
        .overlay(alignment: .trailing) {
            if closableHere {
                Button(action: close) {
                    LucideIcon(icon: .x, size: .sm, label: "Close \(panel.label)")
                        // A stroked `Shape` is hit **on its stroke**, so without a shape of its own
                        // this button's target was the two ~0.9pt diagonals of the ⨯ itself and
                        // every other pixel of the glyph fell through to the tab underneath —
                        // selecting a tab that was not chosen, and doing nothing at all on the one
                        // that was. Measured on a real bundle: 4 of the 49 points in the glyph's
                        // 14pt box closed the tab, and all four lay on a diagonal.
                        //
                        // 4pt of slop on every side, which keeps the glyph exactly where it was
                        // (4 + 4 = the 8 it stood off the chip's edge) and makes the target 22.
                        .padding(4)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .foregroundStyle(hovering || active ? ADI.ink2 : ADI.ink3)
                .padding(.trailing, 4)
            }
        }
        .onHover { hovering = $0 }
        .help(panel.webView.url?.absoluteString ?? panel.home.absoluteString)
    }

    /// Whether a gesture *under the pointer* may close this tab. The app's own never closes; a
    /// pinned one is kept on purpose, and neither the ⨯ nor a wheel-click says out loud what it is
    /// about to do — which is the whole of what pinning guards against (`PanelTabs.togglePin`).
    private var closableHere: Bool { !panel.isApp && !panel.pinned }

    /// What the tab wears.
    ///
    /// The app's tab is the mark and the wordmark rather than whatever the panel currently calls
    /// itself: it is not one page among the open ones, it is the thing they were opened from, and
    /// it says so by being the only one that never changes. §10's top-bar treatment — the mark
    /// beside the wordmark at 15/600 — at the tab-sized 16.
    ///
    /// A kept tab wears the pin, which is what is left to say so once the ⨯ has gone: the strip is
    /// a row of names, and a name that has quietly stopped having a close button would otherwise
    /// read as a name that has lost one.
    @ViewBuilder
    private var label: some View {
        if panel.isApp {
            HStack(spacing: 8) {
                ADILogo(size: 16)
                Text("adi")
                    .font(ADI.TextStyle.wordmark)
            }
        } else {
            HStack(spacing: 8) {
                if panel.pinned {
                    LucideIcon(icon: .pin, size: .sm, label: "Pinned")
                        .foregroundStyle(ADI.ink3)
                }
                Text(panel.label)
                    .font(ADI.TextStyle.row)
                    .lineLimit(1)
                    .truncationMode(.tail)
            }
        }
    }

    /// The three things a tab can be asked to do that there is no room for on the chip.
    ///
    /// Title case, and not the app's sentence case (§8): this is an AppKit menu, opening beside the
    /// menu bar's own *Close Tab* and *Reopen Closed Tab*, and a menu that disagreed with the menu
    /// bar about capitalisation would read as the odd one out rather than as the house style. Close
    /// sits last and behind a divider, where every Mac menu keeps the item you cannot undo.
    @ViewBuilder
    private var menu: some View {
        Button("Rename…") { startRename() }
        Button(panel.pinned ? "Unpin Tab" : "Pin Tab", action: pin)
        Divider()
        Button("Close Tab", action: close)
    }

    // MARK: renaming

    /// The name, in place: the chip becomes an input of the same size, so nothing moves and it is
    /// plainly *this* tab being named — rather than a sheet over the window asking about a tab it
    /// has to name in its own text.
    private var nameField: some View {
        InlineField(text: $draft,
                    placeholder: panel.label,
                    focus: renameFocus,
                    selectsAll: true,
                    submit: { typed, _ in commitRename(typed) },
                    cancel: { renaming = false },
                    ended: { commitRename($0) })
            // The chip's own padding (`TabStyle`) and a width that does not grow with what is typed,
            // so the tabs either side of it stay where they are while a name is being changed.
            .frame(width: 180, height: 20)
            .padding(.horizontal, 10)
            .padding(.vertical, 4)
            .background(ADI.bgRaise, in: RoundedRectangle(cornerRadius: ADI.Radius.md))
            .overlay(RoundedRectangle(cornerRadius: ADI.Radius.md).stroke(ADI.lineStrong, lineWidth: 1))
    }

    /// Seeded with the name the tab is wearing, so renaming a dashboard is editing what it already
    /// says rather than typing it out from nothing — and the field selects it, so the hand that
    /// meant to replace it can.
    private func startRename() {
        draft = panel.label
        renameFocus += 1
        renaming = true
    }

    /// Blank means *go back to what the page calls itself* (`WebPanel.rename`).
    ///
    /// **The name is handed in, not read back out of `draft`.** What the field editor holds is the
    /// answer at the instant ↩ is pressed; `draft` is the same text after a round trip out through a
    /// binding and back through SwiftUI state, which is one more thing that has to have happened
    /// yet. The box is the source, so the box says.
    ///
    /// Guarded, because there are three ways out of the box — ↩, clicking away, and the field losing
    /// the keyboard — and on some paths more than one of them arrives.
    private func commitRename(_ typed: String) {
        guard renaming else { return }
        renaming = false
        panel.rename(typed)
    }
}

/// The wheel-click, which in every browser closes the tab under the pointer.
///
/// An `NSView` over the chip that claims **only** the middle button: `hitTest` answers with itself
/// while the event being dispatched is a middle-button one and with nil otherwise, so every ordinary
/// click falls straight through to the SwiftUI button underneath. SwiftUI has no gesture for this —
/// its `onTapGesture` and its buttons are the left button's alone — and a view placed over a tab
/// that took *all* the clicks would have to reimplement the tab.
///
/// Down and up are both required to land on the chip, which is what makes a press-and-drag-away a
/// change of mind rather than a closed tab.
private struct MiddleClick: NSViewRepresentable {
    let action: () -> Void

    func makeNSView(context: Context) -> Catcher {
        let view = Catcher()
        view.action = action
        return view
    }

    func updateNSView(_ view: Catcher, context: Context) {
        view.action = action
    }

    final class Catcher: NSView {
        var action: () -> Void = {}
        /// Whether the press that is in progress started here.
        private var pressed = false

        override func hitTest(_ point: NSPoint) -> NSView? {
            guard let event = NSApp.currentEvent, Self.isMiddle(event) else { return nil }
            return super.hitTest(point)
        }

        /// So a wheel-click closes a tab in a window that was not already the key one, which is
        /// what it does in a browser.
        override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

        override func otherMouseDown(with event: NSEvent) {
            pressed = event.buttonNumber == Self.middleButton
        }

        override func otherMouseUp(with event: NSEvent) {
            defer { pressed = false }
            guard pressed, event.buttonNumber == Self.middleButton,
                  bounds.contains(convert(event.locationInWindow, from: nil)) else { return }
            action()
        }

        /// Left is 0, right is 1, the wheel is 2. Every other button an eight-button mouse has is
        /// 3 and up, and none of them means "close this".
        private static let middleButton = 2

        private static func isMiddle(_ event: NSEvent) -> Bool {
            switch event.type {
            case .otherMouseDown, .otherMouseUp, .otherMouseDragged:
                return event.buttonNumber == middleButton
            default:
                return false
            }
        }
    }
}

/// ⌘1…⌘9 select a tab, as everywhere else. Past the ninth there is no number left to press, so
/// the chip is simply clicked — the same place the shortcut would have taken you.
private struct TabShortcut: ViewModifier {
    let index: Int

    func body(content: Content) -> some View {
        if index < 9, let key = "\(index + 1)".first {
            content.keyboardShortcut(KeyEquivalent(key), modifiers: .command)
        } else {
            content
        }
    }
}

/// A tab's fill (§6): the chosen one is a tone change on the bar, the rest are nothing until
/// pointed at.
private struct TabStyle: ButtonStyle {
    let active: Bool
    let hovering: Bool

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .foregroundStyle(active ? ADI.ink : ADI.ink2)
            .padding(.horizontal, 10)
            .padding(.vertical, 5)
            .background(fill(pressed: configuration.isPressed),
                        in: RoundedRectangle(cornerRadius: ADI.Radius.md))
            .contentShape(RoundedRectangle(cornerRadius: ADI.Radius.md))
    }

    private func fill(pressed: Bool) -> Color {
        if active { return ADI.bgActive }
        if pressed { return ADI.bgActive }
        return hovering ? ADI.bgHover : Color.clear
    }
}

/// ⌘F: find in this page.
///
/// Per tab (`WebPanel.finding`), so switching tabs does not carry one page's search onto another —
/// and coming back to a tab finds the bar as it was left.
///
/// **There is no "3 of 17".** The public API is `WKWebView.find(_:configuration:)`, whose
/// `WKFindResult` reports `matchFound` and nothing else; a count computed separately in JavaScript
/// would disagree with WebKit's own highlighting on shadow DOM, on hidden text and on a word split
/// across nodes — often enough that a wrong number is worse than no number. So the bar answers the
/// question it can answer honestly: found, or not found.
private struct FindBar: View {
    @ObservedObject var panel: WebPanel

    var body: some View {
        HStack(spacing: 8) {
            LucideIcon(icon: .search, size: .sm, label: "Find in page")
                .foregroundStyle(ADI.ink3)

            InlineField(text: $panel.query,
                        placeholder: "Find in page",
                        focus: panel.findFocus,
                        colour: missed ? ADI.err : ADI.ink,
                        submit: { _, backwards in panel.find(backwards: backwards) },
                        cancel: { panel.endFind() })
                .frame(maxWidth: 280, minHeight: 20)
                // As it is typed, which is what every browser does and what makes a search of a
                // long dashboard worth starting before you have finished the word.
                .onChange(of: panel.query) { _ in panel.find() }

            if missed {
                Text("No matches")
                    .font(ADI.TextStyle.small)
                    .foregroundStyle(ADI.ink3)
            }

            Spacer(minLength: 0)

            // ⇧↩ as well as the chevron, because a hand already on the keyboard for the query is
            // not going to reach for the mouse to step back one match.
            BarButton(icon: .chevronUp, label: "Previous match", enabled: searchable) {
                panel.find(backwards: true)
            }
            .keyboardShortcut(.return, modifiers: .shift)

            BarButton(icon: .chevronDown, label: "Next match", enabled: searchable) {
                panel.find()
            }

            Button("Done") { panel.endFind() }
                .buttonStyle(.adi(.quiet, .small))
                // Esc, which is how every find bar on this machine closes.
                .keyboardShortcut(.cancelAction)
        }
        .padding(.horizontal, 12)
        .frame(height: 40)
        .background(ADI.bgSide)
    }

    private var searchable: Bool { !panel.query.isEmpty }

    /// Looked for, and not there. Not the same as "nothing has been looked for yet", which is what
    /// an empty box is and which nothing should be red about.
    private var missed: Bool { panel.found == false && searchable }
}

/// The box a word is typed into inside this window: the find bar's query, and a tab's new name.
///
/// AppKit's `NSTextField` rather than SwiftUI's `TextField`, for one measured reason: at the moment
/// ⌘F is pressed the window's first responder is the tab's `WKWebView`, and `@FocusState` moves
/// focus **within SwiftUI's own focus system** — it does not take the keyboard back from an AppKit
/// view that already holds it. Measured: with `.focused($focused)` set from `onAppear`, the first
/// responder was still the web view a full second later, so ⌘F opened a bar that could not be typed
/// into, and Escape had nothing to reach.
///
/// A text field can simply ask for the job. Its field editor is also the one place ⎋ and ↩ can be
/// caught while the box has the keyboard, which is where they matter.
private struct InlineField: NSViewRepresentable {
    @Binding var text: String
    let placeholder: String
    /// Bumped to mean *put the keyboard back in the box* — which is what a second ⌘F is for.
    let focus: Int
    /// Red while the query matches nothing; the field's own, because an `NSTextField` does not
    /// inherit SwiftUI's `foregroundStyle`.
    var colour: Color = ADI.ink
    /// Whether taking the keyboard also selects what is in the box. A rename wants it — the name is
    /// there to be typed over — and a find does not, where the cursor belongs after the word.
    var selectsAll = false
    /// ↩: what the box holds, and whether Shift was down — which for the find bar means the previous
    /// match and for a box with one answer means nothing at all.
    ///
    /// The text is handed over rather than read back out of `text` afterwards, because the binding
    /// is a round trip through SwiftUI state and the answer is wanted as it stands *now*.
    let submit: (String, Bool) -> Void
    let cancel: () -> Void
    /// Finished without ↩ or ⎋: the keyboard went elsewhere, or the next click landed outside the
    /// box. Nil where that is not an ending — the find bar stays open while you click about the page
    /// it is searching, which is most of what a find bar is for.
    var ended: ((String) -> Void)?

    func makeNSView(context: Context) -> NSTextField {
        let field = NSTextField(string: text)
        field.delegate = context.coordinator
        // Stated rather than assumed: a text field that is not editable also does not accept first
        // responder, and refusing the keyboard is indistinguishable from never being offered it.
        field.isEditable = true
        field.isSelectable = true
        field.isBordered = false
        field.drawsBackground = false
        field.focusRingType = .none
        field.placeholderString = placeholder
        field.font = Self.face
        field.lineBreakMode = .byTruncatingTail
        field.cell?.usesSingleLineMode = true
        if ended != nil { context.coordinator.watchClicksOutside(field) }
        return field
    }

    func updateNSView(_ field: NSTextField, context: Context) {
        // The coordinator calls back into *this* struct, and SwiftUI makes a new one every pass —
        // so it is handed the current one rather than keeping the one the view was made with.
        context.coordinator.owner = self
        if field.stringValue != text { field.stringValue = text }
        field.textColor = NSColor(colour)
        field.placeholderString = placeholder
        guard context.coordinator.taken != focus else { return }
        context.coordinator.taken = focus
        let selectEverything = selectsAll
        // A turn of the loop late, deliberately: on the pass that inserts the bar the field is not
        // in a window yet, and a first-responder request made then is dropped on the floor.
        DispatchQueue.main.async {
            field.window?.makeFirstResponder(field)
            // After the field editor exists, which it does not until the line above has run: the
            // selection belongs to the editor, not to the field.
            if selectEverything { field.currentEditor()?.selectAll(nil) }
        }
    }

    static func dismantleNSView(_ field: NSTextField, coordinator: Coordinator) {
        coordinator.stopWatching()
    }

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    final class Coordinator: NSObject, NSTextFieldDelegate {
        var owner: InlineField
        /// The `focus` this has already acted on. `-1`, so the first pass always takes the keyboard.
        var taken = -1
        /// Whether this box has already had its answer. Three things can end one — ↩, ⎋ and the
        /// keyboard leaving — and on several paths more than one of them arrives: ⎋ takes the field
        /// off the screen, which is itself the end of an edit, and a box that committed on the way
        /// out would make Escape mean *save*.
        private var answered = false
        private var watcher: Any?

        init(_ owner: InlineField) { self.owner = owner }

        func controlTextDidChange(_ note: Notification) {
            guard let field = note.object as? NSTextField else { return }
            owner.text = field.stringValue
        }

        /// ⎋ closes the box, ↩ submits it and ⇧↩ submits it backwards — through the field editor's
        /// own dispatch, which is what sees these keys while the box has the keyboard.
        func control(_ control: NSControl,
                     textView: NSTextView,
                     doCommandBy selector: Selector) -> Bool {
            switch selector {
            case #selector(NSResponder.cancelOperation(_:)):
                answered = true
                owner.cancel()
            case #selector(NSResponder.insertNewline(_:)),
                 #selector(NSResponder.insertLineBreak(_:)):
                // The field editor's text, not the field's: the field is only brought up to date
                // when editing ends, so on ↩ it is still holding what the box said a keystroke ago.
                owner.submit(textView.string, NSApp.currentEvent?.modifierFlags.contains(.shift) == true)
            default:
                return false
            }
            return true
        }

        /// The keyboard left the box — for another field, or for the web view.
        func controlTextDidEndEditing(_ note: Notification) {
            finish((note.object as? NSTextField)?.stringValue ?? "")
        }

        /// A click that is not in the box ends the edit, and that needs an event monitor: SwiftUI's
        /// buttons and AppKit's web view do not take first responder when they are clicked, so a
        /// click on the tab beside this one is not a *change of keyboard focus* and
        /// `controlTextDidEndEditing` never comes. Without this the box stays open on a tab nobody
        /// is looking at any more.
        func watchClicksOutside(_ field: NSTextField) {
            watcher = NSEvent.addLocalMonitorForEvents(
                matching: [.leftMouseDown, .rightMouseDown, .otherMouseDown]
            ) { [weak self, weak field] event in
                guard let self, let field, event.window === field.window else { return event }
                let point = field.convert(event.locationInWindow, from: nil)
                // Handed back untouched either way: this is a bystander, not a click handler.
                guard !field.bounds.contains(point) else { return event }
                let typed = field.currentEditor()?.string ?? field.stringValue
                // After this click has been delivered. Taking the box off the screen inside the
                // dispatch of the click that ended it is how the click itself gets lost.
                DispatchQueue.main.async { self.finish(typed) }
                return event
            }
        }

        func stopWatching() {
            if let watcher { NSEvent.removeMonitor(watcher) }
            watcher = nil
        }

        private func finish(_ typed: String) {
            guard !answered else { return }
            answered = true
            owner.ended?(typed)
        }
    }

    /// `ADI.TextStyle.row` is a SwiftUI `Font`, which an `NSTextField` cannot wear — the same face
    /// at the same size, in AppKit's currency.
    private static let face: NSFont = ADI.hasGeist
        ? NSFont(name: "Geist", size: 13.5) ?? .systemFont(ofSize: 13.5)
        : .systemFont(ofSize: 13.5)
}

/// The escape hatch: this page, in the default browser.
private struct OpenElsewhere: View {
    @ObservedObject var panel: WebPanel

    var body: some View {
        BarButton(icon: .arrowUpRight, label: "Open in browser") {
            panel.openInDefaultBrowser()
        }
    }
}

/// The tab that is open, and — over it — the reason it is not.
private struct Page: View {
    @ObservedObject var panel: WebPanel

    var body: some View {
        ZStack {
            WebViewHost(panel: panel)
            if let failure = panel.failure {
                unreachable(failure)
            }
        }
        // The tab's own identity, so switching tabs swaps the web view rather than reusing the
        // one that is already mounted with another tab's page in it.
        .id(panel.id)
        .onAppear { panel.start() }
    }

    /// The page could not be loaded — the front door is down, the name does not resolve, the host
    /// refused. Over the page rather than beside it: what is underneath is the *previous* page, and
    /// leaving it there unmarked would say the navigation had worked.
    private func unreachable(_ message: String) -> some View {
        VStack(spacing: 12) {
            LucideIcon(icon: .triangleAlert, size: .xl)
                .foregroundStyle(ADI.ink3)
            Text("Cannot open this page")
                .font(ADI.TextStyle.section)
                .foregroundStyle(ADI.ink)
            Text(message)
                .font(ADI.TextStyle.small)
                .foregroundStyle(ADI.ink2)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: 380)
            Button("Try again") { panel.reload() }
                .buttonStyle(.adi(.normal))
                .padding(.top, 4)
        }
        .padding(32)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(ADI.bg)
    }
}

/// One of the bar's icon buttons.
///
/// Unlabelled, which §9 allows only for a named few — close, filter, the ⋯ menu. These join that
/// set on the same argument: back, forward, reload and "opens elsewhere" are the shape browsers
/// have had for thirty years, and four words across the bar would crowd out the tabs. Each still
/// names itself to a screen reader and on hover.
private struct BarButton: View {
    let icon: Lucide
    let label: String
    var enabled = true
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            LucideIcon(icon: icon, size: .md, label: label)
        }
        .buttonStyle(.adi(.quiet, .small))
        .disabled(!enabled)
        .help(label)
    }
}

/// The two things about this window SwiftUI has no modifier for.
///
/// Keeps the title bar **AppKit's** while it wears the app's colour — which is the whole trick, and
/// took four wrong turns to find.
///
/// A title bar is worth more than the pixels it occupies: double-click to zoom or minimise (as that
/// Mac is set to), drag, the window menu. All of it comes free while AppKit owns the strip, and
/// none of it can be had once a view covers it — SwiftUI's hosting view claims every click in the
/// band it spans, and `allowsHitTesting(false)` does not give them back (measured: the hit is the
/// host, not the title bar).
///
/// So the band must be coloured *without* covering it, which is `titlebarAppearsTransparent` over a
/// window whose background is `--bg-side`. Each half needed a different fight:
///
/// * **The transparency** is `.hiddenTitleBar`'s doing, not this type's. Set by hand on an ordinary
///   window it is silently reverted — measured true at t+3s and false at t+6s, which is exactly how
///   a working setting looks like a broken one.
/// * **`fullSizeContentView`** comes with `.hiddenTitleBar` and is the half that hurts, so it is
///   removed here and kept out. With it gone the strip hit-tests as `NSThemeFrame` again.
/// * **The background colour** holds once set — it was never the problem, though it looked like it
///   while the transparency underneath kept flipping back.
///
/// The cost is the window's title, which `.hiddenTitleBar` also hides and which cannot be restored
/// (`titleVisibility = .visible` is re-hidden from `makeNSView`, from `updateNSView`, and deferred
/// a turn of the loop past either). The tab strip under the band names every open page anyway.
///
/// It also sets `isMovableByWindowBackground`, so the window can be dragged by its tab bar the way
/// any window with a strip of tabs can.
private struct WindowChrome: NSViewRepresentable {
    final class Coordinator {
        var observer: NSObjectProtocol?
    }

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeNSView(context: Context) -> NSView {
        let view = NSView()
        // On the next turn of the loop: a view has no window at the moment it is made.
        DispatchQueue.main.async {
            guard let window = view.window else { return }
            Self.apply(to: window)
            // And on every window update from then on, because SwiftUI re-asserts the window's
            // configuration long after the view is built — a bounded timer loses that race, and
            // `updateNSView` alone stops running once the page settles.
            context.coordinator.observer = NotificationCenter.default.addObserver(
                forName: NSWindow.didUpdateNotification, object: window, queue: .main
            ) { note in
                Self.apply(to: note.object as? NSWindow)
            }
        }
        return view
    }

    func updateNSView(_ view: NSView, context: Context) {
        DispatchQueue.main.async { Self.apply(to: view.window) }
    }

    static func dismantleNSView(_ view: NSView, coordinator: Coordinator) {
        if let observer = coordinator.observer {
            NotificationCenter.default.removeObserver(observer)
        }
    }

    /// Only what has drifted, because this runs on every window update and assigning either one
    /// unconditionally would invalidate the title bar continuously.
    private static func apply(to window: NSWindow?) {
        guard let window else { return }
        // The content view must NOT span the title bar: that is what hands the band's clicks to
        // SwiftUI and takes double-click-to-zoom away. Removed here rather than avoided at the
        // scene, because the same modifier that brings it also brings the transparency the colour
        // depends on.
        if window.styleMask.contains(.fullSizeContentView) {
            window.styleMask.remove(.fullSizeContentView)
        }
        // Belongs to `.hiddenTitleBar`; asserted anyway so the intent is stated where the colour is.
        if !window.titlebarAppearsTransparent {
            window.titlebarAppearsTransparent = true
        }
        let surface = NSColor(ADI.bgSide)
        if window.backgroundColor != surface {
            window.backgroundColor = surface
        }
        if !window.isMovableByWindowBackground {
            window.isMovableByWindowBackground = true
        }
    }
}

/// The `WKWebView`, in SwiftUI.
///
/// The view is made by `WebPanel` rather than here: the bar drives it, and a representable that
/// built its own would hand SwiftUI a fresh, blank page every time it decided to rebuild the body —
/// losing the history, the scroll position and whatever was half-typed into the panel.
private struct WebViewHost: NSViewRepresentable {
    let panel: WebPanel

    func makeNSView(context: Context) -> some NSView { panel.webView }
    func updateNSView(_ view: some NSView, context: Context) {}
}
