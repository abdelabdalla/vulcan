import SwiftUI
import UIKit

/// Entry point of the "Vulcan" share-sheet extension: sends the shared photos, videos or files
/// to the PC's Downloads/Vulcan folder. Unlike "Open in", this takes any number of items.
final class ShareViewController: UIViewController {
    private let model = ShareModel()

    override func viewDidLoad() {
        super.viewDidLoad()
        model.onFinish = { [weak self] in self?.extensionContext?.completeRequest(returningItems: nil) }
        model.onCancel = { [weak self] in self?.extensionContext?.cancelRequest(withError: CocoaError(.userCancelled)) }

        let host = UIHostingController(rootView: ShareView(model: model))
        addChild(host)
        host.view.frame = view.bounds
        host.view.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        view.addSubview(host.view)
        host.didMove(toParent: self)

        let providers = (extensionContext?.inputItems as? [NSExtensionItem] ?? []).flatMap { $0.attachments ?? [] }
        Task { await model.run(providers) }
    }
}

struct ShareView: View {
    let model: ShareModel

    private static let accent = Color(red: 0xa6 / 255, green: 0xe3 / 255, blue: 0xa1 / 255)

    var body: some View {
        NavigationStack {
            content
                .navigationTitle(model.hostname.isEmpty ? "Send to PC" : "Send to \(model.hostname)")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) {
                        if model.phase != .done { Button("Cancel") { model.cancel() } }
                    }
                    ToolbarItem(placement: .confirmationAction) {
                        if model.phase == .done { Button("Done") { model.finish() } }
                    }
                }
        }
        .preferredColorScheme(.dark)
        .tint(Self.accent)
    }

    @ViewBuilder private var content: some View {
        switch model.phase {
        case .preparing:
            ProgressView("Preparing…")
        case let .problem(message):
            ContentUnavailableView("Can't send", systemImage: "exclamationmark.triangle", description: Text(message))
        case .sending, .done:
            List {
                Section {
                    ForEach(model.items) { row($0) }
                } header: {
                    Text(summary)
                } footer: {
                    if model.phase == .done, model.failedCount > 0, model.items.contains(where: { $0.file != nil }) {
                        Button("Retry failed") { model.retryFailed() }
                            .padding(.top, 8)
                    }
                }
            }
        }
    }

    private var summary: String {
        let total = model.items.count
        switch model.phase {
        case .done where model.failedCount == 0:
            return total == 1 ? "Sent to Downloads/Vulcan" : "Sent \(total) items to Downloads/Vulcan"
        case .done:
            return "Sent \(model.sentCount) of \(total)"
        default:
            return "Sending \(min(model.sentCount + model.failedCount + 1, total)) of \(total)"
        }
    }

    private func row(_ item: ShareModel.Item) -> some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 4) {
                Text(item.name).lineLimit(1).truncationMode(.middle)
                switch item.state {
                case .sending:
                    ProgressView(value: item.progress)
                case let .failed(message):
                    Text(message).font(.caption).foregroundStyle(.secondary)
                default:
                    EmptyView()
                }
            }
            Spacer()
            switch item.state {
            case .waiting:
                Image(systemName: "circle").foregroundStyle(.secondary)
            case .sending:
                Image(systemName: "arrow.up.circle").foregroundStyle(Self.accent)
            case .sent:
                Image(systemName: "checkmark.circle.fill").foregroundStyle(Self.accent)
            case .failed:
                Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.yellow)
            }
        }
    }
}
