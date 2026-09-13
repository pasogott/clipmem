import Foundation
import SwiftUI

enum QueryMode: String, CaseIterable, Identifiable, Codable, Hashable, Sendable {
    case recall
    case search
    case recent
    case timeline
    case diagnostics

    var id: String { rawValue }

    var title: String {
        switch self {
        case .recall: "Recall"
        case .search: "Search"
        case .recent: "Recent"
        case .timeline: "Timeline"
        case .diagnostics: "Diagnostics"
        }
    }

    var symbol: String {
        switch self {
        case .recall: "sparkle.magnifyingglass"
        case .search: "magnifyingglass"
        case .recent: "clock"
        case .timeline: "list.bullet.rectangle"
        case .diagnostics: "stethoscope"
        }
    }

    var historyCompatibleMode: QueryMode {
        self == .diagnostics ? .recent : self
    }
}

enum HealthState: String, Sendable {
    case healthy
    case capturePaused
    case watcherStopped
    case noRecentCaptures
    case stale
    case setupNeeded
    case conflict
    case missingBinary
    case error
    case unknown

    var title: String {
        switch self {
        case .healthy: "Capture Running"
        case .capturePaused: "Capture Paused"
        case .watcherStopped: "Watcher Stopped"
        case .noRecentCaptures: "No Recent Captures"
        case .stale: "Capture Stale"
        case .setupNeeded: "Setup Needed"
        case .conflict: "Service Conflict"
        case .missingBinary: "Binary Missing"
        case .error: "Needs Attention"
        case .unknown: "Checking"
        }
    }

    var symbol: String {
        switch self {
        case .healthy: "checkmark.circle.fill"
        case .capturePaused: "pause.circle.fill"
        case .watcherStopped: "stop.circle.fill"
        case .noRecentCaptures: "clock.arrow.circlepath"
        case .stale: "exclamationmark.circle.fill"
        case .setupNeeded: "wrench.and.screwdriver"
        case .conflict: "exclamationmark.triangle.fill"
        case .missingBinary: "questionmark.folder"
        case .error: "xmark.octagon.fill"
        case .unknown: "circle.dotted"
        }
    }

    var tint: Color {
        switch self {
        case .healthy: .green
        case .capturePaused, .watcherStopped, .noRecentCaptures, .stale: .orange
        case .setupNeeded: .blue
        case .conflict, .error, .missingBinary: .red
        case .unknown: .secondary
        }
    }

    var recoveryGuidance: String? {
        switch self {
        case .healthy, .unknown: nil
        case .setupNeeded: "Run setup to initialize the database and start capturing."
        case .missingBinary: "The clipmem binary was not found. Check the path in Settings."
        case .watcherStopped: "The clipboard watcher is not running."
        case .conflict: "Multiple watcher processes detected. Open Settings > Diagnostics to resolve."
        case .error: "The service needs attention. Open Settings > Diagnostics for details."
        case .capturePaused: "Clipboard capture is paused. Resume to start recording again."
        case .stale: "No captures detected recently. The watcher may need a restart."
        case .noRecentCaptures: "The watcher is running but hasn't captured anything yet."
        }
    }

    var menuBarBadgeSymbol: String? {
        switch self {
        case .healthy:
            nil
        case .capturePaused:
            "pause.fill"
        case .watcherStopped:
            "stop.fill"
        case .noRecentCaptures:
            "clock.fill"
        case .stale:
            "exclamationmark"
        case .setupNeeded:
            "plus"
        case .conflict:
            "exclamationmark"
        case .missingBinary:
            "questionmark"
        case .error:
            "xmark"
        case .unknown:
            "ellipsis"
        }
    }

    var menuBarBadgeTone: MenuBarBadgeTone? {
        switch self {
        case .healthy:
            nil
        case .capturePaused, .watcherStopped, .noRecentCaptures, .stale:
            .warning
        case .setupNeeded:
            .setup
        case .conflict, .missingBinary, .error:
            .critical
        case .unknown:
            .neutral
        }
    }
}

enum SettingsTab: String, CaseIterable, Identifiable, Sendable {
    case general
    case storage
    case capture
    case ignoredApps
    case diagnostics
    case privacy

    var id: String { rawValue }

    var title: String {
        switch self {
        case .general: "General"
        case .storage: "Storage"
        case .capture: "Capture"
        case .ignoredApps: "Ignored Apps"
        case .diagnostics: "Diagnostics"
        case .privacy: "Privacy"
        }
    }

    var symbol: String {
        switch self {
        case .general: "gear"
        case .storage: "internaldrive"
        case .capture: "hand.raised"
        case .ignoredApps: "app.badge"
        case .diagnostics: "stethoscope"
        case .privacy: "lock"
        }
    }
}

enum MenuBarBadgeTone: Equatable, Sendable {
    case warning
    case setup
    case critical
    case neutral

    var tint: Color {
        switch self {
        case .warning: .orange
        case .setup: .blue
        case .critical: .red
        case .neutral: .secondary
        }
    }
}

enum RetrievalKind: String, CaseIterable, Identifiable, Codable, Hashable, Sendable {
    case text
    case html
    case rtf
    case url
    case file
    case image
    case pdf
    case binary
    case other

    var id: String { rawValue }

    var title: String {
        switch self {
        case .text: "Text"
        case .html: "HTML"
        case .rtf: "RTF"
        case .url: "URL"
        case .file: "File"
        case .image: "Image"
        case .pdf: "PDF"
        case .binary: "Binary"
        case .other: "Other"
        }
    }
}

// MARK: - Display Layer (UI presentation over QueryMode)

enum SearchStyle: String, CaseIterable, Identifiable, Codable, Hashable, Sendable {
    case smart
    case exact

    var id: String { rawValue }

    var title: String {
        switch self {
        case .smart: "Smart"
        case .exact: "Exact"
        }
    }

    var queryMode: QueryMode {
        switch self {
        case .smart: .recall
        case .exact: .search
        }
    }
}

enum HistoryResultScope: String, CaseIterable, Identifiable, Codable, Hashable, Sendable {
    case uniqueItems
    case copyEvents

    var id: String { rawValue }

    var title: String {
        switch self {
        case .uniqueItems: "Unique items"
        case .copyEvents: "Every copy event"
        }
    }

    var help: String {
        switch self {
        case .uniqueItems: "Show each unique clipboard state once."
        case .copyEvents: "Show every observed copy, including repeats."
        }
    }

    var queryMode: QueryMode {
        switch self {
        case .uniqueItems: .recent
        case .copyEvents: .timeline
        }
    }

    static func from(queryMode: QueryMode) -> HistoryResultScope {
        switch queryMode.historyCompatibleMode {
        case .timeline: .copyEvents
        default: .uniqueItems
        }
    }
}

enum DisplayMode: String, CaseIterable, Identifiable, Codable, Hashable, Sendable {
    case search
    case recent
    case timeline

    var id: String { rawValue }

    var title: String {
        switch self {
        case .search: "Search"
        case .recent: "Recent"
        case .timeline: "Timeline"
        }
    }

    var symbol: String {
        switch self {
        case .search: "magnifyingglass"
        case .recent: "clock"
        case .timeline: "list.bullet.rectangle"
        }
    }

    func queryMode(searchStyle: SearchStyle) -> QueryMode {
        switch self {
        case .search: searchStyle.queryMode
        case .recent: .recent
        case .timeline: .timeline
        }
    }

    /// Map a persisted QueryMode back to DisplayMode + SearchStyle.
    static func from(queryMode: QueryMode) -> (displayMode: DisplayMode, searchStyle: SearchStyle) {
        switch queryMode {
        case .recall: (.search, .smart)
        case .search: (.search, .exact)
        case .recent: (.recent, .smart)
        case .timeline: (.timeline, .smart)
        case .diagnostics: (.recent, .smart)
        }
    }
}

// MARK: - Filters

struct RetrievalFilterState: Equatable, Sendable {
    var hours: Int
    var appName = ""
    var bundleID = ""
    var kind: RetrievalKind?
    var hasText = false
    var hasURL = false
    var hasFile = false
    var hasImage = false
    var hasPDF = false

    var activeAdvancedFilterCount: Int {
        var count = 0
        if !appName.isEmpty { count += 1 }
        if !bundleID.isEmpty { count += 1 }
        if hasText { count += 1 }
        if hasURL { count += 1 }
        if hasFile { count += 1 }
        if hasImage { count += 1 }
        if hasPDF { count += 1 }
        return count
    }

    mutating func resetAdvanced() {
        appName = ""
        bundleID = ""
        hasText = false
        hasURL = false
        hasFile = false
        hasImage = false
        hasPDF = false
    }

    static var defaultValue: RetrievalFilterState {
        RetrievalFilterState(hours: 0)
    }
}
