import SwiftUI
import UniformTypeIdentifiers

// MARK: - Utilities

func humanReadableType(_ uti: String) -> String {
    if let utType = UTType(uti), let desc = utType.localizedDescription {
        return desc
    }
    return uti
}

enum DisplayFormatters {
    static func localTimestamp(
        _ value: String?,
        timeZone: TimeZone = .current,
        locale: Locale = .current
    ) -> String? {
        guard let date = parseTimestamp(value) else { return nil }
        let formatter = FormatterPool.localTimestampFormatter
        formatter.locale = locale
        formatter.timeZone = timeZone
        formatter.dateStyle = .medium
        formatter.timeStyle = .short
        return formatter.string(from: date)
    }

    static func relativeTimestamp(_ value: String?) -> String? {
        guard let date = parseTimestamp(value) else { return nil }
        return FormatterPool.relativeTimestampFormatter.localizedString(for: date, relativeTo: .now)
    }

    static func byteCount(_ bytes: Int?) -> String? {
        guard let bytes else { return nil }
        return ByteCountFormatStyle(style: .file).format(Int64(bytes))
    }

    private static func parseTimestamp(_ value: String?) -> Date? {
        guard let value = value?.trimmingCharacters(in: .whitespacesAndNewlines), value.isEmpty == false else {
            return nil
        }

        if let date = iso8601Date(from: value) {
            return date
        }

        return FormatterPool.sqliteTimestampParser.date(from: value)
    }

    private static func iso8601Date(from value: String) -> Date? {
        if let date = FormatterPool.iso8601FractionalSecondsParser.date(from: value) {
            return date
        }

        return FormatterPool.iso8601Parser.date(from: value)
    }
}

private enum FormatterPool {
    private static let localTimestampFormatterKey = "io.openclaw.clipmem.localTimestampFormatter"
    private static let relativeTimestampFormatterKey = "io.openclaw.clipmem.relativeTimestampFormatter"
    private static let sqliteTimestampParserKey = "io.openclaw.clipmem.sqliteTimestampParser"
    private static let iso8601ParserKey = "io.openclaw.clipmem.iso8601Parser"
    private static let iso8601FractionalSecondsParserKey = "io.openclaw.clipmem.iso8601FractionalSecondsParser"

    static var localTimestampFormatter: DateFormatter {
        threadLocalFormatter(forKey: localTimestampFormatterKey) {
            let formatter = DateFormatter()
            formatter.dateStyle = .medium
            formatter.timeStyle = .short
            return formatter
        }
    }

    static var relativeTimestampFormatter: RelativeDateTimeFormatter {
        threadLocalFormatter(forKey: relativeTimestampFormatterKey) {
            let formatter = RelativeDateTimeFormatter()
            formatter.unitsStyle = .abbreviated
            return formatter
        }
    }

    static var sqliteTimestampParser: DateFormatter {
        threadLocalFormatter(forKey: sqliteTimestampParserKey) {
            let formatter = DateFormatter()
            formatter.locale = Locale(identifier: "en_US_POSIX")
            formatter.timeZone = TimeZone(secondsFromGMT: 0)
            formatter.dateFormat = "yyyy-MM-dd HH:mm:ss"
            return formatter
        }
    }

    static var iso8601Parser: ISO8601DateFormatter {
        threadLocalFormatter(forKey: iso8601ParserKey) {
            let formatter = ISO8601DateFormatter()
            formatter.formatOptions = [.withInternetDateTime]
            return formatter
        }
    }

    static var iso8601FractionalSecondsParser: ISO8601DateFormatter {
        threadLocalFormatter(forKey: iso8601FractionalSecondsParserKey) {
            let formatter = ISO8601DateFormatter()
            formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
            return formatter
        }
    }

    private static func threadLocalFormatter<T: AnyObject>(
        forKey key: String,
        make: () -> T
    ) -> T {
        let dictionary = Thread.current.threadDictionary
        if let formatter = dictionary[key] as? T {
            return formatter
        }

        let formatter = make()
        dictionary[key] = formatter
        return formatter
    }
}

// MARK: - Shared Components

struct StatusBadge: View {
    let state: HealthState

    var body: some View {
        Label(state.title, systemImage: state.symbol)
            .symbolVariant(.fill)
            .foregroundStyle(state.tint)
            .font(DesignType.sectionHeader)
            .labelStyle(.titleAndIcon)
            .accessibilityLabel(state.title)
    }
}

struct EmptyStateView: View {
    let title: String
    let detail: String
    let symbol: String
    var compact: Bool = false

    var body: some View {
        if compact {
            VStack(spacing: Spacing.sm) {
                Image(systemName: symbol)
                    .font(.title2)
                    .foregroundStyle(.secondary)
                Text(title)
                    .font(DesignType.bodySecondary)
                    .fontWeight(.medium)
                Text(detail)
                    .font(DesignType.rowMeta)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
            }
            .padding()
        } else {
            ContentUnavailableView {
                Label(title, systemImage: symbol)
            } description: {
                Text(detail)
            }
        }
    }
}

// MARK: - Banner System

struct BannerContainer<Actions: View>: View {
    let icon: String
    let tint: Color
    let title: String
    var detail: String? = nil
    var pulse: Bool = false
    @ViewBuilder var actions: () -> Actions

    var body: some View {
        HStack(spacing: Spacing.sm) {
            Image(systemName: icon)
                .foregroundStyle(tint)
                .symbolEffect(.pulse, options: .repeating, isActive: pulse)
            VStack(alignment: .leading, spacing: Spacing.xxs) {
                Text(title)
                    .font(DesignType.bodySecondary.weight(.medium))
                    .lineLimit(2)
                if let detail {
                    Text(detail)
                        .font(DesignType.rowMeta)
                        .foregroundStyle(.secondary)
                        .lineLimit(2)
                }
            }
            Spacer()
            actions()
        }
        .bannerStyle(tint: tint)
        .transition(
            .asymmetric(
                insertion: .move(edge: .top).combined(with: .opacity),
                removal: .opacity.combined(with: .offset(y: -8))
            )
        )
    }
}

struct HealthBanner: View {
    let state: HealthState
    var errorDetail: UserError? = nil
    var isRunningAction = false
    var actionLabel: String? = nil
    var onAction: (() -> Void)? = nil

    var body: some View {
        if state != .healthy && state != .unknown {
            BannerContainer(
                icon: state.symbol,
                tint: state.tint,
                title: errorDetail?.message ?? state.title,
                detail: errorDetail?.recovery ?? state.recoveryGuidance,
                pulse: state == .error || state == .conflict || state == .missingBinary
            ) {
                if let actionLabel, let onAction {
                    Button(actionLabel, action: onAction)
                        .buttonStyle(.bordered)
                        .controlSize(.small)
                        .disabled(isRunningAction)
                }
            }
            .accessibilityElement(children: .combine)
            .accessibilityLabel("\(state.title). \(state.recoveryGuidance ?? "")")
        }
    }
}

struct UpdateBanner: View {
    let status: UpdateStatus
    var onCopyCommand: (() -> Void)? = nil
    var onOpenRelease: (() -> Void)? = nil

    var body: some View {
        if status.isUpdateAvailable {
            BannerContainer(
                icon: "arrow.down.circle.fill",
                tint: .blue,
                title: "Update available \u{2014} v\(status.latestVersion ?? "")",
                detail: "You have v\(status.currentVersion)"
            ) {
                if status.shouldShowHomebrewCommand, let onCopyCommand {
                    Button("Copy Command", action: onCopyCommand)
                        .buttonStyle(.bordered)
                        .controlSize(.small)
                } else if let onOpenRelease {
                    Button("Open Release", action: onOpenRelease)
                        .buttonStyle(.bordered)
                        .controlSize(.small)
                        .disabled(status.releaseURL == nil)
                }
            }
        }
    }
}

struct ErrorBanner: View {
    let message: String
    var recovery: String? = nil
    var onRetry: (() -> Void)? = nil

    var body: some View {
        BannerContainer(
            icon: "exclamationmark.triangle",
            tint: .red,
            title: message,
            detail: recovery,
            pulse: true
        ) {
            if let onRetry {
                Button("Retry", action: onRetry)
                    .buttonStyle(.bordered)
                    .controlSize(.small)
            }
        }
    }
}

struct ActionFeedbackOverlay: View {
    let message: String?
    var isSuccess: Bool = true
    var transitionEdge: Edge = .top

    var body: some View {
        if let message {
            HStack(spacing: Spacing.sm) {
                Image(systemName: isSuccess ? "checkmark.circle.fill" : "exclamationmark.circle.fill")
                    .foregroundStyle(isSuccess ? .green : .orange)
                Text(message)
                    .font(DesignType.bodySecondary.weight(.medium))
            }
            .padding(.horizontal, Spacing.lg)
            .padding(.vertical, Spacing.sm)
            .frame(maxWidth: 560)
            .glassOverlay(cornerRadius: 20)
            .transition(
                .asymmetric(
                    insertion: .move(edge: transitionEdge).combined(with: .opacity).animation(DesignAnimation.entrance),
                    removal: .opacity.animation(DesignAnimation.exit)
                )
            )
        }
    }
}

struct FieldRow: View {
    let title: String
    let value: String?
    var lineLimit: Int = 2
    var showPlaceholder: Bool = false

    var body: some View {
        if let value, value.isEmpty == false {
            GridRow {
                Text(title)
                    .foregroundStyle(.secondary)
                    .gridColumnAlignment(.trailing)
                Text(value)
                    .textSelection(.enabled)
                    .lineLimit(lineLimit)
                    .truncationMode(.middle)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .help(value)
            }
        } else if showPlaceholder {
            GridRow {
                Text(title)
                    .foregroundStyle(.secondary)
                    .gridColumnAlignment(.trailing)
                Text("\u{2014}")
                    .foregroundStyle(.tertiary)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
    }
}

struct FilterBar: View {
    @Bindable var history: HistoryModel
    @State private var showAdvanced = false

    private static let timeRanges: [(String, Int)] = [
        ("1h", 1), ("6h", 6), ("24h", 24), ("48h", 48),
        ("7d", 168), ("30d", 720),
    ]

    var body: some View {
        VStack(alignment: .leading, spacing: Spacing.sm) {
            HStack(spacing: Spacing.md) {
                Picker("Time", selection: $history.filters.hours) {
                    Text("All Time").tag(0)
                    ForEach(Self.timeRanges, id: \.1) { label, hours in
                        Text(label).tag(hours)
                    }
                }
                .fixedSize()
                Picker("Kind", selection: $history.filters.kind) {
                    Text("Any").tag(RetrievalKind?.none)
                    ForEach(RetrievalKind.allCases) { kind in
                        Text(kind.title).tag(Optional(kind))
                    }
                }
                .fixedSize()
                Button {
                    withAnimation(DesignAnimation.standard) { showAdvanced.toggle() }
                } label: {
                    HStack(spacing: Spacing.xs) {
                        Label("Filters", systemImage: showAdvanced ? "line.3.horizontal.decrease.circle.fill" : "line.3.horizontal.decrease.circle")
                        if history.filters.activeAdvancedFilterCount > 0 {
                            Text("\(history.filters.activeAdvancedFilterCount)")
                                .font(DesignType.badge)
                                .monospacedDigit()
                                .foregroundStyle(.white)
                                .padding(.horizontal, 5)
                                .padding(.vertical, 1)
                                .background(.blue, in: Capsule())
                                .transition(.scale(scale: 0.25).combined(with: .opacity))
                        }
                    }
                }
                .buttonStyle(.borderless)
            }
            if showAdvanced {
                DisclosureGroup("Advanced Filters") {
                    VStack(alignment: .leading, spacing: Spacing.sm) {
                        HStack(spacing: Spacing.md) {
                            TextField("Application name", text: $history.filters.appName)
                                .textFieldStyle(.roundedBorder)
                                .frame(minWidth: 100)
                            TextField("App identifier", text: $history.filters.bundleID)
                                .textFieldStyle(.roundedBorder)
                                .frame(minWidth: 100)
                        }
                        HStack(spacing: Spacing.md) {
                            Toggle("Text", isOn: $history.filters.hasText)
                            Toggle("URL", isOn: $history.filters.hasURL)
                            Toggle("File", isOn: $history.filters.hasFile)
                            Toggle("Image", isOn: $history.filters.hasImage)
                            Toggle("PDF", isOn: $history.filters.hasPDF)
                            Spacer()
                            if history.filters.activeAdvancedFilterCount > 0 {
                                Button("Reset") {
                                    history.filters.resetAdvanced()
                                }
                                .buttonStyle(.borderless)
                                .foregroundStyle(.secondary)
                            }
                        }
                    }
                }
            }
        }
        .toggleStyle(.checkbox)
        .font(DesignType.bodySecondary)
    }
}
