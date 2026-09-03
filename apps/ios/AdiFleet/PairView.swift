import CoreImage.CIFilterBuiltins
import SwiftUI
import UIKit

/// Pairing, from the viewer's side — in both directions.
///
/// The handshake is symmetric and which side dials is a deployment choice (`docs/fleet.md` §8), so
/// this screen offers both:
///
/// - **Invite a machine** — this device mints a token and the machine spends it by running
///   `adi-mono mesh join`. The whole job is getting the token onto the machine, which is why Copy
///   and Share matter more than the QR does.
/// - **Enter an invite** — the machine minted (`adi-mono mesh invite`) and this device dials it.
///   The direction that works when whoever holds the phone is not sitting at the machine, and the
///   only one that works when they have no terminal at all.
///
/// Nothing here opens a port, in either direction.
struct PairView: View {
    let model: FleetModel
    @Environment(\.dismiss) private var dismiss

    /// Which way round this pairing is going.
    ///
    /// `spend` is first and is the default, because it is the one almost everybody uses: the
    /// machine draws a code and the phone reads it. `mint` is the other direction — this device
    /// makes the invite and the machine spends it from a terminal — which is the right way round
    /// for enrolling a headless box and the wrong way round for a person holding a phone.
    private enum Direction: String, CaseIterable {
        case spend = "Scan a code"
        case mint = "Invite a machine"
    }

    @State private var direction: Direction = .spend
    @State private var token: String?
    @State private var copied = false
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
            VStack(spacing: 0) {
                Picker("Direction", selection: $direction) {
                    ForEach(Direction.allCases, id: \.self) { Text($0.rawValue).tag($0) }
                }
                .pickerStyle(.segmented)
                .padding(.horizontal, 20)
                .padding(.bottom, 8)

                switch direction {
                case .mint:
                    if let token {
                        invite(token)
                    } else if model.ready {
                        VStack(spacing: 12) {
                            ProgressView().controlSize(.large)
                            Text("Minting an invite…")
                                .font(ADI.TextStyle.small)
                                .foregroundStyle(ADI.ink3)
                        }
                        .frame(maxHeight: .infinity)
                    } else {
                        waiting
                    }
                case .spend:
                    spend
                }
            }
            .background(ADI.bg)
            .navigationTitle("Pair a node")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
        .task {
            guard model.ready else { return }
            token = await model.invite()
        }
        // Only the minting direction finishes without anything happening on this screen — the node
        // dials back whenever it gets round to it. Polling is how the sheet notices; a second is
        // well inside "it just worked". The spending direction dismisses itself, below.
        .task {
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(1))
                await model.collectPairings()
                if model.justPaired != nil {
                    await model.refresh()
                    dismiss()
                    return
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
                        if await model.spend(invite: Self.tokenIn(token)) != nil { dismiss() }
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
                    // The whole `adi-mono mesh join <token>` command is what people copy, so what
                    // arrives is trimmed to the token by `tokenIn` rather than refused.
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
                            if await model.spend(invite: Self.tokenIn(typed)) != nil { dismiss() }
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
                    .disabled(joining || Self.tokenIn(typed).isEmpty)
                }
        }
        .padding(.top, 8)
    }

    /// The `adi-invite:…` token inside whatever was pasted.
    ///
    /// `mesh invite` prints a whole command and the QR path yields the bare token, so both shapes
    /// arrive here. Splitting on whitespace and taking the part that looks like a token accepts
    /// each of them, and a token with a stray newline around it — which is what a copy off a
    /// terminal usually is.
    private static func tokenIn(_ text: String) -> String {
        text.split(whereSeparator: \.isWhitespace)
            .first { $0.hasPrefix("adi-invite:") }
            .map(String.init)
            ?? text.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// Shown until the relay session is up. Minting before then would produce a token that only
    /// works on the local network, which is a worse answer than waiting a moment.
    private var waiting: some View {
        VStack(spacing: 12) {
            LucideIcon(icon: .radio, size: .xl)
                .foregroundStyle(ADI.ink3)
            Text("Connecting to the mesh")
                .font(ADI.TextStyle.section)
                .foregroundStyle(ADI.ink)
            Text("An invite needs a relay session, so a node has something to dial. This usually takes a few seconds.")
                .font(ADI.TextStyle.small)
                .foregroundStyle(ADI.ink2)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 320)
        }
        .padding(32)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private func invite(_ token: String) -> some View {
        ScrollView {
            VStack(spacing: 24) {
                // The code on a white plate: a QR drawn on the dark page is an inverted one that
                // many cameras refuse, and the white is also its quiet zone.
                if let image = qr(token) {
                    Image(uiImage: image)
                        .interpolation(.none)
                        .resizable()
                        .scaledToFit()
                        .frame(maxWidth: 260)
                        .padding(12)
                        .background(.white)
                        .clipShape(RoundedRectangle(cornerRadius: ADI.Radius.lg))
                }

                VStack(alignment: .leading, spacing: 8) {
                    Text("Run this on the node")
                        .font(ADI.TextStyle.section)
                        .foregroundStyle(ADI.ink)
                    Text("adi-mono mesh join …")
                        .font(ADI.TextStyle.mono)
                        .foregroundStyle(ADI.code)
                    Text("The node dials back over the mesh. It needs no open port, and it will print nothing you have to type here — this device already holds the password.")
                        .font(ADI.TextStyle.small)
                        .foregroundStyle(ADI.ink2)
                }
                .frame(maxWidth: .infinity, alignment: .leading)

                Text(token)
                    .font(ADI.TextStyle.mono)
                    .foregroundStyle(ADI.ink3)
                    .lineLimit(4)
                    .truncationMode(.middle)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .adiCodeBlock()

                HStack(spacing: 12) {
                    // The one orange on the minting path: getting the command onto the machine is
                    // the whole job.
                    Button {
                        UIPasteboard.general.string = "adi-mono mesh join \(token)"
                        copied = true
                    } label: {
                        HStack(spacing: 6) {
                            LucideIcon(icon: copied ? .check : .copy)
                            Text(copied ? "Copied" : "Copy command")
                        }
                    }
                    .buttonStyle(.adi(.accent, wide: true))

                    ShareLink(item: "adi-mono mesh join \(token)") {
                        HStack(spacing: 6) {
                            LucideIcon(icon: .share)
                            Text("Share")
                        }
                    }
                    .buttonStyle(.adi(wide: true))
                }

                Text("This invite can be spent once, and expires in ten minutes.")
                    .font(ADI.TextStyle.label)
                    .foregroundStyle(ADI.ink3)
            }
            .padding(20)
        }
    }

    /// A QR of the token.
    ///
    /// Correction level `L`: the token carries a full endpoint address, so it is long, and the
    /// lowest correction level is what keeps the result coarse enough to scan off a phone screen.
    private func qr(_ token: String) -> UIImage? {
        let filter = CIFilter.qrCodeGenerator()
        filter.message = Data(token.utf8)
        filter.correctionLevel = "L"
        guard let output = filter.outputImage else { return nil }
        let scaled = output.transformed(by: CGAffineTransform(scaleX: 8, y: 8))
        let context = CIContext()
        guard let cgImage = context.createCGImage(scaled, from: scaled.extent) else { return nil }
        return UIImage(cgImage: cgImage)
    }
}
