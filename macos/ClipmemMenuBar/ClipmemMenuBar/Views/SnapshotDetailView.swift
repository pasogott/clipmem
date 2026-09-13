import AppKit
import SwiftUI

struct SnapshotDetailView: View {
    let detail: SnapshotDetails?
    let fallback: ClipmemItem?
    let appModel: AppModel
    let configurationGeneration: Int
    var isLoading: Bool = false
    var onForgot: (Int) async -> Void

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var visibleSections = 0
    @State private var advancedMetadataPresented = false
    @State private var confirmForget = false
    @State private var forgetTargetID: Int?
    @State private var imagePreviewState: ImagePreviewState = .notAvailable

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Spacing.xl) {
                if let detail {
                    if visibleSections >= 1 {
                        textSection(detail)
                            .transition(.opacity)
                    }
                    if visibleSections >= 2 {
                        Divider()
                        summaryMetadataSection(detail)
                            .transition(.opacity)
                    }
                    if visibleSections >= 3 {
                        Divider()
                        advancedSection(detail)
                            .transition(.opacity)
                    }
                } else if let fallback {
                    Text(fallback.displayText)
                        .textSelection(.enabled)
                        .font(DesignType.bodyPrimary)
                    Text("Select an item to load full snapshot detail.")
                        .foregroundStyle(.secondary)
                } else if isLoading {
                    loadingSkeleton
                } else {
                    EmptyStateView(title: "No Selection", detail: "Select a clipboard item to inspect it, or use Diagnostics for agent context and setup commands.", symbol: "sidebar.right")
                }
            }
            .padding()
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .overlay {
            if isLoading && detail == nil && fallback != nil {
                loadingSkeleton
                    .padding()
            }
        }
        .task(id: previewDescriptor) {
            await revealSections()
            await loadImagePreview()
        }
        .onDisappear {
            removeLoadedPreview()
            imagePreviewState = .notAvailable
        }
        .confirmationDialog("Forget this snapshot?", isPresented: $confirmForget) {
            Button("Forget", role: .destructive) {
                if let snapshotID = forgetTargetID {
                    Task { await onForgot(snapshotID) }
                }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("This permanently removes the saved content and all records of when it was copied. This cannot be undone.")
        }
    }

    private var previewDescriptor: ImagePreviewDescriptor? {
        guard let detail, let representation = detail.imagePreviewRepresentation else { return nil }
        return ImagePreviewDescriptor(
            snapshotId: detail.snapshotId,
            snapshotSha256: detail.sha256,
            textProjectionVersion: detail.textProjectionVersion,
            representation: representation
        )
    }

    private func revealSections() async {
        advancedMetadataPresented = false
        if reduceMotion || detail == nil {
            visibleSections = 3
            return
        }
        visibleSections = 0
        for section in 1...3 {
            try? await Task.sleep(for: .milliseconds(100))
            guard !Task.isCancelled else { return }
            withAnimation(DesignAnimation.standard) {
                visibleSections = section
            }
        }
    }

    @ViewBuilder
    private func textSection(_ detail: SnapshotDetails) -> some View {
        VStack(alignment: .leading, spacing: Spacing.sm) {
            HStack {
                Text("Content")
                    .font(DesignType.sectionHeader)
                Spacer()
            }

            actionBar(detail)

            if detail.imagePreviewRepresentation != nil {
                imagePreviewView
            }

            if let text = contentText(from: detail) {
                CommandClickableMarkdownText(
                    rendered: MarkdownTextRenderer.renderedText(text, style: .detail),
                    lineLimit: nil,
                    truncationMode: .tail,
                    selectionEnabled: true
                )
                    .font(DesignType.bodyPrimary)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(Spacing.md)
                    .background(Color(.textBackgroundColor), in: .rect(cornerRadius: DesignRadius.md))
                    .shadow(color: .black.opacity(0.04), radius: 2, y: 1)
            } else {
                ContentUnavailableView("No Extracted Text", systemImage: "shippingbox", description: Text("This snapshot appears to be binary, image, PDF, or otherwise has no extracted text. Metadata and export actions are available."))
            }
        }
    }

    @ViewBuilder
    private var imagePreviewView: some View {
        switch imagePreviewState {
        case .notAvailable:
            EmptyView()
        case .loading:
            ProgressView()
                .frame(maxWidth: .infinity, minHeight: 180)
                .background(Color(.textBackgroundColor), in: .rect(cornerRadius: DesignRadius.md))
        case .loaded(_, _, let image):
            Image(nsImage: image)
                .resizable()
                .scaledToFit()
                .frame(maxWidth: .infinity, maxHeight: 380)
                .padding(Spacing.sm)
                .background(Color(.textBackgroundColor), in: .rect(cornerRadius: DesignRadius.md))
                .shadow(color: .black.opacity(0.04), radius: 2, y: 1)
        case .failed(_, let message):
            ContentUnavailableView("Preview Unavailable", systemImage: "photo", description: Text(message))
                .frame(maxWidth: .infinity, minHeight: 160)
                .background(Color(.textBackgroundColor), in: .rect(cornerRadius: DesignRadius.md))
        }
    }

    private func contentText(from detail: SnapshotDetails) -> String? {
        if detail.snapshotKind == .image, let ocrText = detail.ocrText, ocrText.isEmpty == false {
            return ocrText
        }
        if detail.shouldHideImagePlaceholderText, detail.imagePreviewRepresentation != nil {
            return nil
        }
        return bestText(from: detail)
    }

    private func bestText(from detail: SnapshotDetails) -> String? {
        [detail.bestText, detail.previewText, detail.textSummary, detail.ocrText]
            .compactMap { $0 }
            .first(where: { $0.isEmpty == false })
    }

    private func canCopy(_ detail: SnapshotDetails) -> Bool {
        if detail.copyableDetailText?.isEmpty == false {
            return true
        }
        return detail.itemCount > 0
    }

    private func copyOriginalButtonTitle(for detail: SnapshotDetails) -> String {
        if detail.snapshotKind == .image {
            return "Copy Image"
        }
        return "Copy Original"
    }

    private func actionBar(_ detail: SnapshotDetails) -> some View {
        HStack(spacing: Spacing.sm) {
            if let text = detail.copyableDetailText, text.isEmpty == false {
                Button("Copy Text", systemImage: "doc.on.doc") {
                    appModel.copyPlainTextToPasteboard(text)
                }
                .buttonStyle(.borderedProminent)
                .controlSize(.small)
                .help("Copy flattened extracted text")
            }
            if detail.itemCount > 0 {
                Button(copyOriginalButtonTitle(for: detail), systemImage: "doc.on.doc.fill") {
                    Task {
                        guard configurationGeneration == appModel.configurationGeneration else { return }
                        await appModel.copySnapshotToPasteboard(snapshotID: detail.snapshotId)
                    }
                }
                .buttonStyle(.bordered)
                .controlSize(.small)
                .help("Copy this saved clipboard item with its exact original formats")
            }
            Button("Restore", systemImage: "arrow.uturn.backward.square") {
                Task {
                    guard configurationGeneration == appModel.configurationGeneration else { return }
                    await appModel.restore(snapshotID: detail.snapshotId)
                }
            }
            .buttonStyle(.bordered)
            .controlSize(.small)
            Button("Forget", systemImage: "trash", role: .destructive) {
                forgetTargetID = detail.snapshotId
                confirmForget = true
            }
            .buttonStyle(.borderless)
            .controlSize(.small)
            Spacer()
        }
    }

    private func loadImagePreview() async {
        guard let descriptor = previewDescriptor else {
            removeLoadedPreview()
            imagePreviewState = .notAvailable
            return
        }

        let snapshotID = descriptor.snapshotId
        let representation = descriptor.representation
        removeLoadedPreview()
        imagePreviewState = .loading(snapshotID: snapshotID)

        let destination = FileManager.default.temporaryDirectory
            .appendingPathComponent("clipmem-preview-\(snapshotID)-\(representation.itemIndex)-\(UUID().uuidString)")
            .appendingPathExtension(representation.fileExtension)

        do {
            let preview = try await appModel.client.preview(
                snapshotID: snapshotID,
                itemIndex: representation.itemIndex,
                uti: representation.uti,
                destination: destination.path,
                force: true
            )
            if preview.available == false {
                _ = try await appModel.client.export(
                    snapshotID: snapshotID,
                    itemIndex: representation.itemIndex,
                    uti: representation.uti,
                    destination: destination.path,
                    force: true
                )
            }
            guard self.previewDescriptor == descriptor else {
                try? FileManager.default.removeItem(at: destination)
                return
            }
            guard let image = NSImage(contentsOf: destination) else {
                try? FileManager.default.removeItem(at: destination)
                imagePreviewState = .failed(snapshotID: snapshotID, message: "The saved image data could not be decoded.")
                return
            }
            imagePreviewState = .loaded(snapshotID: snapshotID, url: destination, image: image)
        } catch is CancellationError {
            try? FileManager.default.removeItem(at: destination)
        } catch {
            guard self.detail?.snapshotId == snapshotID else {
                try? FileManager.default.removeItem(at: destination)
                return
            }
            imagePreviewState = .failed(snapshotID: snapshotID, message: error.localizedDescription)
        }
    }

    private func removeLoadedPreview() {
        if case .loaded(_, let url, _) = imagePreviewState {
            try? FileManager.default.removeItem(at: url)
        }
    }

    private func summaryMetadataSection(_ detail: SnapshotDetails) -> some View {
        GroupBox("Summary") {
            Grid(alignment: .leading, horizontalSpacing: Spacing.md, verticalSpacing: Spacing.sm) {
                FieldRow(title: "Kind", value: detail.snapshotKind.displayTitle)
                FieldRow(title: "Copied", value: DisplayFormatters.localTimestamp(detail.lastObservedAt ?? detail.firstObservedAt))
                FieldRow(title: "App", value: detail.lastFrontmostAppName.map { "Copied while in \($0)" })
                FieldRow(title: "Bytes", value: DisplayFormatters.byteCount(detail.totalBytes) ?? String(detail.totalBytes))
                FieldRow(title: "OCR status", value: detail.ocrStatus)
                FieldRow(title: "URLs", value: detail.urls.joined(separator: "\n"), lineLimit: 3)
                FieldRow(title: "Files", value: detail.filePaths.joined(separator: "\n"), lineLimit: 3)
            }
        }
    }

    private func advancedSection(_ detail: SnapshotDetails) -> some View {
        DisclosureGroup("Advanced metadata", isExpanded: $advancedMetadataPresented) {
            VStack(alignment: .leading, spacing: Spacing.lg) {
                ItemActionButtons(detail: detail, appModel: appModel)
                metadataSection(detail)
                representationsSection(detail)
                eventsSection(detail)
            }
            .padding(.top, Spacing.sm)
        }
    }

    private func metadataSection(_ detail: SnapshotDetails) -> some View {
        GroupBox("Snapshot") {
            Grid(alignment: .leading, horizontalSpacing: Spacing.md, verticalSpacing: Spacing.sm) {
                FieldRow(title: "Snapshot ID", value: String(detail.snapshotId))
                FieldRow(title: "Content fingerprint", value: detail.sha256, lineLimit: 1)
                FieldRow(title: "First Seen", value: DisplayFormatters.localTimestamp(detail.firstObservedAt))
                FieldRow(title: "Last Seen", value: DisplayFormatters.localTimestamp(detail.lastObservedAt))
                FieldRow(title: "Capture Count", value: String(detail.captureCount))
                FieldRow(title: "App identifier", value: detail.lastFrontmostAppBundleId, lineLimit: 1)
            }
        }
    }

    private func representationsSection(_ detail: SnapshotDetails) -> some View {
        GroupBox("Data Formats") {
            VStack(alignment: .leading, spacing: Spacing.md) {
                ForEach(detail.items) { item in
                    VStack(alignment: .leading, spacing: Spacing.xs) {
                        Text("Item \(item.itemIndex)")
                            .font(.subheadline.weight(.semibold))
                        ForEach(item.representations) { representation in
                            HStack {
                                Text(humanReadableType(representation.uti))
                                    .lineLimit(1)
                                    .truncationMode(.middle)
                                    .frame(maxWidth: .infinity, alignment: .leading)
                                    .help(representation.uti)
                                Text(representation.kind.rawValue)
                                    .lineLimit(1)
                                Text("\(representation.byteLen) bytes")
                                    .monospacedDigit()
                                    .lineLimit(1)
                            }
                            .font(DesignType.rowMeta)
                            .foregroundStyle(.secondary)
                        }
                    }
                }
            }
        }
    }

    private func eventsSection(_ detail: SnapshotDetails) -> some View {
        GroupBox("Recent Events") {
            VStack(alignment: .leading, spacing: Spacing.sm) {
                ForEach(detail.recentEvents) { event in
                    HStack {
                        Text("#\(event.eventId)")
                            .monospacedDigit()
                        Text(DisplayFormatters.relativeTimestamp(event.observedAt) ?? event.observedAt)
                            .help(DisplayFormatters.localTimestamp(event.observedAt) ?? event.observedAt)
                        if let app = event.frontmostAppName {
                            Text("Copied while in \(app)")
                                .lineLimit(1)
                                .truncationMode(.tail)
                        }
                    }
                    .font(DesignType.rowMeta)
                    .foregroundStyle(.secondary)
                }
            }
        }
    }

    private var loadingSkeleton: some View {
        VStack(alignment: .leading, spacing: Spacing.xl) {
            VStack(alignment: .leading, spacing: Spacing.sm) {
                Text("Content")
                    .font(DesignType.sectionHeader)
                RoundedRectangle(cornerRadius: DesignRadius.sm)
                    .fill(.quaternary)
                    .frame(height: 80)
            }
            Divider()
            VStack(alignment: .leading, spacing: Spacing.sm) {
                Text("Metadata")
                    .font(DesignType.sectionHeader)
                ForEach(0..<4, id: \.self) { _ in
                    RoundedRectangle(cornerRadius: DesignRadius.sm)
                        .fill(.quaternary)
                        .frame(height: 16)
                        .frame(maxWidth: 300)
                }
            }
        }
        .redacted(reason: .placeholder)
    }
}

private enum ImagePreviewState {
    case notAvailable
    case loading(snapshotID: Int)
    case loaded(snapshotID: Int, url: URL, image: NSImage)
    case failed(snapshotID: Int, message: String)
}
