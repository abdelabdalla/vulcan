import Foundation
import Observation
import Photos
import UniformTypeIdentifiers

/// One upload or download shown in the Files tab.
struct Transfer: Identifiable, Equatable {
    enum State: Equatable {
        case waiting
        case running
        case done(String)
        case failed(String)
    }

    let id = UUID()
    let name: String
    let incoming: Bool
    var progress: Double = 0
    var state: State = .waiting
}

/// A folder listing from the PC's home directory.
struct PCListing: Decodable {
    struct Entry: Decodable, Identifiable, Hashable {
        let name: String
        let dir: Bool
        let size: Int64
        let modified: Int64
        var id: String { name }
    }

    let path: String
    let entries: [Entry]
    let truncated: Bool
}

struct TransferError: LocalizedError {
    let message: String
    var errorDescription: String? { message }
}

/// HTTPS transfers with the PC, on the same pinned certificate and pairing token as the WebSocket.
/// Uploads go to ~/Downloads/Vulcan on the PC. Downloads land in Photos (images and videos)
/// or the app's Received folder, which shows up in the Files app.
@Observable
@MainActor
final class TransferManager {
    private(set) var transfers: [Transfer] = []

    private let connection: ConnectionManager
    @ObservationIgnored private var session: URLSession?
    @ObservationIgnored private var sessionFingerprint: String?
    @ObservationIgnored private var observations: [UUID: NSKeyValueObservation] = [:]
    /// Uploads picked while disconnected (e.g. "Open in Vulcan" right at launch).
    @ObservationIgnored private var pendingUploads: [(URL, String)] = []
    /// Offers being downloaded, so a reconnect's re-sent offer doesn't start a second copy.
    @ObservationIgnored private var activeOffers: Set<String> = []

    init(connection: ConnectionManager) {
        self.connection = connection
    }

    var hasFinished: Bool {
        transfers.contains {
            switch $0.state {
            case .done, .failed: true
            default: false
            }
        }
    }

    func clearFinished() {
        transfers.removeAll {
            switch $0.state {
            case .done, .failed: true
            default: false
            }
        }
    }

    // MARK: Requests

    private func request(path: String, query: [URLQueryItem] = [], method: String = "GET") -> URLRequest? {
        guard connection.isConnected, let p = connection.pairing, let host = connection.activeHost else { return nil }
        var comps = URLComponents()
        comps.scheme = "https"
        comps.host = host
        comps.port = p.port
        comps.path = path
        if !query.isEmpty { comps.queryItems = query }
        guard let url = comps.url else { return nil }
        var req = URLRequest(url: url)
        req.httpMethod = method
        req.setValue("Bearer \(p.token)", forHTTPHeaderField: "Authorization")
        req.timeoutInterval = 30
        return req
    }

    private func urlSession() -> URLSession? {
        guard let fp = connection.pairing?.fingerprint else { return nil }
        if let session, sessionFingerprint == fp { return session }
        session?.invalidateAndCancel()
        let s = URLSession(configuration: .default, delegate: PinningDelegate(fingerprint: fp), delegateQueue: nil)
        session = s
        sessionFingerprint = fp
        return s
    }

    nonisolated private static func check(_ response: URLResponse?, _ data: Data?) throws {
        guard let http = response as? HTTPURLResponse else { throw TransferError(message: "No response from the PC") }
        guard (200..<300).contains(http.statusCode) else {
            let message = data
                .flatMap { try? JSONDecoder().decode([String: String].self, from: $0)["error"] }
                ?? "The PC returned \(http.statusCode)"
            throw TransferError(message: message)
        }
    }

    // MARK: Browsing

    func list(path: String, hidden: Bool) async throws -> PCListing {
        guard let req = request(path: "/fs/list", query: [.init(name: "path", value: path), .init(name: "hidden", value: String(hidden))]),
              let session = urlSession()
        else { throw TransferError(message: "Not connected to the PC") }
        let (data, response) = try await session.data(for: req)
        try Self.check(response, data)
        return try JSONDecoder().decode(PCListing.self, from: data)
    }

    func downloadFromPC(path: String, name: String, hidden: Bool) {
        guard let req = request(path: "/fs/download", query: [.init(name: "path", value: path), .init(name: "hidden", value: String(hidden))]) else {
            transfers.append(Transfer(name: name, incoming: true, state: .failed("Not connected")))
            return
        }
        download(req, name: name, onFinish: nil)
    }

    // MARK: PC → phone offers

    func handleOffer(id: String, name: String) {
        guard !activeOffers.contains(id), let req = request(path: "/files/\(id)") else { return }
        activeOffers.insert(id)
        download(req, name: name) { [weak self] ok in
            self?.activeOffers.remove(id)
            // Done either way, so the PC stops offering it; a failure is shown in the list.
            if ok { self?.connection.send(.fileDone(id)) }
        }
    }

    private func download(_ req: URLRequest, name: String, onFinish: ((Bool) -> Void)?) {
        guard let session = urlSession() else { return }
        var transfer = Transfer(name: name, incoming: true)
        transfer.state = .running
        transfers.append(transfer)
        let id = transfer.id

        let task = session.downloadTask(with: req) { location, response, error in
            // The temp file is removed when this handler returns, so move it now.
            var moved: URL?
            var failure: String?
            if let error {
                failure = error.localizedDescription
            } else if let http = response as? HTTPURLResponse, !(200..<300).contains(http.statusCode) {
                failure = "The PC returned \(http.statusCode)"
            } else if let location {
                let dest = FileManager.default.temporaryDirectory
                    .appendingPathComponent(UUID().uuidString)
                    .appendingPathExtension((name as NSString).pathExtension)
                do {
                    try FileManager.default.moveItem(at: location, to: dest)
                    moved = dest
                } catch {
                    failure = error.localizedDescription
                }
            }
            Task { @MainActor [weak self] in
                guard let self else { return }
                self.observations[id] = nil
                if let moved {
                    let destination = await Self.deliver(moved, name: name)
                    self.update(id) { $0.state = .done(destination); $0.progress = 1 }
                    onFinish?(true)
                } else {
                    self.update(id) { $0.state = .failed(failure ?? "Download failed") }
                    onFinish?(false)
                }
            }
        }
        observe(task, id: id)
        task.resume()
    }

    /// Photos for images and videos, otherwise the app's Received folder (visible in the Files app).
    nonisolated private static func deliver(_ file: URL, name: String) async -> String {
        let type = UTType(filenameExtension: (name as NSString).pathExtension)
        let isImage = type?.conforms(to: .image) ?? false
        let isVideo = type?.conforms(to: .movie) ?? false
        if isImage || isVideo, await saveToPhotos(file, name: name, video: isVideo) {
            return "Saved to Photos"
        }
        do {
            let dir = try FileManager.default.url(for: .documentDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
                .appendingPathComponent("Received", isDirectory: true)
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            let dest = uniqueURL(in: dir, name: name)
            try FileManager.default.moveItem(at: file, to: dest)
            return "Saved to Files › Vulcan › Received"
        } catch {
            return "Couldn't save: \(error.localizedDescription)"
        }
    }

    nonisolated private static func saveToPhotos(_ file: URL, name: String, video: Bool) async -> Bool {
        let status = await PHPhotoLibrary.requestAuthorization(for: .addOnly)
        guard status == .authorized || status == .limited else { return false }
        do {
            try await PHPhotoLibrary.shared().performChanges {
                let options = PHAssetResourceCreationOptions()
                options.originalFilename = name
                PHAssetCreationRequest.forAsset().addResource(with: video ? .video : .photo, fileURL: file, options: options)
            }
            try? FileManager.default.removeItem(at: file)
            return true
        } catch {
            return false
        }
    }

    nonisolated private static func uniqueURL(in dir: URL, name: String) -> URL {
        let candidate = dir.appendingPathComponent(name)
        guard FileManager.default.fileExists(atPath: candidate.path) else { return candidate }
        let ext = (name as NSString).pathExtension
        let stem = (name as NSString).deletingPathExtension
        for i in 1... {
            let n = ext.isEmpty ? "\(stem) (\(i))" : "\(stem) (\(i)).\(ext)"
            let url = dir.appendingPathComponent(n)
            if !FileManager.default.fileExists(atPath: url.path) { return url }
        }
        return candidate
    }

    // MARK: Phone → PC

    /// Copy a picked or shared file (while we have access to it), then upload it.
    func upload(copying url: URL, name: String? = nil) {
        let scoped = url.startAccessingSecurityScopedResource()
        defer { if scoped { url.stopAccessingSecurityScopedResource() } }
        let name = name ?? url.lastPathComponent
        let temp = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
            .appendingPathExtension(url.pathExtension)
        do {
            try FileManager.default.copyItem(at: url, to: temp)
        } catch {
            transfers.append(Transfer(name: name, incoming: false, state: .failed(error.localizedDescription)))
            return
        }
        // Files shared into the app are copied to Documents/Inbox; don't keep them.
        if url.path.contains("/Documents/Inbox/") { try? FileManager.default.removeItem(at: url) }
        upload(temp, name: name)
    }

    /// Upload a temporary file we own; it's deleted afterwards.
    func upload(_ file: URL, name: String) {
        guard connection.isConnected else {
            pendingUploads.append((file, name))
            transfers.append(Transfer(name: name, incoming: false))
            return
        }
        guard let req = request(path: "/files/upload", query: [.init(name: "name", value: name)], method: "POST"),
              let session = urlSession()
        else { return }
        let id: UUID
        if let i = transfers.firstIndex(where: { $0.name == name && !$0.incoming && $0.state == .waiting }) {
            transfers[i].state = .running
            id = transfers[i].id
        } else {
            var t = Transfer(name: name, incoming: false)
            t.state = .running
            transfers.append(t)
            id = t.id
        }
        let task = session.uploadTask(with: req, fromFile: file) { data, response, error in
            try? FileManager.default.removeItem(at: file)
            let result: Result<String, Error>
            if let error {
                result = .failure(error)
            } else {
                do {
                    try Self.check(response, data)
                    let saved = data.flatMap { try? JSONDecoder().decode([String: String].self, from: $0) }?["name"]
                    result = .success("Sent to Downloads/Vulcan\(saved.map { "/\($0)" } ?? "")")
                } catch {
                    result = .failure(error)
                }
            }
            Task { @MainActor [weak self] in
                self?.observations[id] = nil
                self?.update(id) {
                    switch result {
                    case let .success(text): $0.state = .done(text); $0.progress = 1
                    case let .failure(error): $0.state = .failed(error.localizedDescription)
                    }
                }
            }
        }
        observe(task, id: id)
        task.resume()
    }

    /// Called when the connection comes up.
    func flushPending() {
        let pending = pendingUploads
        pendingUploads = []
        for (file, name) in pending { upload(file, name: name) }
    }

    // MARK: Progress

    private func observe(_ task: URLSessionTask, id: UUID) {
        observations[id] = task.progress.observe(\.fractionCompleted) { progress, _ in
            let fraction = progress.fractionCompleted
            Task { @MainActor [weak self] in
                self?.update(id) { if case .running = $0.state { $0.progress = fraction } }
            }
        }
    }

    private func update(_ id: UUID, _ change: (inout Transfer) -> Void) {
        guard let i = transfers.firstIndex(where: { $0.id == id }) else { return }
        change(&transfers[i])
    }
}
