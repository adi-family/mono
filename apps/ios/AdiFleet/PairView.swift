import CoreImage.CIFilterBuiltins
import SwiftUI
import UIKit

/// Pairing, from the viewer's side.
///
/// Pairing is pull-only (`docs/fleet.md` §8): this device mints an invite, and the *node* dials
/// back to spend it. So the whole screen is one job — get this token onto the node — and the token
/// travels by whatever means the person already has open, which is why Copy and Share matter more
/// than the QR does. Nothing here opens a port, on either side.
struct PairView: View {
    let model: FleetModel
    @Environment(\.dismiss) private var dismiss

    @State private var token: String?
    @State private var copied = false

    var body: some View {
        NavigationStack {
            Group {
                if let token {
                    invite(token)
                } else if model.ready {
                    ProgressView("Minting an invite…")
                } else {
                    waiting
                }
            }
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
        // The node dials back, so pairing finishes without anything happening on this screen.
        // Polling is how the sheet notices — a second is well inside "it just worked".
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

    /// Shown until the relay session is up. Minting before then would produce a token that only
    /// works on the local network, which is a worse answer than waiting a moment.
    private var waiting: some View {
        ContentUnavailableView {
            Label("Connecting to the mesh", systemImage: "antenna.radiowaves.left.and.right")
        } description: {
            Text("An invite needs a relay session, so a node has something to dial. This usually takes a few seconds.")
        }
    }

    private func invite(_ token: String) -> some View {
        ScrollView {
            VStack(spacing: 24) {
                if let image = qr(token) {
                    Image(uiImage: image)
                        .interpolation(.none)
                        .resizable()
                        .scaledToFit()
                        .frame(maxWidth: 260)
                        .padding(12)
                        .background(.white)
                        .clipShape(RoundedRectangle(cornerRadius: 12))
                }

                VStack(alignment: .leading, spacing: 8) {
                    Text("Run this on the node")
                        .font(.headline)
                    Text("adi-mono mesh join …")
                        .font(.system(.footnote, design: .monospaced))
                        .foregroundStyle(.secondary)
                    Text("The node dials back over the mesh. It needs no open port, and it will print nothing you have to type here — this device already holds the password.")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
                .frame(maxWidth: .infinity, alignment: .leading)

                Text(token)
                    .font(.system(.caption2, design: .monospaced))
                    .lineLimit(4)
                    .truncationMode(.middle)
                    .padding(10)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(Color(.secondarySystemBackground))
                    .clipShape(RoundedRectangle(cornerRadius: 8))

                HStack(spacing: 12) {
                    Button {
                        UIPasteboard.general.string = "adi-mono mesh join \(token)"
                        copied = true
                    } label: {
                        Label(copied ? "Copied" : "Copy command", systemImage: copied ? "checkmark" : "doc.on.doc")
                            .frame(maxWidth: .infinity)
                    }
                    .buttonStyle(.borderedProminent)

                    ShareLink(item: "adi-mono mesh join \(token)") {
                        Label("Share", systemImage: "square.and.arrow.up")
                            .frame(maxWidth: .infinity)
                    }
                    .buttonStyle(.bordered)
                }

                Text("This invite can be spent once, and expires in ten minutes.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            .padding()
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
