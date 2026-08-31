import AVFoundation
import SwiftUI

/// The camera, and reading an invite out of what it sees.
///
/// The other end of the QR the CLI draws (`adi-cli/src/qr.rs`) and the panel's Fleet page serves.
/// A pairing token is over nine hundred characters: the QR is how it gets onto a phone, and typing
/// it is not a thing anybody will do. This is the same decision the web client already made and
/// wrote down (`adi-mesh-client/src/ui.rs`) — **scanning is the primary action, and pasting is the
/// fallback for when the camera is refused, absent, or pointed at nothing.**
///
/// Where the web client has to decode frames itself in Rust — because `BarcodeDetector` is
/// Chromium-only and iOS Safari is the whole reason that client exists — a native app gets the
/// decoder from the system. `AVCaptureMetadataOutput` hands back the string.
///
/// The session is torn down on every exit path. A camera light left on is alarming, and it is a
/// battery drain on exactly the device this is for.
struct ScanView: UIViewRepresentable {
    /// Called on the main actor with the first `adi-invite:` payload seen. Called once: the view
    /// stops the session before delivering, so a code held in frame cannot fire a second pairing.
    let onFound: (String) -> Void

    func makeCoordinator() -> Coordinator { Coordinator(onFound: onFound) }

    func makeUIView(context: Context) -> PreviewView {
        let view = PreviewView()
        context.coordinator.attach(to: view)
        return view
    }

    func updateUIView(_ uiView: PreviewView, context: Context) {}

    /// SwiftUI tears the representable down when the sheet closes; this is the hook that stops the
    /// camera when it does. Without it the session outlives the screen that opened it.
    static func dismantleUIView(_ uiView: PreviewView, coordinator: Coordinator) {
        coordinator.stop()
    }

    /// A view whose backing layer *is* the preview layer, so the frame follows the view for free.
    /// Doing it the other way — adding a sublayer and resizing it in `layoutSubviews` — is what
    /// produces a preview that is the wrong size for one frame on rotation.
    final class PreviewView: UIView {
        override class var layerClass: AnyClass { AVCaptureVideoPreviewLayer.self }
        var previewLayer: AVCaptureVideoPreviewLayer { layer as! AVCaptureVideoPreviewLayer }
    }

    final class Coordinator: NSObject, AVCaptureMetadataOutputObjectsDelegate {
        private let session = AVCaptureSession()
        private let onFound: (String) -> Void
        private var delivered = false

        init(onFound: @escaping (String) -> Void) {
            self.onFound = onFound
        }

        func attach(to view: PreviewView) {
            view.previewLayer.session = session
            view.previewLayer.videoGravity = .resizeAspectFill

            guard
                let device = AVCaptureDevice.default(.builtInWideAngleCamera, for: .video, position: .back),
                let input = try? AVCaptureDeviceInput(device: device),
                session.canAddInput(input)
            else { return }
            session.addInput(input)

            let output = AVCaptureMetadataOutput()
            guard session.canAddOutput(output) else { return }
            session.addOutput(output)
            output.setMetadataObjectsDelegate(self, queue: .main)
            // Set *after* adding to the session: the available types are empty until then, so
            // assigning `.qr` first throws "Unsupported type" and the scanner silently sees nothing.
            output.metadataObjectTypes = output.availableMetadataObjectTypes.contains(.qr) ? [.qr] : []

            start()
        }

        func start() {
            guard !session.isRunning else { return }
            // Off the main thread: `startRunning` blocks for a beat while the camera warms up, and
            // on the main actor that is a visible hitch as the sheet opens.
            Task.detached(priority: .userInitiated) { [session] in session.startRunning() }
        }

        func stop() {
            guard session.isRunning else { return }
            Task.detached(priority: .userInitiated) { [session] in session.stopRunning() }
        }

        func metadataOutput(
            _ output: AVCaptureMetadataOutput,
            didOutput objects: [AVMetadataObject],
            from connection: AVCaptureConnection
        ) {
            guard !delivered else { return }
            let found = objects
                .compactMap { $0 as? AVMetadataMachineReadableCodeObject }
                .compactMap(\.stringValue)
                .first { $0.contains("adi-invite:") }
            guard let found else { return }
            delivered = true
            stop()
            onFound(found)
        }
    }
}

/// Whether the camera can be used at all, which is three states and not two.
///
/// "Refused" and "this device has no camera" read the same to the code and completely differently
/// to the person holding it, and the simulator is the second one — which is where every screenshot
/// of this screen is taken.
enum CameraAccess {
    case ready
    case denied
    case unavailable

    static func current() -> CameraAccess {
        guard AVCaptureDevice.default(.builtInWideAngleCamera, for: .video, position: .back) != nil
        else { return .unavailable }
        switch AVCaptureDevice.authorizationStatus(for: .video) {
        case .authorized: return .ready
        case .denied, .restricted: return .denied
        default: return .ready          // not yet asked — asking is what `request` is for
        }
    }

    /// Ask, if it has not been asked. Answers what the state is afterwards.
    static func request() async -> CameraAccess {
        guard AVCaptureDevice.default(.builtInWideAngleCamera, for: .video, position: .back) != nil
        else { return .unavailable }
        if AVCaptureDevice.authorizationStatus(for: .video) == .notDetermined {
            _ = await AVCaptureDevice.requestAccess(for: .video)
        }
        return current()
    }
}
