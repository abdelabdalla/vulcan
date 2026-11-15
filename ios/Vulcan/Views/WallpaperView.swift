import PhotosUI
import SwiftUI
import UniformTypeIdentifiers

/// The PC's wallpaper gallery: tap a picture to set it, or let the PC rotate through them.
struct WallpaperView: View {
    @Environment(AppModel.self) private var model
    /// The picture being set (the PC's script also regenerates the colour scheme, which takes a moment).
    @State private var applying: String?
    @State private var addingFromPC = false
    @State private var pickingPhotos = false
    @State private var photoItems: [PhotosPickerItem] = []
    @State private var uploading: (done: Int, total: Int)?
    @State private var selecting = false
    @State private var selection = Set<String>()
    @State private var confirmRemove = false
    @State private var customInterval = false

    private var conn: ConnectionManager { model.connection }
    /// Previews and adding pictures go over HTTPS, so they need Wi-Fi.
    private var wifi: Bool { conn.caps.contains("files") }

    static let intervals = [60, 300, 600, 900, 1800, 3600, 2 * 3600, 6 * 3600, 12 * 3600, 24 * 3600]

    var body: some View {
        NavigationStack {
            Group {
                if !conn.isConnected {
                    ContentUnavailableView("Not connected", systemImage: "photo.on.rectangle", description: Text("Waiting for the PC…"))
                } else if !conn.caps.contains("wallpaper") {
                    ContentUnavailableView(
                        "Update vulcand",
                        systemImage: "arrow.down.circle",
                        description: Text("The PC's vulcand doesn't know about wallpapers yet.")
                    )
                } else if let gallery = conn.wallpapers {
                    content(gallery)
                } else {
                    ProgressView()
                }
            }
            .navigationTitle(selecting ? (selection.isEmpty ? "Select pictures" : "\(selection.count) selected") : "Wallpaper")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { toolbar }
        }
        .task(id: conn.isConnected) {
            if conn.isConnected { conn.send(.wallList) }
        }
        .onChange(of: conn.wallpapers) { _, _ in applying = nil }
        .onChange(of: conn.toast) { _, toast in
            if toast?.isError == true { applying = nil }
        }
        .sheet(isPresented: $addingFromPC) {
            PCPicturePicker { paths in conn.send(.wallAdd(paths)) }
        }
        .sheet(isPresented: $customInterval) {
            IntervalPicker(initial: conn.wallpapers?.interval ?? 1800) { seconds in
                conn.send(.wallRotation(conn.wallpapers?.rotation ?? .order, interval: seconds))
            }
        }
        .photosPicker(isPresented: $pickingPhotos, selection: $photoItems, maxSelectionCount: 50, matching: .images)
        .onChange(of: photoItems) { _, items in
            guard !items.isEmpty else { return }
            photoItems = []
            Task { await upload(items) }
        }
        .confirmationDialog(
            selection.count == 1 ? "Remove this picture from the gallery?" : "Remove \(selection.count) pictures from the gallery?",
            isPresented: $confirmRemove,
            titleVisibility: .visible
        ) {
            Button("Move to Trash", role: .destructive) {
                conn.send(.wallRemove(Array(selection)))
                withAnimation {
                    selecting = false
                    selection = []
                }
            }
        } message: {
            Text("They go to the PC's trash.")
        }
    }

    // MARK: Content

    private func content(_ gallery: WallpaperGallery) -> some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                rotationCard(gallery)

                if !wifi {
                    Label("Over Bluetooth, previews and adding pictures need Wi-Fi.", systemImage: "wifi.exclamationmark")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
                if let uploading {
                    HStack(spacing: 10) {
                        ProgressView()
                        Text("Sending \(uploading.done + 1) of \(uploading.total) to the gallery…").font(.footnote)
                    }
                }

                if gallery.items.isEmpty {
                    ContentUnavailableView {
                        Label("No wallpapers yet", systemImage: "photo.stack")
                    } description: {
                        Text("Add pictures from the PC or from Photos. They're kept in \(gallery.dir.isEmpty ? "the gallery folder" : "~/\(gallery.dir)") on the PC.")
                    } actions: {
                        if wifi { addMenu.buttonStyle(.borderedProminent) }
                    }
                } else {
                    LazyVGrid(columns: [GridItem(.adaptive(minimum: 150), spacing: 10)], spacing: 10) {
                        ForEach(gallery.items) { item in
                            tile(item, gallery)
                        }
                    }
                }
            }
            .padding(16)
        }
        .refreshable { conn.send(.wallList) }
    }

    private func rotationCard(_ gallery: WallpaperGallery) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                VStack(alignment: .leading, spacing: 2) {
                    Text("On screen").font(.caption).foregroundStyle(.secondary)
                    Text(gallery.current ?? "Not from the gallery").lineLimit(1).truncationMode(.middle)
                }
                Spacer()
                Button {
                    conn.send(.wallNext)
                } label: {
                    Label("Next", systemImage: "forward.fill")
                }
                .buttonStyle(.bordered)
                .disabled(gallery.items.isEmpty)
            }

            Picker("Rotation", selection: Binding(
                get: { gallery.rotation },
                set: { conn.send(.wallRotation($0, interval: nil)) }
            )) {
                ForEach(WallRotation.allCases) { Text($0.label).tag($0) }
            }
            .pickerStyle(.segmented)

            if gallery.rotation != .off {
                HStack {
                    Text("Change every")
                    Spacer()
                    Menu {
                        ForEach(Self.intervals, id: \.self) { seconds in
                            Button(Self.describe(seconds)) {
                                conn.send(.wallRotation(gallery.rotation, interval: seconds))
                            }
                        }
                        Divider()
                        Button("Custom…") { customInterval = true }
                    } label: {
                        HStack(spacing: 4) {
                            Text(Self.describe(gallery.interval))
                            Image(systemName: "chevron.up.chevron.down").font(.caption)
                        }
                    }
                }
            }
        }
        .padding(14)
        .background(Theme.surface, in: RoundedRectangle(cornerRadius: 16, style: .continuous))
        .animation(.default, value: gallery.rotation)
    }

    private func tile(_ item: WallpaperItem, _ gallery: WallpaperGallery) -> some View {
        WallpaperTile(
            item: item,
            dir: gallery.dir,
            isCurrent: item.name == gallery.current,
            isApplying: item.name == applying,
            selected: selecting ? selection.contains(item.name) : nil,
            preview: wifi
        )
        .onTapGesture {
            if selecting {
                if selection.contains(item.name) { selection.remove(item.name) } else { selection.insert(item.name) }
            } else {
                set(item.name)
            }
        }
        .contextMenu {
            if !selecting {
                Button {
                    set(item.name)
                } label: {
                    Label("Set as wallpaper", systemImage: "photo")
                }
                Button(role: .destructive) {
                    selection = [item.name]
                    confirmRemove = true
                } label: {
                    Label("Remove from gallery", systemImage: "trash")
                }
            }
        }
    }

    private func set(_ name: String) {
        UIImpactFeedbackGenerator(style: .light).impactOccurred()
        applying = name
        conn.send(.wallSet(name))
        // Don't spin forever if no answer comes back.
        Task {
            try? await Task.sleep(for: .seconds(30))
            if applying == name { applying = nil }
        }
    }

    // MARK: Toolbar

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        if conn.isConnected, let gallery = conn.wallpapers {
            ToolbarItem(placement: .topBarTrailing) {
                Button(selecting ? "Done" : "Select") {
                    withAnimation {
                        selecting.toggle()
                        selection = []
                    }
                }
                .disabled(gallery.items.isEmpty)
            }
            if !selecting {
                ToolbarItem(placement: .topBarTrailing) {
                    addMenu.disabled(!wifi || uploading != nil)
                }
            } else {
                ToolbarItemGroup(placement: .bottomBar) {
                    let all = Set(gallery.items.map(\.name))
                    Button(selection == all ? "Deselect all" : "Select all") {
                        selection = selection == all ? [] : all
                    }
                    Spacer()
                    Button(role: .destructive) {
                        confirmRemove = true
                    } label: {
                        Label("Remove", systemImage: "trash").labelStyle(.titleAndIcon)
                    }
                    .disabled(selection.isEmpty)
                }
            }
        }
    }

    private var addMenu: some View {
        Menu {
            Button {
                addingFromPC = true
            } label: {
                Label("From the PC…", systemImage: "desktopcomputer")
            }
            Button {
                pickingPhotos = true
            } label: {
                Label("From Photos…", systemImage: "photo.on.rectangle")
            }
        } label: {
            Label("Add", systemImage: "plus")
        }
    }

    // MARK: Photos → gallery

    private func upload(_ items: [PhotosPickerItem]) async {
        let formatter = DateFormatter()
        formatter.dateFormat = "yyyy-MM-dd HH.mm.ss"
        let stamp = formatter.string(from: Date())
        var failures: [String] = []
        for (i, item) in items.enumerated() {
            uploading = (i, items.count)
            let type = item.supportedContentTypes.first { $0.conforms(to: .image) }
            guard let data = try? await item.loadTransferable(type: Data.self) else {
                failures.append("Couldn't read a photo")
                continue
            }
            let ext = type?.preferredFilenameExtension ?? "jpg"
            let suffix = items.count > 1 ? " \(i + 1)" : ""
            do {
                try await model.transfers.uploadWallpaper(data, name: "iPhone \(stamp)\(suffix).\(ext)")
            } catch {
                failures.append(error.localizedDescription)
            }
        }
        uploading = nil
        if let first = failures.first {
            conn.showError(failures.count == 1 ? first : "\(failures.count) photos failed: \(first)")
        } else {
            conn.showInfo(items.count == 1 ? "Added to the wallpapers" : "Added \(items.count) wallpapers")
        }
    }

    static func describe(_ seconds: Int) -> String {
        let f = DateComponentsFormatter()
        f.unitsStyle = .full
        f.allowedUnits = [.day, .hour, .minute, .second]
        f.maximumUnitCount = 2
        return f.string(from: TimeInterval(seconds)) ?? "\(seconds) s"
    }
}

/// One gallery picture. `selected` is nil outside selection mode.
private struct WallpaperTile: View {
    @Environment(AppModel.self) private var model
    let item: WallpaperItem
    let dir: String
    let isCurrent: Bool
    let isApplying: Bool
    let selected: Bool?
    let preview: Bool
    @State private var image: UIImage?

    private var path: String { dir.isEmpty ? item.name : "\(dir)/\(item.name)" }

    var body: some View {
        Color.clear
            .aspectRatio(16 / 9, contentMode: .fit)
            .background(Theme.surface)
            .overlay {
                if let image {
                    Image(uiImage: image).resizable().scaledToFill()
                } else {
                    Image(systemName: "photo").font(.title2).foregroundStyle(.secondary)
                }
            }
            .overlay(alignment: .bottomLeading) {
                Text(item.name)
                    .font(.caption2)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 4)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(.black.opacity(0.5))
            }
            .overlay {
                if isApplying {
                    ProgressView().padding(10).background(.ultraThinMaterial, in: Circle())
                }
            }
            .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
            .overlay {
                RoundedRectangle(cornerRadius: 10, style: .continuous)
                    .strokeBorder(isCurrent ? Theme.accent : .white.opacity(0.08), lineWidth: isCurrent ? 3 : 1)
            }
            .overlay(alignment: .topTrailing) {
                if let selected {
                    Image(systemName: selected ? "checkmark.circle.fill" : "circle")
                        .font(.title3)
                        .foregroundStyle(selected ? Theme.accent : .white)
                        .shadow(radius: 2)
                        .padding(6)
                } else if isCurrent {
                    Image(systemName: "checkmark.circle.fill")
                        .font(.title3)
                        .foregroundStyle(.black, Theme.accent)
                        .padding(6)
                }
            }
            .contentShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(item.name + (isCurrent ? ", current wallpaper" : ""))
            .accessibilityAddTraits(.isButton)
            .task(id: "\(path)|\(item.modified)|\(preview)") {
                guard preview else { return }
                image = await model.transfers.thumbnail(path: path, modified: item.modified, hidden: false, size: 512)
            }
    }
}

/// Hours and minutes for a custom rotation interval.
private struct IntervalPicker: View {
    let initial: Int
    let onDone: (Int) -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var hours = 0
    @State private var minutes = 30

    var body: some View {
        NavigationStack {
            HStack(spacing: 0) {
                Picker("Hours", selection: $hours) {
                    ForEach(0..<73) { Text("\($0) h").tag($0) }
                }
                Picker("Minutes", selection: $minutes) {
                    ForEach(0..<60) { Text("\($0) min").tag($0) }
                }
            }
            .pickerStyle(.wheel)
            .navigationTitle("Change every")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Set") {
                        onDone(hours * 3600 + minutes * 60)
                        dismiss()
                    }
                    .disabled(hours == 0 && minutes == 0)
                }
            }
        }
        .presentationDetents([.height(300)])
        .onAppear {
            hours = initial / 3600
            minutes = (initial % 3600) / 60
        }
    }
}

/// Pick pictures (or whole folders) on the PC to copy into the gallery.
private struct PCPicturePicker: View {
    let onAdd: ([String]) -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var chosen = Set<String>()

    var body: some View {
        NavigationStack {
            PictureFolder(path: "", title: "Home", chosen: $chosen) { add in
                if add { onAdd(chosen.sorted()) }
                dismiss()
            }
        }
    }
}

private struct PictureFolder: View {
    @Environment(AppModel.self) private var model
    let path: String
    let title: String
    @Binding var chosen: Set<String>
    let finish: (_ add: Bool) -> Void
    @State private var listing: PCListing?
    @State private var error: String?

    private static func isPicture(_ name: String) -> Bool {
        UTType(filenameExtension: (name as NSString).pathExtension)?.conforms(to: .image) == true
    }

    private var folders: [PCListing.Entry] { listing?.entries.filter(\.dir) ?? [] }
    private var pictures: [PCListing.Entry] { listing?.entries.filter { !$0.dir && Self.isPicture($0.name) } ?? [] }

    var body: some View {
        List {
            if let error {
                Text(error).foregroundStyle(.red)
            }
            if listing != nil {
                if !pictures.isEmpty, !path.isEmpty {
                    Section {
                        Button {
                            toggle(path)
                        } label: {
                            Label(
                                chosen.contains(path) ? "Every picture here is chosen" : "Choose every picture here",
                                systemImage: chosen.contains(path) ? "checkmark.circle.fill" : "square.stack"
                            )
                        }
                    }
                }
                Section {
                    ForEach(folders) { entry in
                        NavigationLink {
                            PictureFolder(path: join(entry.name), title: entry.name, chosen: $chosen, finish: finish)
                        } label: {
                            HStack {
                                FolderRow(name: entry.name)
                                if chosen.contains(join(entry.name)) {
                                    Spacer()
                                    Image(systemName: "checkmark.circle.fill").foregroundStyle(Theme.accent)
                                }
                            }
                        }
                    }
                    ForEach(pictures) { entry in
                        Button {
                            toggle(join(entry.name))
                        } label: {
                            HStack {
                                FileRow(entry: entry, path: join(entry.name), hidden: false)
                                Spacer()
                                Image(systemName: chosen.contains(join(entry.name)) ? "checkmark.circle.fill" : "circle")
                                    .foregroundStyle(chosen.contains(join(entry.name)) ? Theme.accent : .secondary)
                            }
                        }
                        .tint(.primary)
                    }
                    if folders.isEmpty, pictures.isEmpty {
                        Text("No pictures or folders here").foregroundStyle(.secondary)
                    }
                }
            } else if error == nil {
                ProgressView()
            }
        }
        .navigationTitle(title)
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .cancellationAction) {
                Button("Cancel") { finish(false) }
            }
            ToolbarItem(placement: .confirmationAction) {
                Button(chosen.isEmpty ? "Add" : "Add (\(chosen.count))") { finish(true) }
                    .disabled(chosen.isEmpty)
            }
        }
        .task { await load() }
    }

    private func toggle(_ p: String) {
        if chosen.contains(p) { chosen.remove(p) } else { chosen.insert(p) }
    }

    private func join(_ name: String) -> String {
        path.isEmpty ? name : "\(path)/\(name)"
    }

    private func load() async {
        do {
            listing = try await model.transfers.list(path: path, hidden: false)
            error = nil
        } catch {
            self.error = error.localizedDescription
        }
    }
}
