import AVFoundation
import ChromeUseKit
import SwiftUI
import UIKit
import VisionKit

struct PairingView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss
    @State private var link: PairLink?
    @State private var pasted = ""
    @State private var name = ""
    @State private var pairing = false
    @State private var error: String?
    /// A saved computer this link would replace (same device id, another
    /// address or gateway).
    @State private var conflict: GatewayCredential?
    @State private var confirmReplace = false
    private var conflictName: String { conflict?.name ?? "" }

    var body: some View {
        NavigationStack {
            Form {
                if let link {
                    if let conflict {
                        Section {
                            Label("This replaces “\(conflict.name)”: a different address or computer", systemImage: "exclamationmark.octagon.fill")
                                .font(.headline)
                                .foregroundStyle(.red)
                            LabeledContent("Before", value: conflict.url.absoluteString)
                            LabeledContent("Now", value: link.url.absoluteString)
                        } footer: {
                            Text("Continue only if you are sure this phone is being paired with a new computer or address. Otherwise cancel and run mobile pair again on the computer.")
                        }
                    }
                    Section("Computers") {
                        LabeledContent("Address", value: link.url.absoluteString)
                        TextField("Name", text: $name)
                    }
                    Section {
                        Button(role: conflict == nil ? nil : .destructive) {
                            if conflict == nil { Task { await pair(link, replacing: false) } } else { confirmReplace = true }
                        } label: {
                            HStack {
                                Text(pairing ? (model.waitingForLocalNetwork ? LocalizedStringKey("Waiting for Local Network permission…") : LocalizedStringKey("Pairing…")) : conflict == nil ? LocalizedStringKey("Pair") : LocalizedStringKey("Replace and pair"))
                                if pairing { Spacer(); ProgressView() }
                            }
                        }
                        .disabled(pairing || name.trimmingCharacters(in: .whitespaces).isEmpty)
                        .accessibilityIdentifier("pairButton")
                        .confirmationDialog("Replace “\(conflictName)”?", isPresented: $confirmReplace, titleVisibility: .visible) {
                            Button("Replace", role: .destructive) { Task { await pair(link, replacing: true) } }
                        } message: {
                            Text("The previous computer will be removed from this phone.")
                        }
                        Button("Use another QR code", role: .cancel) { self.link = nil; conflict = nil; error = nil }
                            .disabled(pairing)
                    } footer: {
                        Text("Pairing creates a key that exists only on this phone; the QR code stops working right away and cannot be scanned again.")
                    }
                } else {
                    Section {
                        QRScannerView { found in accept(found) }
                            .frame(height: 300)
                            .listRowInsets(EdgeInsets())
                    } footer: {
                        Text("On the computer, run chrome-use mobile pair --url <address your phone can reach> and scan the QR code in the terminal.")
                    }
                    Section("Or paste a pairing link") {
                        // Single line: Return submits, so the keyboard never
                        // hides the Use button on a small phone.
                        TextField("chromeuse://pair?…", text: $pasted)
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                            .keyboardType(.URL)
                            .submitLabel(.go)
                            .onSubmit { if !pasted.isEmpty { accept(pasted) } }
                            .font(.footnote.monospaced())
                        HStack {
                            Button("Paste from clipboard") {
                                if let text = UIPasteboard.general.string { pasted = text; accept(text) }
                            }
                            Spacer()
                            Button("Use") { accept(pasted) }.disabled(pasted.isEmpty)
                        }
                    }
                }
                if let error {
                    Section { Text(error).foregroundStyle(.red) }
                }
            }
            .scrollDismissesKeyboard(.interactively)
            .navigationTitle(model.repairing == nil ? LocalizedStringKey("Pair a computer") : LocalizedStringKey("Pair again"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { model.repairing = nil; dismiss() }.disabled(pairing)
                }
            }
            .onAppear {
                if let pending = model.pendingLink { use(pending) }
            }
        }
        .interactiveDismissDisabled(pairing)
    }

    private func accept(_ text: String) {
        do {
            use(try PairLink(parsing: text))
        } catch {
            self.error = describe(error)
        }
    }

    private func use(_ found: PairLink) {
        link = found
        error = nil
        // Ask for Local Network permission now, not in the middle of pairing.
        LocalNetwork.touch(found.url)
        conflict = model.conflict(for: found)
        if conflict != nil {
            // Never presented as an ordinary re-pair of the saved computer.
            name = found.suggestedName
        } else if let existing = model.client(found.deviceId) {
            name = existing.credential.name
        } else if let replacing = model.repairing.flatMap(model.client) {
            name = replacing.credential.name
        } else {
            name = found.suggestedName
        }
    }

    private func pair(_ link: PairLink, replacing: Bool) async {
        pairing = true
        defer { pairing = false }
        do {
            let client = try await model.pair(link, name: name.trimmingCharacters(in: .whitespaces), replacing: replacing)
            model.pendingLink = nil
            dismiss()
            model.path = [.computer(client.id)]
        } catch {
            if let gateway = error as? GatewayError, gateway == .closed(.unauthorized) {
                self.error = String(localized: "The computer refused this QR code: it may have been used, expired (10 minutes) or revoked. Run mobile pair again on the computer.")
            } else {
                self.error = describe(error)
            }
        }
    }
}

/// VisionKit's scanner where the device supports it, AVFoundation otherwise,
/// and a note where there is no camera (the simulator).
struct QRScannerView: View {
    let found: (String) -> Void
    @Environment(\.scenePhase) private var phase
    @State private var authorization = AVCaptureDevice.authorizationStatus(for: .video)

    var body: some View {
        if authorization == .denied || authorization == .restricted {
            ContentUnavailableView {
                Label("Camera access is off", systemImage: "camera")
            } description: {
                Text("Paste a pairing link below, or allow camera access in Settings to scan a QR code.")
            } actions: {
                if authorization == .denied {
                    Button("Open Settings") {
                        if let url = URL(string: UIApplication.openSettingsURLString) { UIApplication.shared.open(url) }
                    }
                }
            }
            .onChange(of: phase) { _, phase in
                if phase == .active { authorization = AVCaptureDevice.authorizationStatus(for: .video) }
            }
        } else if authorization == .notDetermined && AVCaptureDevice.default(for: .video) != nil {
            ProgressView()
                .task {
                    _ = await AVCaptureDevice.requestAccess(for: .video)
                    authorization = AVCaptureDevice.authorizationStatus(for: .video)
                }
        } else if DataScannerViewController.isSupported && DataScannerViewController.isAvailable {
            DataScanner(found: found)
        } else if AVCaptureDevice.default(for: .video) != nil {
            CaptureScanner(found: found)
        } else {
            ContentUnavailableView("No camera available", systemImage: "camera.metering.unknown", description: Text("Paste the pairing link, or print it on the computer with --json."))
        }
    }
}

private struct DataScanner: UIViewControllerRepresentable {
    let found: (String) -> Void

    func makeUIViewController(context: Context) -> DataScannerViewController {
        let scanner = DataScannerViewController(
            recognizedDataTypes: [.barcode(symbologies: [.qr])],
            qualityLevel: .balanced,
            isHighlightingEnabled: true
        )
        scanner.delegate = context.coordinator
        try? scanner.startScanning()
        return scanner
    }

    func updateUIViewController(_ controller: DataScannerViewController, context: Context) {}

    static func dismantleUIViewController(_ controller: DataScannerViewController, coordinator: Coordinator) {
        controller.stopScanning()
    }

    func makeCoordinator() -> Coordinator { Coordinator(found: found) }

    final class Coordinator: NSObject, DataScannerViewControllerDelegate {
        let found: (String) -> Void
        private var delivered = false
        init(found: @escaping (String) -> Void) { self.found = found }

        func dataScanner(_ scanner: DataScannerViewController, didAdd items: [RecognizedItem], allItems: [RecognizedItem]) {
            for item in items {
                if !delivered, case .barcode(let code) = item, let text = code.payloadStringValue, text.hasPrefix("chromeuse:") {
                    delivered = true
                    scanner.stopScanning()
                    found(text)
                    return
                }
            }
        }
    }
}

private struct CaptureScanner: UIViewControllerRepresentable {
    let found: (String) -> Void

    func makeUIViewController(context: Context) -> CaptureController {
        let controller = CaptureController()
        controller.found = found
        return controller
    }

    func updateUIViewController(_ controller: CaptureController, context: Context) {}

    final class CaptureController: UIViewController, AVCaptureMetadataOutputObjectsDelegate {
        var found: ((String) -> Void)?
        private let session = AVCaptureSession()
        private let sessionQueue = DispatchQueue(label: "com.leeguoo.chromeuse.camera")
        private var preview: AVCaptureVideoPreviewLayer?

        override func viewDidLoad() {
            super.viewDidLoad()
            guard let device = AVCaptureDevice.default(for: .video),
                  let input = try? AVCaptureDeviceInput(device: device),
                  session.canAddInput(input)
            else { return }
            session.addInput(input)
            let output = AVCaptureMetadataOutput()
            guard session.canAddOutput(output) else { return }
            session.addOutput(output)
            output.setMetadataObjectsDelegate(self, queue: .main)
            output.metadataObjectTypes = [.qr]
            let layer = AVCaptureVideoPreviewLayer(session: session)
            layer.videoGravity = .resizeAspectFill
            view.layer.addSublayer(layer)
            preview = layer
        }

        override func viewWillAppear(_ animated: Bool) {
            super.viewWillAppear(animated)
            let session = self.session
            sessionQueue.async { if !session.isRunning { session.startRunning() } }
        }

        override func viewDidLayoutSubviews() {
            super.viewDidLayoutSubviews()
            preview?.frame = view.bounds
        }

        override func viewWillDisappear(_ animated: Bool) {
            super.viewWillDisappear(animated)
            stopScanning()
        }

        private func stopScanning() {
            let session = self.session
            sessionQueue.async { if session.isRunning { session.stopRunning() } }
        }

        func metadataOutput(_ output: AVCaptureMetadataOutput, didOutput objects: [AVMetadataObject], from connection: AVCaptureConnection) {
            for object in objects {
                if let code = object as? AVMetadataMachineReadableCodeObject, let text = code.stringValue, text.hasPrefix("chromeuse:") {
                    stopScanning()
                    let deliver = found
                    found = nil
                    deliver?(text)
                    return
                }
            }
        }
    }
}
