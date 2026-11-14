import CoreTransferable
import PhotosUI
import SwiftUI
import UniformTypeIdentifiers

/// Send files to the PC, follow transfers, and browse the PC's home folder.
struct FilesView: View {
    @Environment(AppModel.self) private var model
    @State private var photoItems: [PhotosPickerItem] = []
    @State private var importing = false

    private var transfers: TransferManager { model.transfers }

    var body: some View {
        NavigationStack {
            List {
                Section {
                    PhotosPicker(selection: $photoItems, maxSelectionCount: 50, matching: .any(of: [.images, .videos])) {
                        Label("Photos and videos", systemImage: "photo.on.rectangle")
                    }
                    Button {
                        importing = true
                    } label: {
                        Label("Files", systemImage: "folder")
                    }
                } header: {
                    Text("Send to PC")
                } footer: {
                    Text("Files go to Downloads/Vulcan on the PC. You can also use Share → Vulcan from any app.")
                }

                if !transfers.transfers.isEmpty {
                    Section {
                        ForEach(transfers.transfers.reversed()) { TransferRow(transfer: $0) }
                        if transfers.hasFinished {
                            Button("Clear finished") { transfers.clearFinished() }
                        }
                    } header: {
                        Text("Transfers")
                    }
                }

                Section {
                    NavigationLink {
                        BrowserView(path: "", title: "Home")
                    } label: {
                        Label("Browse home folder", systemImage: "house")
                    }
                } header: {
                    Text("On the PC")
                } footer: {
                    Text("Tap a file to download it: photos and videos go to Photos, everything else to Files › On My iPhone › Vulcan › Received. Files sent from the PC (Dolphin's Send to iPhone, the tray, or `vulcand send`) download automatically while Vulcan is open.")
                }
            }
            .navigationTitle("Files")
            .navigationBarTitleDisplayMode(.inline)
            .disabled(!model.connection.isConnected)
            .fileImporter(isPresented: $importing, allowedContentTypes: [.item], allowsMultipleSelection: true) { result in
                if case let .success(urls) = result {
                    for url in urls { transfers.upload(copying: url) }
                }
            }
            .onChange(of: photoItems) { _, items in
                guard !items.isEmpty else { return }
                photoItems = []
                Task { await sendPhotos(items) }
            }
        }
    }

    private func sendPhotos(_ items: [PhotosPickerItem]) async {
        let formatter = DateFormatter()
        formatter.dateFormat = "yyyy-MM-dd HH.mm.ss"
        let stamp = formatter.string(from: Date())
        for (i, item) in items.enumerated() {
            let type = item.supportedContentTypes.first
            let suffix = items.count > 1 ? " \(i + 1)" : ""
            if type?.conforms(to: .movie) == true, let movie = try? await item.loadTransferable(type: PickedMovie.self) {
                transfers.upload(movie.url, name: "Video \(stamp)\(suffix).\(movie.url.pathExtension)")
            } else if let data = try? await item.loadTransferable(type: Data.self) {
                let ext = type?.preferredFilenameExtension ?? "jpg"
                let temp = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString).appendingPathExtension(ext)
                do {
                    try data.write(to: temp)
                    transfers.upload(temp, name: "Photo \(stamp)\(suffix).\(ext)")
                } catch {
                    model.connection.showInfo("Couldn't read that photo")
                }
            }
        }
    }
}

/// Videos are loaded as files rather than into memory.
private struct PickedMovie: Transferable {
    let url: URL

    static var transferRepresentation: some TransferRepresentation {
        FileRepresentation(importedContentType: .movie) { received in
            let dest = FileManager.default.temporaryDirectory
                .appendingPathComponent(UUID().uuidString)
                .appendingPathExtension(received.file.pathExtension)
            try FileManager.default.copyItem(at: received.file, to: dest)
            return PickedMovie(url: dest)
        }
    }
}

private struct TransferRow: View {
    let transfer: Transfer

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: transfer.incoming ? "arrow.down.circle" : "arrow.up.circle")
                .font(.title3)
                .foregroundStyle(iconColor)
            VStack(alignment: .leading, spacing: 4) {
                Text(transfer.name).lineLimit(1).truncationMode(.middle)
                switch transfer.state {
                case .waiting:
                    Text("Waiting for the PC…").font(.caption).foregroundStyle(.secondary)
                case .running:
                    ProgressView(value: transfer.progress)
                    if let detail = transfer.detail {
                        Text(detail).font(.caption).foregroundStyle(.secondary)
                    }
                case let .done(text):
                    Text(text).font(.caption).foregroundStyle(.secondary)
                case let .failed(text):
                    Text(text).font(.caption).foregroundStyle(.red)
                }
            }
        }
    }

    private var iconColor: Color {
        switch transfer.state {
        case .done: Theme.accent
        case .failed: .red
        default: .secondary
        }
    }
}

/// Read-only listing of a folder in the PC's home directory, with multi-select downloads.
struct BrowserView: View {
    @Environment(AppModel.self) private var model
    let path: String
    let title: String

    @AppStorage("browseHidden") private var showHidden = false
    @State private var listing: PCListing?
    @State private var error: String?
    @State private var editMode: EditMode = .inactive
    @State private var selection = Set<String>()

    private var selecting: Bool { editMode.isEditing }

    var body: some View {
        List(selection: $selection) {
            if let error {
                Text(error).foregroundStyle(.red)
            }
            if let listing {
                ForEach(listing.entries) { entry in
                    row(entry).tag(entry.name)
                }
                if listing.entries.isEmpty {
                    Text("Empty folder").foregroundStyle(.secondary)
                }
                if listing.truncated {
                    Text("Showing the first \(listing.entries.count) items.").font(.caption).foregroundStyle(.secondary)
                }
            } else if error == nil {
                ProgressView()
            }
        }
        .environment(\.editMode, $editMode)
        .navigationTitle(selecting ? (selection.isEmpty ? "Select items" : "\(selection.count) selected") : title)
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button(selecting ? "Done" : "Select") {
                    withAnimation {
                        editMode = selecting ? .inactive : .active
                        selection = []
                    }
                }
                .disabled(listing?.entries.isEmpty ?? true)
            }
            if !selecting {
                ToolbarItem(placement: .topBarTrailing) {
                    Menu {
                        Toggle("Show hidden", isOn: $showHidden)
                    } label: {
                        Image(systemName: "ellipsis.circle")
                    }
                }
            }
            if selecting {
                ToolbarItemGroup(placement: .bottomBar) {
                    let all = Set(listing?.entries.map(\.name) ?? [])
                    Button(selection == all ? "Deselect all" : "Select all") {
                        selection = selection == all ? [] : all
                    }
                    Spacer()
                    Button {
                        downloadSelected()
                    } label: {
                        Label("Download", systemImage: "arrow.down.circle")
                            .labelStyle(.titleAndIcon)
                    }
                    .disabled(selection.isEmpty)
                }
            }
        }
        .task(id: showHidden) { await load() }
        .refreshable { await load() }
    }

    @ViewBuilder
    private func row(_ entry: PCListing.Entry) -> some View {
        if selecting {
            // While selecting, a tap toggles the checkmark instead of opening or downloading.
            if entry.dir {
                FolderRow(name: entry.name)
            } else {
                FileRow(entry: entry, path: join(entry.name), hidden: showHidden)
            }
        } else if entry.dir {
            NavigationLink {
                BrowserView(path: join(entry.name), title: entry.name)
            } label: {
                FolderRow(name: entry.name)
            }
        } else {
            Button {
                model.transfers.downloadFromPC(path: join(entry.name), name: entry.name, hidden: showHidden)
                model.connection.showInfo("Downloading \(entry.name)")
            } label: {
                FileRow(entry: entry, path: join(entry.name), hidden: showHidden)
            }
            .tint(.primary)
        }
    }

    private func downloadSelected() {
        guard let listing else { return }
        let chosen = listing.entries.filter { selection.contains($0.name) }
        for entry in chosen {
            if entry.dir {
                model.transfers.downloadFolder(path: join(entry.name), name: entry.name, hidden: showHidden)
            } else {
                model.transfers.downloadFromPC(path: join(entry.name), name: entry.name, hidden: showHidden)
            }
        }
        model.connection.showInfo(chosen.count == 1 ? "Downloading \(chosen[0].name)" : "Downloading \(chosen.count) items")
        withAnimation {
            editMode = .inactive
            selection = []
        }
    }

    private func join(_ name: String) -> String {
        path.isEmpty ? name : "\(path)/\(name)"
    }

    private func load() async {
        do {
            listing = try await model.transfers.list(path: path, hidden: showHidden)
            error = nil
        } catch {
            self.error = error.localizedDescription
        }
    }
}

private struct FolderRow: View {
    let name: String

    var body: some View {
        HStack(spacing: 12) {
            Image(systemName: "folder.fill")
                .font(.title3)
                .foregroundStyle(Theme.accent)
                .frame(width: 40, height: 40)
            Text(name).lineLimit(1).truncationMode(.middle)
        }
    }
}

private struct FileRow: View {
    let entry: PCListing.Entry
    let path: String
    let hidden: Bool

    private var type: UTType? { UTType(filenameExtension: (entry.name as NSString).pathExtension) }
    private var hasPreview: Bool { type?.conforms(to: .image) == true || type?.conforms(to: .movie) == true }

    var body: some View {
        HStack(spacing: 12) {
            if hasPreview {
                Thumbnail(path: path, modified: entry.modified, hidden: hidden, fallback: icon, isVideo: type?.conforms(to: .movie) == true)
            } else {
                Image(systemName: icon).font(.title3).foregroundStyle(.secondary).frame(width: 40, height: 40)
            }
            VStack(alignment: .leading, spacing: 2) {
                Text(entry.name).lineLimit(1).truncationMode(.middle)
                Text("\(ByteCountFormatter.string(fromByteCount: entry.size, countStyle: .file)) · \(Date(timeIntervalSince1970: TimeInterval(entry.modified)).formatted(date: .abbreviated, time: .shortened))")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
    }

    private var icon: String {
        if type?.conforms(to: .image) == true { return "photo" }
        if type?.conforms(to: .movie) == true { return "film" }
        if type?.conforms(to: .audio) == true { return "music.note" }
        if type?.conforms(to: .pdf) == true { return "doc.richtext" }
        if type?.conforms(to: .archive) == true { return "doc.zipper" }
        if type?.conforms(to: .text) == true || type?.conforms(to: .sourceCode) == true { return "doc.text" }
        return "doc"
    }
}

/// Lazily loaded preview of a PC image or video.
private struct Thumbnail: View {
    @Environment(AppModel.self) private var model
    let path: String
    let modified: Int64
    let hidden: Bool
    let fallback: String
    let isVideo: Bool
    @State private var image: UIImage?

    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: 6, style: .continuous).fill(Color.white.opacity(0.06))
            if let image {
                Image(uiImage: image).resizable().scaledToFill()
            } else {
                Image(systemName: fallback).foregroundStyle(.secondary)
            }
            if isVideo, image != nil {
                Image(systemName: "play.fill")
                    .font(.caption2)
                    .foregroundStyle(.white)
                    .shadow(radius: 2)
            }
        }
        .frame(width: 40, height: 40)
        .clipShape(RoundedRectangle(cornerRadius: 6, style: .continuous))
        .task(id: "\(path)|\(modified)") {
            image = await model.transfers.thumbnail(path: path, modified: modified, hidden: hidden)
        }
    }
}
