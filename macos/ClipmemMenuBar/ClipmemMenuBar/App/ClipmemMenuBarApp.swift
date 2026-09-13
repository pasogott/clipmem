import AppKit
import SwiftUI

@main
struct ClipmemMenuBarApp: App {
    @Environment(\.openWindow) private var openWindow
    @AppStorage(PreferenceKey.hotkeyEnabled) private var hotkeyEnabled = true
    @State private var appModel = AppModel()

    var body: some Scene {
        MenuBarExtra {
            MenuBarPanelView(appModel: appModel)
                .id(appModel.configurationGeneration)
                .frame(width: 400, height: 520)
        } label: {
            ClipmemMenuBarLabel(
                healthState: appModel.healthState,
                isUpdateAvailable: appModel.updateStatus.isUpdateAvailable
            )
                .task {
                    if await appModel.start() {
                        configureHotkey()
                    }
                }
                .onChange(of: hotkeyEnabled) {
                    configureHotkey()
                }
        }
        .menuBarExtraStyle(.window)

        WindowGroup("History", id: WindowID.history.rawValue) {
            HistoryWindowView(appModel: appModel)
                .id(appModel.configurationGeneration)
                .frame(minWidth: 880, idealWidth: 1160, minHeight: 600, idealHeight: 740)
                .modifier(WindowFrameLimiter(maxVisibleWidthInset: 48, maxVisibleHeightInset: 64))
        }
        .commands {
            InspectorCommands()
        }
        .keyboardShortcut("h", modifiers: [.command, .shift])
        .defaultSize(width: 1160, height: 740)
        .defaultPosition(.center)
        .windowResizability(.contentMinSize)

        Window("Quick Recall", id: WindowID.quickRecall.rawValue) {
            QuickRecallWindowView(appModel: appModel)
                .id(appModel.configurationGeneration)
                .frame(width: 720, height: 520)
        }
        .keyboardShortcut("v", modifiers: [.option, .shift])

        Settings {
            ClipmemSettingsView(appModel: appModel)
                .frame(width: 660, height: 560)
        }
    }

    @MainActor
    private func configureHotkey() {
        appModel.configureHotkey(enabled: hotkeyEnabled) {
            WindowActivation.openWindow(openWindow, id: .quickRecall)
        }
    }
}

private struct ClipmemMenuBarLabel: View {
    let healthState: HealthState
    let isUpdateAvailable: Bool

    var body: some View {
        ZStack(alignment: .bottomTrailing) {
            Image("ClipmemMenuBarIcon")
                .renderingMode(.template)
                .resizable()
                .scaledToFit()
                .frame(width: 18, height: 18)

            if let badgeSymbol = healthState.menuBarBadgeSymbol,
               let badgeTone = healthState.menuBarBadgeTone {
                MenuBarHealthBadge(symbol: badgeSymbol, tone: badgeTone)
                    .offset(x: 2, y: 1)
            }
        }
        .frame(width: 20, height: 18)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("clipmem")
        .accessibilityValue(Text(accessibilityValue))
        .help(accessibilityValue)
    }

    private var accessibilityValue: String {
        if isUpdateAvailable {
            return "\(healthState.title), update available"
        }
        return healthState.title
    }
}

private struct MenuBarHealthBadge: View {
    let symbol: String
    let tone: MenuBarBadgeTone

    var body: some View {
        Image(systemName: symbol)
            .font(.system(size: 6, weight: .black))
            .foregroundStyle(.white)
            .frame(width: 9, height: 9)
            .background(tone.tint, in: Circle())
            .overlay {
                Circle()
                    .stroke(.primary.opacity(0.18), lineWidth: 1)
            }
    }
}

private struct WindowFrameLimiter: ViewModifier {
    let maxVisibleWidthInset: CGFloat
    let maxVisibleHeightInset: CGFloat

    func body(content: Content) -> some View {
        content.background(WindowFrameLimiterView(widthInset: maxVisibleWidthInset, heightInset: maxVisibleHeightInset))
    }
}

private struct WindowFrameLimiterView: NSViewRepresentable {
    let widthInset: CGFloat
    let heightInset: CGFloat

    func makeCoordinator() -> Coordinator {
        Coordinator()
    }

    func makeNSView(context: Context) -> NSView {
        let view = NSView(frame: .zero)
        DispatchQueue.main.async {
            context.coordinator.limitWindowFrame(for: view, widthInset: widthInset, heightInset: heightInset)
        }
        return view
    }

    func updateNSView(_ nsView: NSView, context: Context) {
        DispatchQueue.main.async {
            context.coordinator.limitWindowFrame(for: nsView, widthInset: widthInset, heightInset: heightInset)
        }
    }

    @MainActor
    final class Coordinator {
        private var didLimitFrame = false

        func limitWindowFrame(for view: NSView, widthInset: CGFloat, heightInset: CGFloat) {
            guard didLimitFrame == false, let window = view.window else { return }
            didLimitFrame = true

            let visibleFrame = window.screen?.visibleFrame ?? NSScreen.main?.visibleFrame ?? .zero
            guard visibleFrame.width > 0, visibleFrame.height > 0 else { return }

            let maxSize = CGSize(
                width: max(880, visibleFrame.width - widthInset),
                height: max(600, visibleFrame.height - heightInset)
            )
            var frame = window.frame
            frame.size.width = min(frame.width, maxSize.width)
            frame.size.height = min(frame.height, maxSize.height)

            if frame.maxX > visibleFrame.maxX {
                frame.origin.x = visibleFrame.maxX - frame.width
            }
            if frame.minX < visibleFrame.minX {
                frame.origin.x = visibleFrame.minX
            }
            if frame.maxY > visibleFrame.maxY {
                frame.origin.y = visibleFrame.maxY - frame.height
            }
            if frame.minY < visibleFrame.minY {
                frame.origin.y = visibleFrame.minY
            }

            if frame != window.frame {
                window.setFrame(frame, display: true, animate: false)
            }
        }
    }
}
