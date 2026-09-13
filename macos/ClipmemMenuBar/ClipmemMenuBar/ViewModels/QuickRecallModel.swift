import Foundation
import Observation

@MainActor
@Observable
final class QuickRecallModel {
    var mode: QueryMode = .recall { didSet { if mode != oldValue { invalidateRequest() } } }
    var query = "" { didSet { if query != oldValue { invalidateRequest() } } }
    var results: [ClipmemItem] = []
    var selectedID: Int?
    var selectedRowID: String?
    var isLoading = false
    var error: UserError?

    let configurationGeneration: Int
    @ObservationIgnored private let appModel: AppModel
    @ObservationIgnored private var searchTask: Task<Void, Never>?
    @ObservationIgnored private var requestTask: Task<[ClipmemItem], Error>?
    @ObservationIgnored private var requestGeneration = 0
    @ObservationIgnored private let resultLoader: (@MainActor (QueryMode, String) async throws -> [ClipmemItem])?
    @ObservationIgnored private let forgetItem: @MainActor (ClipmemItem) async -> Bool

    init(appModel: AppModel, resultLoader: (@MainActor (QueryMode, String) async throws -> [ClipmemItem])? = nil, forgetItem: (@MainActor (ClipmemItem) async -> Bool)? = nil) {
        self.resultLoader = resultLoader
        self.configurationGeneration = appModel.configurationGeneration
        self.appModel = appModel
        self.forgetItem = forgetItem ?? { item in
            await appModel.forget(item)
        }
    }

    var selectedItem: ClipmemItem? {
        if let selectedRowID { return results.first { $0.id == selectedRowID } }
        guard let selectedID else { return nil }
        return results.first { $0.snapshotId == selectedID }
    }

    func queryChanged() {
        searchTask?.cancel()
        searchTask = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(180))
            guard Task.isCancelled == false else { return }
            await self?.refresh()
        }
    }

    private func invalidateRequest() {
        requestTask?.cancel()
        requestGeneration += 1
        results = []
        selectedID = nil
        selectedRowID = nil
        isLoading = false
    }

    func refresh() async {
        invalidateRequest()
        let generation = requestGeneration
        let mode = mode
        let query = query
        isLoading = true
        defer { if generation == requestGeneration { isLoading = false } }
        let task = Task { try await loadResults(mode: mode, query: query) }
        requestTask = task
        do {
            let newResults = try await task.value
            guard !Task.isCancelled, generation == requestGeneration,
                  configurationGeneration == appModel.configurationGeneration else { return }
            results = newResults
            selectedID = results.first?.snapshotId
            selectedRowID = results.first?.id
            error = nil
        } catch is CancellationError {
        } catch {
            guard generation == requestGeneration, configurationGeneration == appModel.configurationGeneration else { return }
            self.error = UserError(error)
        }
    }

    private func loadResults(mode: QueryMode, query: String) async throws -> [ClipmemItem] {
        if let resultLoader { return try await resultLoader(mode, query) }
        let filters = RetrievalFilterState.defaultValue
        switch mode {
        case .recall:
            let envelope = try await appModel.client.recall(query: query.isEmpty ? nil : query, limit: 12, filters: filters)
            return [envelope.bestCandidate] + envelope.alternatives
        case .search:
            guard !query.isEmpty else { return [] }
            return try await appModel.client.search(query: query, limit: 20, cursor: nil, filters: filters).results
        case .recent:
            return try await appModel.client.recent(limit: 20, cursor: nil, filters: filters).results
        case .timeline:
            return try await appModel.client.timeline(limit: 20, cursor: nil, filters: filters).results
        case .diagnostics:
            return []
        }
    }

    func moveSelection(_ delta: Int) {
        guard results.isEmpty == false else { return }
        let current = selectedItem.flatMap { selected in results.firstIndex { $0.id == selected.id } } ?? 0
        let next = min(max(current + delta, 0), results.count - 1)
        selectRow(id: results[next].id)
    }

    func selectRow(id: String?) {
        selectedRowID = id
        selectedID = results.first { $0.id == id }?.snapshotId
    }

    func restoreSelected() async {
        guard configurationGeneration == appModel.configurationGeneration else { return }
        guard let selectedItem else { return }
        await restore(selectedItem)
    }

    func restore(_ item: ClipmemItem) async {
        guard configurationGeneration == appModel.configurationGeneration else { return }
        await appModel.restore(item)
    }

    func forgetSelected() async {
        guard let selectedItem else { return }
        await forget(selectedItem)
    }

    func forget(_ item: ClipmemItem) async {
        guard configurationGeneration == appModel.configurationGeneration else { return }
        guard await forgetItem(item) else { return }
        results.removeAll { $0.snapshotId == item.snapshotId }
        if selectedID == item.snapshotId {
            selectedID = results.first?.snapshotId
            selectedRowID = results.first?.id
        }
    }
}
