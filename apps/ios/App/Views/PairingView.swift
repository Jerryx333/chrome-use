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

    var body: some View {
        NavigationStack {
            Form {
                if let link {
                    if let conflict {
                        Section {
                            Label("这会替换「\(conflict.name)」，地址/电脑不同", systemImage: "exclamationmark.octagon.fill")
                                .font(.headline)
                                .foregroundStyle(.red)
                            LabeledContent("原来", value: conflict.url.absoluteString)
                            LabeledContent("现在", value: link.url.absoluteString)
                        } footer: {
                            Text("只有你确定这是同一部手机在新电脑或新地址上的配对时才继续。否则取消，并在电脑上重新运行 mobile pair。")
                        }
                    }
                    Section("电脑") {
                        LabeledContent("地址", value: link.url.absoluteString)
                        TextField("名称", text: $name)
                    }
                    Section {
                        Button(role: conflict == nil ? nil : .destructive) {
                            if conflict == nil { Task { await pair(link, replacing: false) } } else { confirmReplace = true }
                        } label: {
                            HStack {
                                Text(pairing ? "正在配对…" : conflict == nil ? "配对" : "替换并配对")
                                if pairing { Spacer(); ProgressView() }
                            }
                        }
                        .disabled(pairing || name.trimmingCharacters(in: .whitespaces).isEmpty)
                        .confirmationDialog("替换「\(conflict?.name ?? "")」？", isPresented: $confirmReplace, titleVisibility: .visible) {
                            Button("替换", role: .destructive) { Task { await pair(link, replacing: true) } }
                        } message: {
                            Text("原来的电脑会从这部手机上删除。")
                        }
                        Button("换一个二维码", role: .cancel) { self.link = nil; conflict = nil; error = nil }
                            .disabled(pairing)
                    } footer: {
                        Text("配对会生成一份只存在这部手机里的密钥；二维码随即失效，再扫一次也无法使用。")
                    }
                } else {
                    Section {
                        QRScannerView { found in accept(found) }
                            .frame(height: 300)
                            .listRowInsets(EdgeInsets())
                    } footer: {
                        Text("在电脑上运行 chrome-use mobile pair --url <手机能访问的地址>，扫描终端里的二维码。")
                    }
                    Section("或粘贴配对链接") {
                        TextField("chromeuse://pair?…", text: $pasted, axis: .vertical)
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                            .font(.footnote.monospaced())
                        HStack {
                            Button("从剪贴板粘贴") {
                                if let text = UIPasteboard.general.string { pasted = text; accept(text) }
                            }
                            Spacer()
                            Button("使用") { accept(pasted) }.disabled(pasted.isEmpty)
                        }
                    }
                }
                if let error {
                    Section { Text(error).foregroundStyle(.red) }
                }
            }
            .navigationTitle(model.repairing == nil ? "配对电脑" : "重新配对")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("取消") { model.repairing = nil; dismiss() }.disabled(pairing)
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
                self.error = String(localized: "电脑拒绝了这个二维码：它可能已被使用、已过期（10 分钟）或已被撤销。请在电脑上重新运行 mobile pair。")
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

    var body: some View {
        if DataScannerViewController.isSupported && DataScannerViewController.isAvailable {
            DataScanner(found: found)
        } else if AVCaptureDevice.default(for: .video) != nil {
            CaptureScanner(found: found)
        } else {
            ContentUnavailableView("没有可用的相机", systemImage: "camera.metering.unknown", description: Text("粘贴配对链接，或在电脑上用 --json 打印链接。"))
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

    func makeCoordinator() -> Coordinator { Coordinator(found: found) }

    final class Coordinator: NSObject, DataScannerViewControllerDelegate {
        let found: (String) -> Void
        init(found: @escaping (String) -> Void) { self.found = found }

        func dataScanner(_ scanner: DataScannerViewController, didAdd items: [RecognizedItem], allItems: [RecognizedItem]) {
            for item in items {
                if case .barcode(let code) = item, let text = code.payloadStringValue, text.hasPrefix("chromeuse:") {
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
            let session = self.session
            DispatchQueue.global(qos: .userInitiated).async { session.startRunning() }
        }

        override func viewDidLayoutSubviews() {
            super.viewDidLayoutSubviews()
            preview?.frame = view.bounds
        }

        override func viewWillDisappear(_ animated: Bool) {
            super.viewWillDisappear(animated)
            session.stopRunning()
        }

        func metadataOutput(_ output: AVCaptureMetadataOutput, didOutput objects: [AVMetadataObject], from connection: AVCaptureConnection) {
            for object in objects {
                if let code = object as? AVMetadataMachineReadableCodeObject, let text = code.stringValue, text.hasPrefix("chromeuse:") {
                    session.stopRunning()
                    found?(text)
                    return
                }
            }
        }
    }
}
