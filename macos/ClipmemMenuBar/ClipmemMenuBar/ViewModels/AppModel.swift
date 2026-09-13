import AppKit
import CoreFoundation
import Foundation
import Observation
import SwiftUI

struct HistoryOpenRequest: Equatable, Sendable {
    var id: Int
    var mode: QueryMode
    var query: String
    var focusedSnapshotID: Int?
}

struct SettingsOpenRequest: Equatable, Sendable {
    var id: Int
    var tab: SettingsTab
}

@MainActor
@Observable
final class AppModel {
    var serviceStatus: ServiceStatusReport?
    var doctorReport: DoctorReport?
    var settingsReport: SettingsReport?
    var recentPreview: [ClipmemItem] = []
    var recentPreviewRefreshError: UserError?
    var clipboardHistoryRevision = 0
    var lastError: UserError?
    var actionMessage: String?
    var hotkeyMessage: String?
    var hotkeyEnabled = UserDefaults.standard.clipmemHotkeyEnabled
    var launchAtLoginEnabled = UserDefaults.standard.clipmemLaunchAtLoginEnabled
    var launchAtLoginStatus = LoginItemController.status()
    var launchAtLoginError: UserError?
    var defaultRecentHours = UserDefaults.standard.clipmemDefaultHours
    var defaultQueryMode = UserDefaults.standard.clipmemDefaultMode
    var isRefreshing = false
    var isRunningAction = false
    var imageOptimizationProgress: ImageOptimizationProgressState?
    var updateStatus = UpdateStatus.load()
    var pendingHistoryOpenRequest: HistoryOpenRequest?
    var pendingSettingsOpenRequest: SettingsOpenRequest?
    @ObservationIgnored private let hotKeyManager = HotKeyManager()
    @ObservationIgnored private let updateChecker = UpdateChecker()
    @ObservationIgnored private var lastUpdateAttemptAt: Date?
    @ObservationIgnored private let startupMode: AppStartupMode
    @ObservationIgnored private let loadRecentPreview: (@MainActor () async throws -> [ClipmemItem])?
    @ObservationIgnored private var historyOpenRequestID = 0
    @ObservationIgnored private var settingsOpenRequestID = 0
    @ObservationIgnored private var pasteboardMonitor: PasteboardChangeMonitor?
    @ObservationIgnored private var appRefreshNotificationMonitor: AppRefreshNotificationMonitor?
    @ObservationIgnored private var revisionMonitorTask: Task<Void, Never>?
    @ObservationIgnored private var observedRevision: ArchiveRevision?
    @ObservationIgnored private var statusRefreshInFlight = false
    @ObservationIgnored private var lastStatusRefreshAt: Date?
    @ObservationIgnored private var revisionRefreshInFlight = false
    private(set) var configurationGeneration = 0
    @ObservationIgnored private var activeConfiguration = ClipmemClientConfiguration.current
    @ObservationIgnored private var recentRefreshCoordinator: RecentPreviewRefreshCoordinator?
    @ObservationIgnored private var recentPreviewRefreshedAt: Date?
    @ObservationIgnored private var openQuickRecallAction: (@MainActor () -> Void)?
    @ObservationIgnored private var lastResolvedBinaryPath = UserDefaults.standard.string(forKey: PreferenceKey.binaryPathOverride)
    @ObservationIgnored private var lastResolvedDatabasePath = UserDefaults.standard.string(forKey: PreferenceKey.databasePathOverride)
    init(startupMode: AppStartupMode = .current, loadRecentPreview: (@MainActor () async throws -> [ClipmemItem])? = nil) {
        self.startupMode = startupMode
        self.loadRecentPreview = loadRecentPreview
    }
    var healthState: HealthState {
        if client.resolvedBinaryPath() == nil {
            return .missingBinary
        }
        guard var status = serviceStatus else { return .unknown }
        status.paused = settingsReport?.paused ?? status.paused
        return status.health
    }
    var client: ClipmemClient {
        ClipmemClient(configuration: activeConfiguration)
    }
    @discardableResult
    func start() async -> Bool {
        guard startupMode.allowsApplicationSideEffects, Task.isCancelled == false else { return false }
        configureDefaultLaunchAtLoginIfNeeded()
        await installSelfIgnoreIfNeeded()
        guard Task.isCancelled == false else { return false }
        await refreshAll()
        guard Task.isCancelled == false else { return false }
        startPasteboardMonitorIfNeeded()
        startAppRefreshNotificationMonitorIfNeeded()
        startRevisionMonitorIfNeeded()
        await checkForUpdatesIfNeeded()
        return Task.isCancelled == false
    }
    deinit {
        revisionMonitorTask?.cancel()
        appRefreshNotificationMonitor?.stop()
    }
    func refreshAll() async {
        isRefreshing = true
        defer { isRefreshing = false }
        lastError = nil
        async let statusTask: Void = refreshStatus()
        async let settingsTask: Void = refreshSettings()
        async let recentTask: Bool = refreshRecentPreview()
        _ = await (statusTask, settingsTask, recentTask)
    }
    func refreshStatus() async {
        guard !statusRefreshInFlight else { return }
        statusRefreshInFlight = true
        lastStatusRefreshAt = Date()
        let generation = configurationGeneration
        defer { statusRefreshInFlight = false }
        do {
            let status = try await client.serviceStatus()
            guard generation == configurationGeneration else { return }
            serviceStatus = status
            observedRevision = observedRevision ?? status.revision
        } catch {
            guard generation == configurationGeneration else { return }
            serviceStatus = nil
            lastError = UserError(error)
        }
    }

    func refreshDoctor() async {
        let generation = configurationGeneration
        do {
            let report = try await client.doctor()
            guard generation == configurationGeneration else { return }
            doctorReport = report
        } catch {
            guard generation == configurationGeneration else { return }
            doctorReport = nil
            lastError = UserError(error)
        }
    }

    func refreshSettings() async {
        let generation = configurationGeneration
        do {
            let report = try await client.settings()
            guard generation == configurationGeneration else { return }
            settingsReport = report
        } catch {
            guard generation == configurationGeneration else { return }
            settingsReport = nil
            lastError = UserError(error)
        }
    }

    @discardableResult
    func refreshRecentPreview() async -> Bool {
        let generation = configurationGeneration
        do {
            let loadedPreview: [ClipmemItem]
            if let loadRecentPreview {
                loadedPreview = try await loadRecentPreview()
            } else {
                loadedPreview = try await client.recent(limit: 40, cursor: nil, filters: RetrievalFilterState(hours: defaultRecentHours)).results
            }
            guard generation == configurationGeneration else { return false }
            let changed = loadedPreview != recentPreview
            recentPreview = loadedPreview
            recentPreviewRefreshedAt = Date()
            recentPreviewRefreshError = nil
            return changed
        } catch {
            guard generation == configurationGeneration else { return false }
            recentPreviewRefreshError = UserError(error)
            return false
        }
    }

    func refreshRecentPreviewIfStale(maxAge: TimeInterval) async {
        if let recentPreviewRefreshedAt, Date().timeIntervalSince(recentPreviewRefreshedAt) < maxAge {
            return
        }
        await recentCoordinator().refreshNow()
    }

    func runSetup() async {
        if await runAction(.setup(), successMessage: "Setup completed.") {
            await refreshAll()
        }
    }

    func serviceAction(_ action: String) async {
        if await runAction(.service(action), successMessage: "Service \(action) completed.") {
            await refreshAll()
        }
    }

    func compactDatabase() async {
        let generation = configurationGeneration
        isRunningAction = true
        actionMessage = nil
        defer { isRunningAction = false }
        do {
            let report = try await client.storageCompact(dryRun: false)
            guard generation == configurationGeneration else { return }
            lastError = nil
            showActionMessage(
                "Compacted database. Reclaimed \(formatBytes(report.reclaimedBytes)).",
                duration: .seconds(8)
            )
            await refreshStatus()
        } catch {
            guard generation == configurationGeneration else { return }
            lastError = UserError(error)
            actionMessage = nil
        }
    }

    func optimizeImages() async {
        let generation = configurationGeneration
        isRunningAction = true
        actionMessage = nil
        imageOptimizationProgress = nil
        defer {
            isRunningAction = false
            imageOptimizationProgress = nil
        }
        do {
            let report = try await client.storageOptimizeImagesWithProgress(dryRun: false, limit: nil) { event in
                await MainActor.run {
                    guard generation == self.configurationGeneration else { return }
                    self.applyImageOptimizationProgress(event)
                }
            }
            guard generation == configurationGeneration else { return }
            lastError = nil
            let saved = DisplayFormatters.byteCount(report.logicalSavedBytes) ?? "\(report.logicalSavedBytes) bytes"
            let reclaimed = formatBytes(report.filesystemSavedBytes)
            if let compactError = report.compactError {
                showActionMessage(
                    "Compressed \(report.compressedRows) images. Reduced image bytes by \(saved), but database compaction failed: \(compactError). Run Compact Database to retry.",
                    duration: .seconds(8)
                )
            } else if report.compactRun {
                showActionMessage(
                    "Compressed \(report.compressedRows) images. Reduced image bytes by \(saved) and reclaimed \(reclaimed) from the database.",
                    duration: .seconds(8)
                )
            } else if report.compactRecommended {
                showActionMessage(
                    "Compressed \(report.compressedRows) images. Reduced image bytes by \(saved). Run Compact Database to return freed pages to disk.",
                    duration: .seconds(8)
                )
            } else {
                showActionMessage(
                    "Compressed \(report.compressedRows) images. Reduced image bytes by \(saved).",
                    duration: .seconds(8)
                )
            }
            await refreshStatus()
        } catch {
            guard generation == configurationGeneration else { return }
            lastError = UserError(error)
            actionMessage = nil
        }
    }

    private func applyImageOptimizationProgress(_ event: ImageOptimizationProgressEvent) {
        switch event {
        case .started(let totalRows):
            imageOptimizationProgress = ImageOptimizationProgressState(
                phase: .scanning,
                scannedRows: 0,
                totalRows: totalRows,
                compressedRows: 0,
                skippedRows: 0,
                conflictCount: 0
            )
        case .scanning(let snapshot):
            imageOptimizationProgress = ImageOptimizationProgressState(
                phase: .scanning,
                scannedRows: snapshot.scannedRows,
                totalRows: snapshot.totalRows,
                compressedRows: snapshot.compressedRows,
                skippedRows: snapshot.skippedRows,
                conflictCount: snapshot.conflictCount
            )
        case .compacting(let snapshot):
            imageOptimizationProgress = ImageOptimizationProgressState(
                phase: .compacting,
                scannedRows: snapshot.scannedRows,
                totalRows: snapshot.totalRows,
                compressedRows: snapshot.compressedRows,
                skippedRows: snapshot.skippedRows,
                conflictCount: snapshot.conflictCount
            )
        case .complete:
            break
        }
    }

    func previewPurge(olderThan: String) async -> PurgeOutput? {
        let generation = configurationGeneration
        let threshold = olderThan.trimmingCharacters(in: .whitespacesAndNewlines)
        guard threshold.isEmpty == false else {
            lastError = UserError(message: "Enter a purge threshold.", recovery: "Use a duration like 30d, 12h, or 15m.")
            return nil
        }

        isRunningAction = true
        actionMessage = nil
        defer { isRunningAction = false }
        do {
            let report = try await client.purge(olderThan: threshold, dryRun: true)
            guard generation == configurationGeneration else { return nil }
            lastError = nil
            return report
        } catch {
            guard generation == configurationGeneration else { return nil }
            lastError = UserError(error)
            return nil
        }
    }

    func purge(olderThan: String) async -> PurgeOutput? {
        let generation = configurationGeneration
        let threshold = olderThan.trimmingCharacters(in: .whitespacesAndNewlines)
        guard threshold.isEmpty == false else {
            lastError = UserError(message: "Enter a purge threshold.", recovery: "Use a duration like 30d, 12h, or 15m.")
            return nil
        }

        isRunningAction = true
        actionMessage = nil
        defer { isRunningAction = false }
        do {
            let report = try await client.purge(olderThan: threshold, dryRun: false)
            guard generation == configurationGeneration else { return nil }
            lastError = nil
            showActionMessage(
                "Purged \(formatCount(report.snapshotCount, singular: "snapshot")) older than \(threshold). Removed \(formatBytes(UInt64(report.totalBytes))).",
                duration: .seconds(8)
            )
            await refreshStatus()
            await refreshSettings()
            _ = await refreshRecentPreview()
            clipboardHistoryRevision += 1
            return report
        } catch {
            guard generation == configurationGeneration else { return nil }
            lastError = UserError(error)
            actionMessage = nil
            return nil
        }
    }

    private func formatBytes(_ bytes: UInt64) -> String {
        let clamped = min(bytes, UInt64(Int.max))
        return DisplayFormatters.byteCount(Int(clamped)) ?? "\(bytes) bytes"
    }

    private func formatCount(_ count: Int, singular: String) -> String {
        count == 1 ? "1 \(singular)" : "\(count) \(singular)s"
    }

    @discardableResult
    func runAction(_ command: ClipmemCommand, successMessage: String? = nil) async -> Bool {
        let generation = configurationGeneration
        isRunningAction = true
        actionMessage = nil
        defer { isRunningAction = false }
        do {
            try await client.runAction(command)
            guard generation == configurationGeneration else { return false }
            lastError = nil
            showActionMessage(successMessage)
            return true
        } catch {
            guard generation == configurationGeneration else { return false }
            lastError = UserError(error)
            actionMessage = nil
            return false
        }
    }

    @discardableResult
    func restore(_ item: ClipmemItem) async -> Bool {
        await restore(snapshotID: item.snapshotId, successMessage: "Restored to clipboard")
    }

    @discardableResult
    func restore(snapshotID: Int, successMessage: String = "Restored to clipboard") async -> Bool {
        let generation = configurationGeneration
        do {
            _ = try await client.restore(snapshotID: snapshotID)
            pasteboardMonitor?.markCurrentChangeHandled()
            guard generation == configurationGeneration else { return false }
            lastError = nil
            showActionMessage(successMessage)
            await refreshRecentPreview()
            return true
        } catch {
            guard generation == configurationGeneration else { return false }
            lastError = UserError(error)
            return false
        }
    }

    func copySnapshotToPasteboard(snapshotID: Int) async {
        await restore(snapshotID: snapshotID, successMessage: "Copied to clipboard")
    }

    func copyPlainTextToPasteboard(_ text: String) {
        PasteboardActions.copyPlainText(text)
        pasteboardMonitor?.markCurrentChangeHandled()
        lastError = nil
        showActionMessage("Copied to clipboard")
    }

    @discardableResult
    func forget(_ item: ClipmemItem) async -> Bool {
        await forget(snapshotID: item.snapshotId)
    }
    @discardableResult
    func forget(snapshotID: Int) async -> Bool {
        let generation = configurationGeneration
        do {
            _ = try await client.forget(snapshotID: snapshotID)
            guard generation == configurationGeneration else { return false }
            recentPreview.removeAll { $0.snapshotId == snapshotID }
            guard generation == configurationGeneration else { return false }
            lastError = nil
            return true
        } catch {
            guard generation == configurationGeneration else { return false }
            lastError = UserError(error)
            return false
        }
    }

    func openLogsFolder() {
        guard let path = serviceStatus?.logPaths.first else { return }
        let url = URL(fileURLWithPath: path).deletingLastPathComponent()
        NSWorkspace.shared.activateFileViewerSelecting([url])
    }

    func checkForUpdatesIfNeeded() async {
        guard updateStatus.shouldCheck() else { return }
        await checkForUpdates(force: false, manual: false)
    }

    func checkForUpdates(force: Bool = true, manual: Bool = true) async {
        if updateStatus.isChecking {
            return
        }
        if force == false, updateStatus.shouldCheck() == false {
            return
        }

        if !force, let lastUpdateAttemptAt, Date().timeIntervalSince(lastUpdateAttemptAt) < 3600 {
            return
        }
        lastUpdateAttemptAt = Date()
        updateStatus.beginCheck(manual: manual)
        do {
            let result = try await updateChecker.latestStableRelease()
            updateStatus.applySuccess(result)
        } catch {
            updateStatus.applyFailure(error, manual: manual)
        }
    }

    func copyUpgradeCommand() {
        PasteboardActions.copyPlainText(UpdateChecker.homebrewUpgradeCommand)
        showActionMessage("Upgrade command copied")
    }

    func copyAgentContextCommand() {
        PasteboardActions.copyPlainText(agentCommand("agents context --format json"))
        showActionMessage("Agent context command copied")
    }

    func copyAgentSkillInstallCommand() {
        PasteboardActions.copyPlainText(agentCommand("agents openclaw install-skill"))
        showActionMessage("Agent skill install command copied")
    }

    func copyAgentOpenClawDoctorCommand() {
        PasteboardActions.copyPlainText(agentCommand("agents openclaw doctor"))
        showActionMessage("OpenClaw doctor command copied")
    }

    func copyAgentHermesDoctorCommand() {
        PasteboardActions.copyPlainText(agentCommand("agents hermes doctor"))
        showActionMessage("Hermes doctor command copied")
    }

    func copyAgentPrintSkillCommand() {
        PasteboardActions.copyPlainText(agentCommand("agents openclaw print-skill"))
        showActionMessage("Print skill command copied")
    }

    func copyAgentCapabilityMapCommand() {
        PasteboardActions.copyPlainText("\(agentCommand("agents context --format json")) | jq '.capabilities'")
        showActionMessage("Capability map command copied")
    }

    private func agentCommand(_ arguments: String) -> String {
        let databasePath = UserDefaults.standard.string(forKey: PreferenceKey.databasePathOverride)?
            .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        guard databasePath.isEmpty == false else {
            return "clipmem \(arguments)"
        }
        return "clipmem --db \(shellQuoted(databasePath)) \(arguments)"
    }

    private func shellQuoted(_ value: String) -> String {
        "'\(value.replacingOccurrences(of: "'", with: "'\\''"))'"
    }

    func openUpdateRelease() {
        guard let releaseURL = updateStatus.releaseURL else { return }
        NSWorkspace.shared.open(releaseURL)
    }

    func requestHistorySearch(query: String) {
        let trimmedQuery = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmedQuery.isEmpty == false else { return }
        enqueueHistoryOpenRequest(mode: .search, query: trimmedQuery, focusedSnapshotID: nil)
    }

    func requestHistoryFocus(snapshotID: Int, mode: QueryMode, query: String) {
        let historyMode = mode.historyCompatibleMode
        let trimmedQuery = query.trimmingCharacters(in: .whitespacesAndNewlines)
        let historyQuery = switch historyMode {
        case .recall, .search:
            trimmedQuery
        case .recent, .timeline, .diagnostics:
            ""
        }
        enqueueHistoryOpenRequest(mode: historyMode, query: historyQuery, focusedSnapshotID: snapshotID)
    }

    func requestSettingsTab(_ tab: SettingsTab) {
        settingsOpenRequestID += 1
        pendingSettingsOpenRequest = SettingsOpenRequest(id: settingsOpenRequestID, tab: tab)
    }

    func configureHotkey(enabled: Bool, openQuickRecall: @escaping @MainActor () -> Void) {
        openQuickRecallAction = openQuickRecall
        hotkeyEnabled = enabled
        if enabled {
            hotkeyMessage = hotKeyManager.registerDefault(action: openQuickRecall)
        } else {
            unregisterHotkey()
        }
    }

    func unregisterHotkey() {
        hotKeyManager.unregister()
        hotkeyMessage = nil
    }

    func setLaunchAtLoginEnabled(_ enabled: Bool) {
        UserDefaults.standard.set(true, forKey: PreferenceKey.didConfigureLaunchAtLogin)
        do {
            try LoginItemController.setEnabled(enabled)
            UserDefaults.standard.set(enabled, forKey: PreferenceKey.launchAtLoginEnabled)
            launchAtLoginEnabled = enabled
            launchAtLoginStatus = LoginItemController.status()
            launchAtLoginError = nil
        } catch {
            launchAtLoginStatus = LoginItemController.status()
            launchAtLoginEnabled = launchAtLoginStatus == .enabled
            UserDefaults.standard.set(launchAtLoginEnabled, forKey: PreferenceKey.launchAtLoginEnabled)
            launchAtLoginError = UserError(
                message: "Could not update launch at login.",
                recovery: error.localizedDescription
            )
        }
    }

    // MARK: - Private

    private func showActionMessage(_ message: String?, duration: Duration = .seconds(2.5)) {
        actionMessage = message
        if let message {
            Task {
                try? await Task.sleep(for: duration)
                if self.actionMessage == message {
                    withAnimation { self.actionMessage = nil }
                }
            }
        }
    }

    private func enqueueHistoryOpenRequest(mode: QueryMode, query: String, focusedSnapshotID: Int?) {
        historyOpenRequestID += 1
        pendingHistoryOpenRequest = HistoryOpenRequest(
            id: historyOpenRequestID,
            mode: mode,
            query: query,
            focusedSnapshotID: focusedSnapshotID
        )
    }

    private func installSelfIgnoreIfNeeded() async {
        do {
            try await client.runAction(.settingsIgnoreAdd("io.openclaw.clipmem.menubar"))
        } catch {
            AppLoggers.service.info("Self ignore setup was skipped or failed")
        }
    }

    private func configureDefaultLaunchAtLoginIfNeeded() {
        let defaults = UserDefaults.standard
        switch LaunchAtLoginDefaultConfigurator.configureIfNeeded(defaults: defaults) {
        case .enableLoginItem:
            setLaunchAtLoginEnabled(true)
            return
        case .refreshFromDefaults:
            break
        }
        launchAtLoginEnabled = defaults.clipmemLaunchAtLoginEnabled
        launchAtLoginStatus = LoginItemController.status()
    }

    private func startPasteboardMonitorIfNeeded() {
        if pasteboardMonitor != nil { return }
        let monitor = PasteboardChangeMonitor { [weak self] in
            self?.recentCoordinator().schedule()
        }
        pasteboardMonitor = monitor
        monitor.start()
    }

    private func startRevisionMonitorIfNeeded() {
        guard revisionMonitorTask == nil else { return }
        revisionMonitorTask = Task { [weak self] in
            while Task.isCancelled == false {
                try? await Task.sleep(for: .seconds(2))
                guard Task.isCancelled == false else { return }
                await self?.pollArchiveRevision()
            }
        }
    }

    private func startAppRefreshNotificationMonitorIfNeeded() {
        guard appRefreshNotificationMonitor == nil else { return }
        let monitor = AppRefreshNotificationMonitor { [weak self] in
            Task { await self?.handleAppRefreshNotification() }
        }
        appRefreshNotificationMonitor = monitor
        monitor.start()
    }

    private func handleAppRefreshNotification() async {
        await refreshAppPreferences()
        await pollArchiveRevision()
    }

    private func pollArchiveRevision() async {
        await checkForUpdatesIfNeeded()
        guard revisionRefreshInFlight == false else { return }
        revisionRefreshInFlight = true
        defer { revisionRefreshInFlight = false }

        let defaults = UserDefaults.standard
        defaults.synchronize()
        if lastResolvedBinaryPath != defaults.string(forKey: PreferenceKey.binaryPathOverride)
            || lastResolvedDatabasePath != defaults.string(forKey: PreferenceKey.databasePathOverride) {
            await refreshAppPreferences()
            return
        }

        if lastStatusRefreshAt.map({ Date().timeIntervalSince($0) >= 30 }) ?? true {
            await refreshStatus()
        }
        let generation = configurationGeneration
        do {
            let next = try await client.serviceRevision()
            guard generation == configurationGeneration else { return }
            guard let previous = observedRevision else {
                observedRevision = next
                return
            }
            guard next.revision > previous.revision else {
                observedRevision = next
                return
            }

            await refreshForRevisionChange(from: previous, to: next)
            guard generation == configurationGeneration else { return }
            observedRevision = next
            lastError = nil
        } catch {
            guard generation == configurationGeneration else { return }
            if case ClipmemClientError.setupNeeded = error {
                observedRevision = nil
                return
            }
            lastError = UserError(error)
        }
    }

    private func refreshForRevisionChange(from previous: ArchiveRevision, to next: ArchiveRevision) async {
        let archiveChanged = next.archiveContentRevision != previous.archiveContentRevision
        let ocrChanged = next.ocrRevision != previous.ocrRevision
        let settingsChanged = next.settingsRevision != previous.settingsRevision
        let storageChanged = next.storageRevision != previous.storageRevision
        let serviceChanged = next.serviceRevision != previous.serviceRevision
        let appPreferencesChanged = next.appPreferencesRevision != previous.appPreferencesRevision

        if serviceChanged {
            await refreshStatus()
        }
        if settingsChanged {
            await refreshSettings()
        }
        if appPreferencesChanged {
            await refreshAppPreferences()
        }
        if archiveChanged || ocrChanged {
            _ = await refreshRecentPreview()
            clipboardHistoryRevision += 1
        }
        if storageChanged, doctorReport != nil {
            await refreshDoctor()
        }
    }

    private func recentCoordinator() -> RecentPreviewRefreshCoordinator {
        if let recentRefreshCoordinator {
            return recentRefreshCoordinator
        }
        let coordinator = RecentPreviewRefreshCoordinator { [weak self] in
            guard let self else { return false }
            let refreshed = await self.refreshRecentPreview()
            if refreshed {
                self.clipboardHistoryRevision += 1
            }
            return refreshed
        }
        recentRefreshCoordinator = coordinator
        return coordinator
    }

    func adoptConfiguration(_ configuration: ClipmemClientConfiguration) {
        activeConfiguration = configuration
        configurationGeneration += 1
        clipboardHistoryRevision += 1
        recentPreview = []
        actionMessage = nil
        imageOptimizationProgress = nil
        recentPreviewRefreshedAt = nil
        lastStatusRefreshAt = nil
        pendingHistoryOpenRequest = nil
        lastError = nil
        recentPreviewRefreshError = nil
        serviceStatus = nil
        doctorReport = nil
        settingsReport = nil
        observedRevision = nil
        recentRefreshCoordinator = nil
    }

    func applyPaths(binary: String, database: String) async -> Bool {
        guard !isRunningAction else {
            lastError = UserError(message: "Wait for the current action to finish before changing paths.")
            return false
        }
        let environment = ProcessInfo.processInfo.environment
        if let forcedDatabase = environment["CLIPMEM_DB_PATH"], forcedDatabase != database.trimmingCharacters(in: .whitespacesAndNewlines) {
            lastError = UserError(message: "The database path is controlled by CLIPMEM_DB_PATH. Relaunch without that override to change it here.")
            return false
        }
        if let forcedBinary = environment["CLIPMEM_BINARY_PATH"], forcedBinary != binary.trimmingCharacters(in: .whitespacesAndNewlines) {
            lastError = UserError(message: "The binary path is controlled by CLIPMEM_BINARY_PATH. Relaunch without that override to change it here.")
            return false
        }
        var candidate = activeConfiguration
        candidate.binaryOverride = binary.trimmingCharacters(in: .whitespacesAndNewlines)
        candidate.databaseOverride = database.trimmingCharacters(in: .whitespacesAndNewlines)
        do {
            _ = try await ClipmemClient(configuration: candidate).serviceRevision()
            UserDefaults.standard.set(candidate.binaryOverride, forKey: PreferenceKey.binaryPathOverride)
            UserDefaults.standard.set(candidate.databaseOverride, forKey: PreferenceKey.databasePathOverride)
            await refreshAppPreferences()
            lastError = nil
            return true
        } catch {
            lastError = UserError(error)
            return false
        }
    }

    private func refreshAppPreferences() async {
        let defaults = UserDefaults.standard
        defaults.synchronize()

        let previousBinaryPath = lastResolvedBinaryPath
        let previousDatabasePath = lastResolvedDatabasePath
        let previousHotkeyEnabled = hotkeyEnabled
        let previousLaunchAtLoginEnabled = launchAtLoginEnabled
        let previousUpdateStatus = updateStatus
        let previousDefaultRecentHours = defaultRecentHours
        let previousDefaultQueryMode = defaultQueryMode
        let nextBinaryPath = defaults.string(forKey: PreferenceKey.binaryPathOverride)
        let nextDatabasePath = defaults.string(forKey: PreferenceKey.databasePathOverride)
        lastResolvedBinaryPath = nextBinaryPath
        lastResolvedDatabasePath = nextDatabasePath
        defaultRecentHours = defaults.clipmemDefaultHours
        defaultQueryMode = defaults.clipmemDefaultMode

        let nextHotkeyEnabled = defaults.clipmemHotkeyEnabled
        hotkeyEnabled = nextHotkeyEnabled
        if let openQuickRecallAction {
            configureHotkey(enabled: nextHotkeyEnabled, openQuickRecall: openQuickRecallAction)
        }

        applyLaunchAtLoginPreference()
        updateStatus = UpdateStatus.load()

        if previousBinaryPath != nextBinaryPath || previousDatabasePath != nextDatabasePath {
            adoptConfiguration(.current)
            await installSelfIgnoreIfNeeded()
            await refreshAll()
        } else if previousHotkeyEnabled != nextHotkeyEnabled || previousLaunchAtLoginEnabled != launchAtLoginEnabled || previousUpdateStatus != updateStatus {
            clipboardHistoryRevision += 1
        } else if previousDefaultRecentHours != defaultRecentHours || previousDefaultQueryMode != defaultQueryMode {
            clipboardHistoryRevision += 1
        }
    }

    private func applyLaunchAtLoginPreference() {
        let desired = UserDefaults.standard.clipmemLaunchAtLoginEnabled
        let status = LoginItemController.status()
        let isApplied = status == .enabled
        guard desired != isApplied else {
            launchAtLoginEnabled = desired
            launchAtLoginStatus = status
            launchAtLoginError = nil
            return
        }
        do {
            try LoginItemController.setEnabled(desired)
            launchAtLoginEnabled = desired
            launchAtLoginStatus = LoginItemController.status()
            launchAtLoginError = nil
        } catch {
            launchAtLoginStatus = LoginItemController.status()
            launchAtLoginEnabled = launchAtLoginStatus == .enabled
            launchAtLoginError = UserError(
                message: "Could not apply launch at login preference.",
                recovery: error.localizedDescription
            )
        }
    }
}
