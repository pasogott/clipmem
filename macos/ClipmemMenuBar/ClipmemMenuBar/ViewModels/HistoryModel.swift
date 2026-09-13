import Foundation
import Observation

typealias HistoryPage = (items: [ClipmemItem], nextCursor: String?)
typealias HistoryPageLoader = @MainActor (QueryMode, String, RetrievalFilterState, String?) async throws -> HistoryPage
typealias HistoryDetailLoader = @MainActor (Int) async throws -> SnapshotDetails

@MainActor
@Observable
final class HistoryModel {
    var searchStyle: SearchStyle
    var resultScope: HistoryResultScope
    var query = ""
    var filters = RetrievalFilterState.defaultValue
    var results: [ClipmemItem] = []
    var selectedID: Int? {
        didSet {
            guard selectedID != oldValue else { return }
            detailTask?.cancel()
            detailGeneration += 1
            selectedDetail = nil
            isLoadingDetail = false
        }
    }
    var selectedRowID: String?
    var selectedDetail: SnapshotDetails?
    var nextCursor: String?
    var isLoading = false
    var isLoadingDetail = false
    var error: UserError?

    let configurationGeneration: Int
    @ObservationIgnored private let appModel: AppModel
    @ObservationIgnored private let pageLoader: HistoryPageLoader?
    @ObservationIgnored private let detailLoader: HistoryDetailLoader
    @ObservationIgnored private var loadGeneration = 0
    @ObservationIgnored private var detailGeneration = 0
    @ObservationIgnored private var loadedPageKey: HistoryPageKey?
    @ObservationIgnored private var pageTask: Task<HistoryPage, Error>?
    @ObservationIgnored private var detailTask: Task<SnapshotDetails, Error>?

    init(
        mode: QueryMode = UserDefaults.standard.clipmemDefaultMode,
        appModel: AppModel,
        pageLoader: HistoryPageLoader? = nil,
        detailLoader: HistoryDetailLoader? = nil
    ) {
        let historyMode = mode.historyCompatibleMode
        let displayState = DisplayMode.from(queryMode: historyMode)
        searchStyle = displayState.searchStyle
        resultScope = HistoryResultScope.from(queryMode: historyMode)
        self.configurationGeneration = appModel.configurationGeneration
        self.appModel = appModel
        self.pageLoader = pageLoader
        self.detailLoader = detailLoader ?? { snapshotID in
            try await appModel.client.get(snapshotID: snapshotID).snapshot
        }
    }

    var selectedItem: ClipmemItem? {
        guard let selectedID else { return nil }
        if let selectedRowID, let item = results.first(where: { $0.id == selectedRowID }) {
            return item
        }
        return results.first { $0.snapshotId == selectedID }
    }

    var mode: QueryMode {
        resolvedMode
    }

    var displaysCopyEvents: Bool {
        loadedPageKey?.mode == .timeline
    }

    func reload(selecting snapshotID: Int? = nil) async {
        pageTask?.cancel()
        detailTask?.cancel()
        loadGeneration += 1
        let generation = loadGeneration
        nextCursor = nil
        loadedPageKey = nil
        selectedID = snapshotID
        selectedRowID = nil
        if selectedDetail?.snapshotId != snapshotID { selectedDetail = nil }
        await loadMore(generation: generation)
    }

    func loadMore() async {
        guard loadedPageKey == nil || loadedPageKey == currentPageKey else { return }
        loadGeneration += 1
        await loadMore(generation: loadGeneration)
    }

    func refreshForExternalHistoryChange() async {
        loadGeneration += 1
        let generation = loadGeneration
        let previousSelectedID = selectedID
        let previousSelectedRowID = selectedRowID
        let previousSelectedIndex = results.firstIndex { $0.id == previousSelectedRowID }
        let previousCount = results.count
        let request = HistoryRequest(
            generation: generation,
            mode: resolvedMode,
            query: query,
            filters: filters,
            cursor: nil
        )

        isLoading = true
        defer {
            if generation == loadGeneration {
                isLoading = false
            }
        }

        do {
            var page = try await loadPageOwned(request)
            guard isCurrent(request) else { return }
            var cursor = page.nextCursor
            while page.items.count < min(previousCount, 200), let nextCursor = cursor {
                let continuation = HistoryRequest(
                    generation: generation,
                    mode: request.mode,
                    query: request.query,
                    filters: request.filters,
                    cursor: nextCursor
                )
                let next = try await loadPageOwned(continuation)
                guard isCurrent(request) else { return }
                page.items.append(contentsOf: next.items)
                cursor = next.nextCursor
            }
            results = page.items
            nextCursor = cursor
            loadedPageKey = request.pageKey
            if let previousSelectedRowID, results.contains(where: { $0.id == previousSelectedRowID }) {
                selectedRowID = previousSelectedRowID
                selectedID = results.first { $0.id == previousSelectedRowID }?.snapshotId
            } else if let previousSelectedID, results.contains(where: { $0.snapshotId == previousSelectedID }) {
                selectedID = previousSelectedID
                selectedRowID = results.first { $0.snapshotId == previousSelectedID }?.id
            } else {
                let adjacentIndex = min(previousSelectedIndex ?? 0, max(results.count - 1, 0))
                selectedID = results.isEmpty ? nil : results[adjacentIndex].snapshotId
                selectedRowID = results.isEmpty ? nil : results[adjacentIndex].id
                selectedDetail = nil
            }
            error = nil
            await loadSelectedDetail()
        } catch is CancellationError {
        } catch {
            guard isCurrent(request) else { return }
            self.error = UserError(error)
        }
    }

    private func loadMore(generation: Int) async {
        guard nextCursor == nil || loadedPageKey == currentPageKey else { return }
        let request = HistoryRequest(
            generation: generation,
            mode: resolvedMode,
            query: query,
            filters: filters,
            cursor: nextCursor
        )
        isLoading = true
        defer {
            if generation == loadGeneration {
                isLoading = false
            }
        }
        do {
            let page = try await loadPageOwned(request)
            guard isCurrent(request) else { return }
            if request.cursor == nil {
                results = page.items
            } else {
                results.append(contentsOf: page.items)
            }
            nextCursor = page.nextCursor
            loadedPageKey = request.pageKey
            if selectedID == nil {
                selectedID = results.first?.snapshotId
                selectedRowID = results.first?.id
            } else if selectedRowID == nil {
                selectedRowID = results.first { $0.snapshotId == selectedID }?.id
            }
            if selectedID != nil, selectedDetail == nil {
                await loadSelectedDetail()
            }
            error = nil
        } catch is CancellationError {
        } catch {
            guard isCurrent(request) else { return }
            self.error = UserError(error)
        }
    }

    func loadSelectedDetail() async {
        detailTask?.cancel()
        detailGeneration += 1
        let generation = detailGeneration
        selectedDetail = nil
        guard let selectedID else {
            selectedDetail = nil
            return
        }
        isLoadingDetail = true
        defer {
            if generation == detailGeneration {
                isLoadingDetail = false
            }
        }
        do {
            let task = Task { try await detailLoader(selectedID) }
            detailTask = task
            let detail = try await task.value
            guard configurationGeneration == appModel.configurationGeneration, generation == detailGeneration, self.selectedID == selectedID else { return }
            selectedDetail = detail
            error = nil
        } catch is CancellationError {
        } catch {
            guard configurationGeneration == appModel.configurationGeneration, generation == detailGeneration, self.selectedID == selectedID else { return }
            self.error = UserError(error)
        }
    }

    func restoreSelected() async {
        guard configurationGeneration == appModel.configurationGeneration else { return }
        guard let selectedItem else { return }
        await appModel.restore(selectedItem)
    }

    func forget(snapshotID: Int) async {
        guard configurationGeneration == appModel.configurationGeneration else { return }
        let selectedIndex = results.firstIndex { $0.snapshotId == snapshotID } ?? 0
        guard await appModel.forget(snapshotID: snapshotID) else { return }
        results.removeAll { $0.snapshotId == snapshotID }
        guard selectedID == snapshotID else { return }
        let adjacentIndex = min(selectedIndex, max(results.count - 1, 0))
        self.selectedID = results.isEmpty ? nil : results[adjacentIndex].snapshotId
        selectedRowID = results.isEmpty ? nil : results[adjacentIndex].id
        await loadSelectedDetail()
    }

    func selectRow(id rowID: String?) {
        selectedRowID = rowID
        guard let rowID else {
            selectedID = nil
            selectedDetail = nil
            return
        }
        guard let item = results.first(where: { $0.id == rowID }) else { return }
        selectedID = item.snapshotId
    }

    private func loadPage(_ request: HistoryRequest) async throws -> HistoryPage {
        if let pageLoader {
            return try await pageLoader(request.mode, request.query, request.filters, request.cursor)
        }
        switch request.mode {
        case .recall:
            let envelope = try await appModel.client.recall(query: request.query.isEmpty ? nil : request.query, limit: 25, filters: request.filters)
            return ([envelope.bestCandidate] + envelope.alternatives, nil)
        case .search:
            let envelope = try await appModel.client.search(query: request.query, limit: 40, cursor: request.cursor, filters: request.filters)
            return (envelope.results, envelope.nextCursor)
        case .recent:
            let envelope = try await appModel.client.recent(limit: 40, cursor: request.cursor, filters: request.filters)
            return (envelope.results, envelope.nextCursor)
        case .timeline:
            let envelope = try await appModel.client.timeline(limit: 40, cursor: request.cursor, filters: request.filters)
            return (envelope.results, envelope.nextCursor)
        case .diagnostics:
            return ([], nil)
        }
    }

    private func loadPageOwned(_ request: HistoryRequest) async throws -> HistoryPage {
        pageTask?.cancel()
        let task = Task { try await loadPage(request) }
        pageTask = task
        return try await task.value
    }

    private func isCurrent(_ request: HistoryRequest) -> Bool {
        configurationGeneration == appModel.configurationGeneration
            && request.generation == loadGeneration
            && request.mode == resolvedMode
            && request.query == query
            && request.filters == filters
    }

    private var resolvedMode: QueryMode {
        query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            ? resultScope.queryMode
            : searchStyle.queryMode
    }

    private var currentPageKey: HistoryPageKey {
        HistoryPageKey(mode: resolvedMode, query: query, filters: filters)
    }
}

private struct HistoryRequest: Equatable, Sendable {
    var generation: Int
    var mode: QueryMode
    var query: String
    var filters: RetrievalFilterState
    var cursor: String?

    var pageKey: HistoryPageKey {
        HistoryPageKey(mode: mode, query: query, filters: filters)
    }
}

private struct HistoryPageKey: Equatable, Sendable {
    var mode: QueryMode
    var query: String
    var filters: RetrievalFilterState
}
