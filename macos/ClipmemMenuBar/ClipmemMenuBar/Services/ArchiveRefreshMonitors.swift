import AppKit
import CoreFoundation
import Foundation

final class AppRefreshNotificationMonitor: @unchecked Sendable {
    private static let notificationRawName = "io.openclaw.clipmem.revision.changed"
    private static let notificationName = CFNotificationName(notificationRawName as CFString)

    private let onRefresh: @MainActor @Sendable () -> Void
    private var isStarted = false

    init(onRefresh: @escaping @MainActor @Sendable () -> Void) {
        self.onRefresh = onRefresh
    }

    deinit {
        stop()
    }

    func start() {
        guard isStarted == false else { return }
        isStarted = true
        let center = CFNotificationCenterGetDarwinNotifyCenter()
        CFNotificationCenterAddObserver(
            center,
            Unmanaged.passUnretained(self).toOpaque(),
            { _, observer, _, _, _ in
                guard let observer else { return }
                let monitor = Unmanaged<AppRefreshNotificationMonitor>.fromOpaque(observer).takeUnretainedValue()
                Task { @MainActor in
                    monitor.onRefresh()
                }
            },
            AppRefreshNotificationMonitor.notificationName.rawValue,
            nil,
            .deliverImmediately
        )
    }

    func stop() {
        guard isStarted else { return }
        let center = CFNotificationCenterGetDarwinNotifyCenter()
        CFNotificationCenterRemoveObserver(
            center,
            Unmanaged.passUnretained(self).toOpaque(),
            AppRefreshNotificationMonitor.notificationName,
            nil
        )
        isStarted = false
    }
}

@MainActor
final class PasteboardChangeMonitor {
    static let defaultPollInterval: Duration = .milliseconds(250)

    private let pollInterval: Duration
    private let changeCount: @MainActor () -> Int
    private let onChange: @MainActor () -> Void
    private var task: Task<Void, Never>?
    private var lastChangeCount: Int?

    init(
        pollInterval: Duration = PasteboardChangeMonitor.defaultPollInterval,
        changeCount: @escaping @MainActor () -> Int = { NSPasteboard.general.changeCount },
        onChange: @escaping @MainActor () -> Void
    ) {
        self.pollInterval = pollInterval
        self.changeCount = changeCount
        self.onChange = onChange
    }

    deinit {
        task?.cancel()
    }

    func start() {
        guard task == nil else { return }
        lastChangeCount = changeCount()
        task = Task { [weak self] in
            while Task.isCancelled == false {
                guard let self else { return }
                try? await Task.sleep(for: self.pollInterval)
                guard Task.isCancelled == false else { return }
                self.pollOnce()
            }
        }
    }

    func stop() {
        task?.cancel()
        task = nil
    }

    func pollOnce() {
        let currentChangeCount = changeCount()
        guard let lastChangeCount else {
            self.lastChangeCount = currentChangeCount
            return
        }
        guard currentChangeCount != lastChangeCount else { return }
        self.lastChangeCount = currentChangeCount
        onChange()
    }

    func markCurrentChangeHandled() {
        lastChangeCount = changeCount()
    }
}

@MainActor
final class RecentPreviewRefreshCoordinator {
    static let defaultDebounce: Duration = .milliseconds(550)

    private let debounce: Duration
    private let sleep: @MainActor (Duration) async throws -> Void
    private let refresh: @MainActor () async -> Bool
    private var pendingTask: Task<Void, Never>?
    private var isRefreshing = false
    private var needsFollowUp = false

    init(
        debounce: Duration = RecentPreviewRefreshCoordinator.defaultDebounce,
        sleep: @escaping @MainActor (Duration) async throws -> Void = { try await Task.sleep(for: $0) },
        refresh: @escaping @MainActor () async -> Bool
    ) {
        self.debounce = debounce
        self.sleep = sleep
        self.refresh = refresh
    }

    deinit {
        pendingTask?.cancel()
    }

    func schedule() {
        pendingTask?.cancel()
        pendingTask = Task { [weak self] in
            guard let self else { return }
            do {
                try await sleep(debounce)
            } catch {
                return
            }
            guard Task.isCancelled == false else { return }
            await runRefresh(queueFollowUpIfBusy: true)
        }
    }

    func refreshNow() async {
        pendingTask?.cancel()
        pendingTask = nil
        await runRefresh(queueFollowUpIfBusy: false)
    }

    private func runRefresh(queueFollowUpIfBusy: Bool) async {
        if isRefreshing {
            if queueFollowUpIfBusy {
                needsFollowUp = true
            }
            return
        }

        isRefreshing = true
        _ = await refresh()
        isRefreshing = false

        if needsFollowUp {
            needsFollowUp = false
            schedule()
        }
    }
}
