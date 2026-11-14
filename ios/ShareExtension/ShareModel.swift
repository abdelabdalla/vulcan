import Foundation
import Observation
import UniformTypeIdentifiers

/// Sends everything shared to the extension (photos, videos, files) to the PC, one at a time.
@MainActor
@Observable
final class ShareModel {
    enum Phase: Equatable {
        case preparing
        case sending
        case done
        case problem(String)
    }

    enum ItemState: Equatable {
        case waiting
        case sending
        case sent
        case failed(String)
    }

    struct Item: Identifiable {
        let id = UUID()
        var name: String
        /// Our own copy of the shared file; nil when it couldn't be read.
        var file: URL?
        var progress = 0.0
        var state = ItemState.waiting
    }

    private(set) var phase = Phase.preparing
    private(set) var items: [Item] = []
    private(set) var hostname = ""

    @ObservationIgnored var onFinish: (() -> Void)?
    @ObservationIgnored var onCancel: (() -> Void)?
    @ObservationIgnored private var uploader: Uploader?
    @ObservationIgnored private var cancelled = false

    var sentCount: Int { items.filter { $0.state == .sent }.count }
    var failedCount: Int {
        items.filter { if case .failed = $0.state { true } else { false } }.count
    }

    func run(_ providers: [NSItemProvider]) async {
        guard var pairing = PairingStore.load() else {
            phase = .problem("Open Vulcan and pair it with your PC first.")
            return
        }
        hostname = pairing.hostname

        for provider in providers {
            if cancelled { return }
            if let item = await Self.load(provider) {
                items.append(item)
            } else {
                items.append(Item(name: provider.suggestedName ?? "Item", state: .failed("Couldn't read this item")))
            }
        }
        guard items.contains(where: { $0.file != nil }) else {
            phase = .problem("Nothing here can be sent to the PC.")
            return
        }

        // The last known address first, then look for the PC on the network in case its IP changed.
        var uploader = Uploader(pairing: pairing)
        if await !uploader.reachable() {
            guard let host = await Discovery.find(fingerprint: pairing.fingerprint) else {
                phase = .problem("Can't reach \(pairing.hostname). Sending files needs the phone and PC on the same Wi-Fi.")
                return
            }
            pairing.host = host
            PairingStore.save(pairing)
            uploader = Uploader(pairing: pairing)
        }
        self.uploader = uploader
        await sendAll()
    }

    func retryFailed() {
        for i in items.indices where items[i].file != nil {
            if case .failed = items[i].state { items[i].state = .waiting; items[i].progress = 0 }
        }
        Task { await sendAll() }
    }

    func cancel() {
        cancelled = true
        uploader?.cancel()
        cleanUp()
        onCancel?()
    }

    func finish() {
        cleanUp()
        onFinish?()
    }

    private func sendAll() async {
        guard let uploader else { return }
        phase = .sending
        for i in items.indices {
            guard !cancelled, items[i].state == .waiting, let file = items[i].file else { continue }
            items[i].state = .sending
            do {
                try await uploader.upload(file, name: items[i].name) { [weak self] fraction in
                    Task { @MainActor in self?.items[i].progress = fraction }
                }
                items[i].state = .sent
                items[i].progress = 1
                try? FileManager.default.removeItem(at: file.deletingLastPathComponent())
                items[i].file = nil
            } catch {
                items[i].state = .failed(error.localizedDescription)
            }
        }
        guard !cancelled else { return }
        phase = .done
        if failedCount == 0 {
            try? await Task.sleep(for: .seconds(1.2))
            finish()
        }
    }

    private func cleanUp() {
        for item in items {
            if let file = item.file { try? FileManager.default.removeItem(at: file.deletingLastPathComponent()) }
        }
    }

    // MARK: Reading shared items

    /// Copy the shared item into our temp folder (the provider's copy is deleted when its callback returns).
    nonisolated private static func load(_ provider: NSItemProvider) async -> Item? {
        guard let type = bestType(provider) else { return nil }
        return await withCheckedContinuation { continuation in
            _ = provider.loadFileRepresentation(forTypeIdentifier: type) { url, _ in
                guard let url else { return continuation.resume(returning: nil) }
                let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
                let dest = dir.appendingPathComponent(url.lastPathComponent)
                do {
                    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
                    try FileManager.default.copyItem(at: url, to: dest)
                    continuation.resume(returning: Item(name: name(for: provider, file: url), file: dest))
                } catch {
                    continuation.resume(returning: nil)
                }
            }
        }
    }

    /// The original photo or video if there is one, else any file data. Providers list their types
    /// best first, so a HEIC photo is sent as HEIC rather than a converted JPEG.
    nonisolated private static func bestType(_ provider: NSItemProvider) -> String? {
        let types = provider.registeredTypeIdentifiers.compactMap { UTType($0) }
        let pick = types.first { $0.conforms(to: .image) }
            ?? types.first { $0.conforms(to: .movie) }
            ?? types.first { $0.conforms(to: .data) && !$0.conforms(to: .url) }
        return pick?.identifier
    }

    /// Photos suggests "IMG_1234" without an extension; take that from the file.
    nonisolated static func name(for provider: NSItemProvider, file: URL) -> String {
        guard let suggested = provider.suggestedName, !suggested.isEmpty else { return file.lastPathComponent }
        if (suggested as NSString).pathExtension.isEmpty, !file.pathExtension.isEmpty {
            return "\(suggested).\(file.pathExtension)"
        }
        return suggested
    }
}
