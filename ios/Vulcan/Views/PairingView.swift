import SwiftUI
import VisionKit

struct PairingView: View {
    @Environment(AppModel.self) private var model
    @State private var scanning = false
    @State private var manualLink = ""

    private var scannerAvailable: Bool {
        DataScannerViewController.isSupported && DataScannerViewController.isAvailable
    }

    var body: some View {
        NavigationStack {
            VStack(spacing: 24) {
                Image(systemName: "iphone.gen3.radiowaves.left.and.right")
                    .font(.system(size: 64))
                    .foregroundStyle(Theme.accent)
                    .padding(.top, 40)

                Text("Pair with your PC")
                    .font(.title.bold())

                Text("On the PC, choose **Pair new device…** from the Vulcan tray icon, or run `vulcand pair` in a terminal. Then scan the code.")
                    .multilineTextAlignment(.center)
                    .foregroundStyle(.secondary)

                Button {
                    scanning = true
                } label: {
                    Label("Scan QR code", systemImage: "qrcode.viewfinder")
                        .frame(maxWidth: .infinity)
                }
                .buttonStyle(.borderedProminent)
                .controlSize(.large)
                .disabled(!scannerAvailable)

                if !scannerAvailable {
                    Text("Camera scanning isn't available. Scan the code with the Camera app instead, or paste the link below.")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                        .multilineTextAlignment(.center)
                }

                VStack(alignment: .leading, spacing: 8) {
                    Text("Or paste the vulcan:// link").font(.footnote).foregroundStyle(.secondary)
                    HStack {
                        TextField("vulcan://pair?…", text: $manualLink)
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                            .textFieldStyle(.roundedBorder)
                        Button("Pair") {
                            if let url = URL(string: manualLink.trimmingCharacters(in: .whitespacesAndNewlines)) {
                                model.pair(with: url)
                            }
                        }
                        .disabled(manualLink.isEmpty)
                    }
                }

                if let error = model.pairingError {
                    Text(error).foregroundStyle(.red).font(.footnote)
                }
                Spacer()
            }
            .padding(24)
            .sheet(isPresented: $scanning) {
                ScannerView { payload in
                    if let url = URL(string: payload), url.scheme == "vulcan" {
                        scanning = false
                        model.pair(with: url)
                    }
                }
                .ignoresSafeArea()
            }
        }
    }
}

/// VisionKit live QR scanner; calls `onCode` with each QR payload it recognises.
struct ScannerView: UIViewControllerRepresentable {
    let onCode: (String) -> Void

    func makeUIViewController(context: Context) -> DataScannerViewController {
        let vc = DataScannerViewController(
            recognizedDataTypes: [.barcode(symbologies: [.qr])],
            qualityLevel: .balanced,
            isHighlightingEnabled: true
        )
        vc.delegate = context.coordinator
        return vc
    }

    func updateUIViewController(_ vc: DataScannerViewController, context: Context) {
        context.coordinator.onCode = onCode
        if !vc.isScanning { try? vc.startScanning() }
    }

    static func dismantleUIViewController(_ vc: DataScannerViewController, coordinator: Coordinator) {
        vc.stopScanning()
    }

    func makeCoordinator() -> Coordinator { Coordinator(onCode: onCode) }

    final class Coordinator: NSObject, DataScannerViewControllerDelegate {
        var onCode: (String) -> Void

        init(onCode: @escaping (String) -> Void) { self.onCode = onCode }

        func dataScanner(_ scanner: DataScannerViewController, didAdd items: [RecognizedItem], allItems: [RecognizedItem]) {
            for item in items {
                if case let .barcode(code) = item, let payload = code.payloadStringValue {
                    onCode(payload)
                    return
                }
            }
        }
    }
}
