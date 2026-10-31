import SwiftUI
import UIKit

/// Pairing, from the viewer's side.
///
/// A node is enrolled one way: the machine mints a token — `adi-mono mesh invite`, or its Fleet
/// page — and this device spends it by dialling out. The handshake is symmetric (`docs/fleet.md`
/// §8), but the person holding a phone is never the side that should be pasting a `adi-mono mesh
/// join` command into a terminal, so this screen offers only the spending half: scan the code the
/// machine draws, or paste the token. (The other direction — this device minting and a machine
/// spending it from a terminal — was dropped on purpose; a headless box is enrolled from its own
/// terminal, not from here.)
///
/// Nothing here opens a port.
struct PairView: View {
    let model: FleetModel
    @Environment(\.dismiss) private var dismiss

    /// What has been pasted into the spend field.
    @State private var typed = ""
    /// True while the dial is in flight, so the button cannot be pressed twice.
    @State private var joining = false
    /// Whether the camera can be used — three states, because "refused" and "there isn't one" read
    /// identically to the code and completely differently to the person holding the phone.
    @State private var camera: CameraAccess = .ready
    /// The paste fallback. Opens itself when there is no camera to scan with.
    @State private var pasteOpen = false

    var body: some View {
        NavigationStack {
            spend
                .background(ADI.bg)
                .navigationTitle("Pair a node")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) {
                        Button("Done") { dismiss() }
                    }
                }
        }
    }

    /// Spend a token the machine minted — by scanning the code it draws.
    ///
    /// **Scanning is the primary action and pasting is behind a disclosure.** That is not a fresh
    /// call; it is the one the web client already made and wrote down (`adi-mesh-client/src/ui.rs`),
    /// and the reason is arithmetic: a token is over nine hundred characters, the QR is how it gets
    /// onto a phone, and a text field is what you fall back to when the camera is refused, absent,
    /// or pointed at nothing.
    ///
    /// An earlier version of this screen was paste-only, and argued in a comment that a camera
    /// permission would be "bought for nothing". That had it exactly backwards — the phone is the
    /// device with the camera, and reading the code off the machine's screen is the whole flow.
    ///
    /// The disclosure opens itself where there is no camera to call, so the fallback is never
    /// hidden — only quiet.
    private var spend: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                Text("Run `adi-mono mesh invite` on the machine — or open its Fleet page — then point this phone at the code it draws.")
                    .font(ADI.TextStyle.small)
                    .foregroundStyle(ADI.ink2)

                scanner

                DisclosureGroup(isExpanded: $pasteOpen) {
                    pasteField
                } label: {
                    Text("Paste the token instead")
                        .font(ADI.TextStyle.row)
                        .foregroundStyle(ADI.ink2)
                }
                .tint(ADI.ink3)
            }
            .padding(20)
        }
        .task {
            camera = await CameraAccess.request()
            // Where there is no camera, the fallback is the only way through, so it is already open.
            if camera != .ready { pasteOpen = true }
        }
    }

    /// The camera, or the sentence saying why there isn't one.
    @ViewBuilder private var scanner: some View {
        switch camera {
        case .ready:
            ZStack {
                ScanView { token in
                    guard !joining else { return }
                    Task {
                        joining = true
                        defer { joining = false }
                        if await model.spend(invite: token) != nil { dismiss() }
                    }
                }
                .frame(height: 300)
                .clipShape(RoundedRectangle(cornerRadius: ADI.Radius.lg))
                // A reticle, so it is obvious what to point at what. White on the picture: the
                // viewfinder is the one surface here that is not the app's own.
                RoundedRectangle(cornerRadius: ADI.Radius.lg)
                    .stroke(.white.opacity(0.9), lineWidth: 2)
                    .frame(width: 200, height: 200)
                if joining {
                    ProgressView().controlSize(.large).tint(.white)
                }
            }
        case .denied:
            fallbackNote("Camera access is off for adi Fleet, so the code cannot be scanned. Turn it on in Settings, or paste the token below.")
        case .unavailable:
            fallbackNote("This device has no camera, so paste the token the machine printed.")
        }
    }

    private func fallbackNote(_ text: String) -> some View {
        Note(icon: .camera, text: text)
            .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var pasteField: some View {
        VStack(alignment: .leading, spacing: 16) {
                TextEditor(text: $typed)
                    .font(ADI.TextStyle.mono)
                    .foregroundStyle(ADI.code)
                    .scrollContentBackground(.hidden)
                    .frame(height: 140)
                    .adiField()
                    .autocorrectionDisabled()
                    .textInputAutocapitalization(.never)
                    .overlay(alignment: .topLeading) {
                        if typed.isEmpty {
                            Text("adi-invite:…")
                                .font(ADI.TextStyle.mono)
                                .foregroundStyle(ADI.ink3)
                                .padding(.horizontal, 17)
                                .padding(.vertical, 17)
                                .allowsHitTesting(false)
                        }
                    }

                HStack(spacing: 12) {
                    // `PasteButton`, not a plain button reading `UIPasteboard.general.string`.
                    // Reading the pasteboard in code makes iOS put up "adi Fleet would like to
                    // paste from …", a modal the person has to agree to before anything happens —
                    // and this is the screen App Review is walked through, so that alert lands in
                    // the middle of the one flow that has to go smoothly. A `PasteButton` is
                    // system-drawn and system-attested: the tap *is* the consent, so there is no
                    // prompt and the app still never sees the pasteboard it was not given.
                    //
                    // What arrives is whatever was on the pasteboard — the whole `adi-mono mesh
                    // join <token>` command, a numbered line out of a note, the payload without
                    // its name — and it is passed on unedited, because the core reads the token
                    // out of it. See `hasSomethingToSpend`.
                    PasteButton(payloadType: String.self) { items in
                        guard let pasted = items.first else { return }
                        typed = pasted
                    }
                    .labelStyle(.titleAndIcon)
                    // System-drawn (that is its whole point — the tap is the consent), so it keeps
                    // the platform's glyph; the shape and the ink are the design's.
                    .buttonBorderShape(.roundedRectangle(radius: ADI.Radius.md))
                    .tint(ADI.ink)

                    Button {
                        Task {
                            joining = true
                            defer { joining = false }
                            if await model.spend(invite: typed) != nil { dismiss() }
                        }
                    } label: {
                        Group {
                            if joining {
                                ProgressView()
                            } else {
                                HStack(spacing: 6) {
                                    LucideIcon(icon: .link)
                                    Text("Pair")
                                }
                            }
                        }
                    }
                    // The one orange on the pasting path: pairing is what the screen is for.
                    .buttonStyle(.adi(.accent, wide: true))
                    .disabled(joining || !Self.hasSomethingToSpend(typed))
                }
        }
        .padding(.top, 8)
    }

    /// Whether there is anything here worth spending — the enabled state of **Pair**, and nothing
    /// more than that.
    ///
    /// It deliberately does not try to find the token: the core does that, in `adi-mesh`'s `token`
    /// module, and it is handed the paste *whole* so it can. This screen used to pre-trim to the
    /// first whitespace-separated part beginning `adi-invite:`, which quietly threw away every
    /// shape that trimming cannot fix — a payload whose `adi-invite:` was left behind by a
    /// double-tap (iOS ends a word at `:` and `-`), a token its carrier wrapped across two lines,
    /// a stray backtick from the copy it was pasted out of. Each of those then failed with "not an
    /// adi invite", which reads, to the person holding the token, as a lie. That is what an App
    /// Store reviewer met on 2026-09-02.
    private static func hasSomethingToSpend(_ text: String) -> Bool {
        !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }
}
