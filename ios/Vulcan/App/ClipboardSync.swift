import UIKit
import UniformTypeIdentifiers

/// Keeps the phone and PC clipboards in step while the app is open.
///
/// - PC → phone: clipboard messages from the PC are written to UIPasteboard unless we already
///   have that content (by hash), so reconnecting doesn't clobber a newer phone/Mac copy.
/// - Phone → PC (auto): when the pasteboard changes and the change wasn't our own write, read it
///   and send it. Reading shows iOS's "Allow Paste" prompt unless the user allowed it in Settings.
/// - Manual: the system Paste button (no prompt) and "Get from PC".
@MainActor
final class ClipboardSync {
    private let connection: ConnectionManager
    private let defaults = UserDefaults.standard
    private var forceNextFromPC = false

    /// changeCount right after we wrote PC content, so we don't send it back.
    private var lastWrittenCount: Int {
        get { defaults.integer(forKey: "clip.lastWrittenCount") }
        set { defaults.set(newValue, forKey: "clip.lastWrittenCount") }
    }
    /// changeCount we last auto-sent (or decided not to), so each change is read at most once.
    private var lastSentCount: Int {
        get { defaults.integer(forKey: "clip.lastSentCount") }
        set { defaults.set(newValue, forKey: "clip.lastSentCount") }
    }
    /// Hash of the last PC content we put on the phone.
    private var lastFromPCHash: String? {
        get { defaults.string(forKey: "clip.lastFromPCHash") }
        set { defaults.set(newValue, forKey: "clip.lastFromPCHash") }
    }

    init(connection: ConnectionManager) {
        self.connection = connection
        NotificationCenter.default.addObserver(
            forName: UIPasteboard.changedNotification, object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated { self?.autoSendIfChanged() }
        }
    }

    // MARK: PC → phone

    func receive(_ msg: ServerMessage) {
        guard let hash = msg.hash else { return }
        if hash == lastFromPCHash, !forceNextFromPC { return }
        let wasManual = forceNextFromPC
        forceNextFromPC = false

        let pasteboard = UIPasteboard.general
        switch msg.kind {
        case "text":
            guard let text = msg.text else { return }
            pasteboard.string = text
        case "image":
            guard let b64 = msg.data, let data = Data(base64Encoded: b64) else { return }
            let type = msg.mime.flatMap { UTType(mimeType: $0) } ?? .png
            pasteboard.setData(data, forPasteboardType: type.identifier)
        default:
            return
        }
        lastFromPCHash = hash
        lastWrittenCount = pasteboard.changeCount
        lastSentCount = pasteboard.changeCount
        if wasManual { connection.showInfo("Copied the PC clipboard") }
    }

    func getFromPC() {
        forceNextFromPC = true
        connection.send(.clipGet)
    }

    // MARK: Phone → PC

    /// Called when the pasteboard changes or the PC's settings arrive (i.e. on every connect).
    func autoSendIfChanged() {
        guard connection.isConnected, connection.clipToPC, !connection.paused else { return }
        let pasteboard = UIPasteboard.general
        let count = pasteboard.changeCount
        guard count != lastWrittenCount, count != lastSentCount else { return }
        lastSentCount = count
        // hasStrings / hasImages don't trigger the paste prompt.
        guard pasteboard.hasStrings || pasteboard.hasImages else { return }
        if let payload = Self.read(pasteboard) {
            connection.send(.clipSet(payload, auto: true))
        }
    }

    private static func read(_ pasteboard: UIPasteboard) -> ClipPayload? {
        if pasteboard.hasImages, let image = pasteboard.image, let png = image.pngData() {
            return .image(png, mime: "image/png")
        }
        if let text = pasteboard.string, !text.isEmpty {
            return .text(text)
        }
        return nil
    }

    /// From the system Paste button: no prompt, since the user tapped it.
    func sendManually(_ providers: [NSItemProvider]) {
        guard let provider = providers.first else { return }
        if provider.canLoadObject(ofClass: UIImage.self) {
            _ = provider.loadObject(ofClass: UIImage.self) { [weak self] object, _ in
                let png = (object as? UIImage)?.pngData()
                Task { @MainActor in
                    guard let self, let png else { return }
                    self.connection.send(.clipSet(.image(png, mime: "image/png"), auto: false))
                    self.markSent()
                }
            }
        } else if provider.canLoadObject(ofClass: String.self) {
            _ = provider.loadObject(ofClass: String.self) { [weak self] text, _ in
                Task { @MainActor in
                    guard let self, let text, !text.isEmpty else { return }
                    self.connection.send(.clipSet(.text(text), auto: false))
                    self.markSent()
                }
            }
        }
    }

    private func markSent() {
        lastSentCount = UIPasteboard.general.changeCount
    }
}
