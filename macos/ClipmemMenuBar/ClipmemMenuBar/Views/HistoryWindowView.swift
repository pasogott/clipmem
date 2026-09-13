import SwiftUI

struct HistoryWindowView: View {
    let appModel: AppModel

    @Environment(\.openWindow) private var openWindow
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var history: HistoryModel
    @SceneStorage("history.mode") private var storedMode = ""
    @SceneStorage("history.searchStyle") private var storedSearchStyle = ""
    @SceneStorage("history.resultScope") private var storedResultScope = ""
    @SceneStorage("history.query") private var storedQuery = ""
    @SceneStorage("history.selected") private var storedSelectedID = 0
    @State private var handledHistoryOpenRequestID = 0

    init(appModel: AppModel) {
        self.appModel = appModel
        _history = State(initialValue: HistoryModel(appModel: appModel))
    }

    var body: some View {
        NavigationSplitView {
            sidebar
        } content: {
            contentColumn
        } detail: {
            detailColumn
        }
        .navigationTitle("History")
        .toolbar {
            ToolbarItem(placement: .principal) {
                HStack(spacing: Spacing.sm) {
                    Text("History")
                        .font(DesignType.bodySecondary.weight(.medium))
                    if !history.results.isEmpty {
                        Text("\u{2014} \(history.results.count) item\(history.results.count == 1 ? "" : "s")")
                            .font(DesignType.bodySecondary)
                            .monospacedDigit()
                            .foregroundStyle(.secondary)
                    }
                }
            }
            ToolbarItemGroup {
                Button("Refresh", systemImage: "arrow.clockwise") {
                    Task { await history.reload() }
                }
                .keyboardShortcut("r", modifiers: .command)
                Button("Quick Recall", systemImage: "sparkle.magnifyingglass") {
                    WindowActivation.openWindow(openWindow, id: .quickRecall)
                }
            }
        }
        .overlay(alignment: .top) {
            ActionFeedbackOverlay(message: appModel.actionMessage)
                .padding(.top, Spacing.sm)
        }
        .navigationSplitViewStyle(.balanced)
        .task {
            restoreSceneState()
            if await applyPendingHistoryOpenRequestIfNeeded() == false {
                await history.reload()
            }
        }
        .onChange(of: appModel.pendingHistoryOpenRequest?.id) {
            Task {
                await applyPendingHistoryOpenRequestIfNeeded()
            }
        }
        .onChange(of: history.query) {
            storedQuery = history.query
        }
        .onChange(of: history.selectedID) {
            storedSelectedID = history.selectedID ?? 0
            Task { await history.loadSelectedDetail() }
        }
        .onChange(of: appModel.clipboardHistoryRevision) {
            Task { await history.refreshForExternalHistoryChange() }
        }
    }

    // MARK: - Sidebar

    private var sidebar: some View {
        List {
            if let status = appModel.serviceStatus {
                Section("Statistics") {
                    LabeledContent("Database", value: DisplayFormatters.byteCount(status.dbSizeBytes) ?? "\u{2014}")
                        .font(DesignType.rowMeta)
                    if let retention = status.retention {
                        LabeledContent("Retention", value: retention)
                            .font(DesignType.rowMeta)
                    }
                }
            }
        }
        .navigationTitle("clipmem")
        .navigationSplitViewColumnWidth(min: 150, ideal: 200, max: 220)
        .safeAreaInset(edge: .bottom) {
            sidebarStatusIndicator
                .padding(.horizontal, Spacing.md)
                .padding(.vertical, Spacing.sm)
        }
    }

    private var sidebarStatusIndicator: some View {
        HStack(spacing: Spacing.sm) {
            Circle()
                .fill(appModel.healthState.tint)
                .frame(width: 8, height: 8)
            Text(appModel.healthState.title)
                .font(DesignType.rowMeta)
                .foregroundStyle(.secondary)
                .lineLimit(1)
        }
    }

    // MARK: - Content Column

    private var contentColumn: some View {
        VStack(spacing: 0) {
            queryControls
                .padding()
            Divider()
            resultList
        }
        .navigationTitle("History")
        .navigationSplitViewColumnWidth(min: 320, ideal: 420, max: 560)
    }

    private var detailColumn: some View {
        SnapshotDetailView(
            detail: history.selectedDetail,
            fallback: history.selectedItem,
            appModel: appModel,
            configurationGeneration: history.configurationGeneration,
            isLoading: history.isLoadingDetail,
            onForgot: { snapshotID in await history.forget(snapshotID: snapshotID) }
        )
            .navigationTitle("History")
            .navigationSplitViewColumnWidth(min: 360, ideal: 580)
    }

    // MARK: - Query Controls

    private var queryControls: some View {
        VStack(spacing: Spacing.md) {
            HStack(spacing: Spacing.sm) {
                TextField(searchPrompt, text: $history.query)
                    .textFieldStyle(.roundedBorder)
                    .onSubmit {
                        Task { await reloadAndStoreMode() }
                    }
                Picker("Style", selection: $history.searchStyle) {
                    Text("Smart").tag(SearchStyle.smart)
                    Text("Exact").tag(SearchStyle.exact)
                }
                .pickerStyle(.segmented)
                .fixedSize()
                .controlSize(.small)
                .onChange(of: history.searchStyle) {
                    Task { await reloadAndStoreMode() }
                }
                Button("Search", systemImage: "magnifyingglass") {
                    Task { await reloadAndStoreMode() }
                }
            }
            .padding(Spacing.md)
            .background(.quaternary.opacity(0.5), in: .rect(cornerRadius: DesignRadius.md))

            Picker("Results", selection: $history.resultScope) {
                ForEach(HistoryResultScope.allCases) { scope in
                    Text(scope.title).tag(scope)
                }
            }
            .pickerStyle(.segmented)
            .disabled(isSearching)
            .help(isSearching ? "Result organization applies when browsing without search text." : history.resultScope.help)
            .onChange(of: history.resultScope) {
                Task { await reloadAndStoreMode() }
            }

            FilterBar(history: history)
        }
    }

    private var searchPrompt: String {
        history.searchStyle == .smart ? "Describe what you want to find" : "Search exact text"
    }

    private var isSearching: Bool {
        history.query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty == false
    }

    private var selectedRowBinding: Binding<String?> {
        Binding {
            history.selectedRowID
        } set: { rowID in
            history.selectRow(id: rowID)
        }
    }

    // MARK: - Results

    private var resultList: some View {
        VStack(spacing: 0) {
            if let error = history.error ?? appModel.lastError {
                ErrorBanner(
                    message: error.message,
                    recovery: error.recovery,
                    onRetry: { Task { await history.reload() } }
                )
                .padding()
            }
            List(selection: selectedRowBinding) {
                ForEach(Array(history.results.enumerated()), id: \.element.id) { index, item in
                    ResultRowView(
                        item: item,
                        selected: item.id == history.selectedRowID,
                        presentation: history.displaysCopyEvents ? .copyEvent : .uniqueSnapshot
                    )
                        .tag(item.id)
                        .animation(DesignAnimation.staggerDelay(index: index, reduceMotion: reduceMotion), value: history.results.count)
                        .onAppear {
                            if item.id == history.results.last?.id,
                               history.nextCursor != nil {
                                Task { await history.loadMore() }
                            }
                        }
                }
                if history.nextCursor != nil {
                    Button("Load More", systemImage: "arrow.down.circle") {
                        Task { await history.loadMore() }
                    }
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, Spacing.sm)
                }
            }
            .listStyle(.inset)
            .overlay {
                if !history.isLoading && history.results.isEmpty && history.error == nil {
                    EmptyStateView(
                        title: history.query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? "No history yet" : "No results",
                        detail: history.query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                            ? "Start copying to build your clipboard history, or check Diagnostics when capture looks stale."
                            : "Try different search text or loosen your filters.",
                        symbol: history.query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? "clock" : "magnifyingglass"
                    )
                }
            }
            if history.isLoading {
                ProgressView()
                    .padding(Spacing.sm)
            }
        }
    }

    // MARK: - State Management

    private func restoreSceneState() {
        let restoredMode = storedMode.isEmpty ? UserDefaults.standard.clipmemDefaultMode.rawValue : storedMode
        let queryMode = (QueryMode(rawValue: restoredMode) ?? .recent).historyCompatibleMode
        let displayState = DisplayMode.from(queryMode: queryMode)
        history.searchStyle = SearchStyle(rawValue: storedSearchStyle) ?? displayState.searchStyle
        history.resultScope = HistoryResultScope(rawValue: storedResultScope) ?? HistoryResultScope.from(queryMode: queryMode)
        storedMode = queryMode.rawValue
        storePresentationState()
        history.query = storedQuery
        history.selectedID = storedSelectedID == 0 ? nil : storedSelectedID
    }

    @discardableResult
    private func applyPendingHistoryOpenRequestIfNeeded() async -> Bool {
        guard let request = appModel.pendingHistoryOpenRequest else { return false }
        guard request.id != handledHistoryOpenRequestID else { return false }

        handledHistoryOpenRequestID = request.id

        let queryMode = request.mode.historyCompatibleMode
        let displayState = DisplayMode.from(queryMode: queryMode)
        history.searchStyle = displayState.searchStyle
        history.resultScope = HistoryResultScope.from(queryMode: queryMode)

        history.query = request.query
        storedMode = history.mode.rawValue
        storePresentationState()
        storedQuery = request.query
        storedSelectedID = request.focusedSnapshotID ?? 0
        await history.reload(selecting: request.focusedSnapshotID)
        storedMode = history.mode.rawValue
        storePresentationState()
        return true
    }

    @MainActor
    private func reloadAndStoreMode() async {
        await history.reload()
        storedMode = history.mode.rawValue
        storePresentationState()
    }

    private func storePresentationState() {
        storedSearchStyle = history.searchStyle.rawValue
        storedResultScope = history.resultScope.rawValue
    }
}
