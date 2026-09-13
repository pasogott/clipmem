@preconcurrency import AppKit
import SwiftUI

struct CommandClickableMarkdownText: View {
    let rendered: MarkdownRenderedText
    var lineLimit: Int?
    var truncationMode: Text.TruncationMode = .tail
    var selectionEnabled = false

    @ViewBuilder
    var body: some View {
        let baseText = Text(rendered.attributed)
            .lineLimit(lineLimit)
            .truncationMode(truncationMode)

        if rendered.links.isEmpty {
            selectableText(baseText)
        } else {
            let monitoredText = baseText.linkCommandClickMonitor(
                attributedString: rendered.attributed,
                links: rendered.links,
                lineLimit: lineLimit,
                truncationMode: truncationMode
            )
            selectableText(monitoredText)
        }
    }

    @ViewBuilder
    private func selectableText<Content: View>(_ text: Content) -> some View {
        if selectionEnabled {
            text.textSelection(.enabled)
        } else {
            text.textSelection(.disabled)
        }
    }
}

private extension View {
    func linkCommandClickMonitor(
        attributedString: AttributedString,
        links: [MarkdownRenderedLink],
        lineLimit: Int?,
        truncationMode: Text.TruncationMode
    ) -> some View {
        background {
            LinkCommandClickMonitor(
                attributedString: attributedString,
                links: links,
                lineLimit: lineLimit,
                truncationMode: truncationMode
            ) { target in
                PasteboardActions.openLinkTarget(target)
            }
            .allowsHitTesting(false)
        }
    }
}

private struct LinkCommandClickMonitor: NSViewRepresentable {
    let attributedString: AttributedString
    let links: [MarkdownRenderedLink]
    let lineLimit: Int?
    let truncationMode: Text.TruncationMode
    let onOpen: (LinkTargetResolution) -> Void

    func makeNSView(context: Context) -> LinkCommandClickMonitorView {
        let view = LinkCommandClickMonitorView()
        view.coordinator = context.coordinator
        return view
    }

    func updateNSView(_ nsView: LinkCommandClickMonitorView, context: Context) {
        context.coordinator.configure(
            attributedString: attributedString,
            links: links,
            lineLimit: lineLimit,
            lineBreakMode: truncationMode.nsLineBreakMode,
            onOpen: onOpen
        )
        nsView.coordinator = context.coordinator
    }

    func makeCoordinator() -> Coordinator {
        Coordinator()
    }

    @MainActor
    final class Coordinator {
        private var attributedString = NSAttributedString()
        private var links: [MarkdownRenderedLink] = []
        private var lineLimit: Int?
        private var lineBreakMode: NSLineBreakMode = .byTruncatingTail
        private var onOpen: ((LinkTargetResolution) -> Void)?

        func configure(
            attributedString: AttributedString,
            links: [MarkdownRenderedLink],
            lineLimit: Int?,
            lineBreakMode: NSLineBreakMode,
            onOpen: @escaping (LinkTargetResolution) -> Void
        ) {
            self.attributedString = NSAttributedString(attributedString)
            self.links = links
            self.lineLimit = lineLimit
            self.lineBreakMode = lineBreakMode
            self.onOpen = onOpen
        }

        func actionableTarget(at point: NSPoint, in bounds: NSRect) -> LinkTargetResolution? {
            guard bounds.contains(point) else { return nil }
            return MarkdownLinkHitTesting.actionableTarget(
                at: point,
                in: bounds.size,
                attributedString: attributedString,
                links: links,
                lineLimit: lineLimit,
                lineBreakMode: lineBreakMode
            )
        }

        func open(_ target: LinkTargetResolution) {
            onOpen?(target)
        }
    }
}

private final class LinkCommandClickMonitorView: NSView {
    override var isFlipped: Bool { true }
    weak var coordinator: LinkCommandClickMonitor.Coordinator?
    private weak var registeredWindow: NSWindow?
    private static var windowMonitors: [ObjectIdentifier: WindowLinkEventMonitor] = [:]

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        registerWithCurrentWindow()
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        registerWithCurrentWindow()
    }

    override func hitTest(_ point: NSPoint) -> NSView? {
        nil
    }

    override func viewWillMove(toWindow newWindow: NSWindow?) {
        super.viewWillMove(toWindow: newWindow)
        guard newWindow !== window else { return }
        unregisterFromCurrentWindow()
    }

    func actionableTarget(at point: NSPoint) -> LinkTargetResolution? {
        coordinator?.actionableTarget(at: point, in: bounds)
    }

    func open(_ target: LinkTargetResolution) {
        coordinator?.open(target)
    }

    private func registerWithCurrentWindow() {
        guard let window else { return }
        guard registeredWindow !== window else { return }
        unregisterFromCurrentWindow()

        let monitor = Self.monitor(for: window)
        monitor.register(self)
        registeredWindow = window
    }

    private func unregisterFromCurrentWindow() {
        guard let window = registeredWindow else { return }
        let identifier = ObjectIdentifier(window)
        Self.windowMonitors[identifier]?.unregister(self)
        if Self.windowMonitors[identifier]?.isEmpty == true {
            Self.windowMonitors[identifier]?.close()
            Self.windowMonitors[identifier] = nil
        }
        registeredWindow = nil
    }

    private static func monitor(for window: NSWindow) -> WindowLinkEventMonitor {
        let identifier = ObjectIdentifier(window)
        if let monitor = windowMonitors[identifier] {
            return monitor
        }
        let monitor = WindowLinkEventMonitor(window: window)
        windowMonitors[identifier] = monitor
        return monitor
    }

    @MainActor
    private final class WindowLinkEventMonitor {
        private weak var window: NSWindow?
        private var monitor: Any?
        private var views: [WeakMonitorView] = []
        private let previousAcceptsMouseMovedEvents: Bool
        private var isShowingPointingHand = false

        var isEmpty: Bool {
            cleanupViews()
            return views.isEmpty
        }

        init(window: NSWindow) {
            self.window = window
            previousAcceptsMouseMovedEvents = window.acceptsMouseMovedEvents
            window.acceptsMouseMovedEvents = true
            monitor = NSEvent.addLocalMonitorForEvents(matching: [.leftMouseDown, .mouseMoved, .flagsChanged]) {
                [weak self] event in
                nonisolated(unsafe) let event = event
                let shouldConsume = MainActor.assumeIsolated {
                    self?.shouldConsume(event) ?? false
                }
                return shouldConsume ? nil : event
            }
        }

        func close() {
            removeMonitor()
            restoreCursorIfNeeded()
            if let window {
                window.acceptsMouseMovedEvents = previousAcceptsMouseMovedEvents
            }
        }

        func register(_ view: LinkCommandClickMonitorView) {
            cleanupViews()
            guard views.contains(where: { $0.view === view }) == false else { return }
            views.append(WeakMonitorView(view))
        }

        func unregister(_ view: LinkCommandClickMonitorView) {
            views.removeAll { $0.view == nil || $0.view === view }
            restoreCursorIfNeeded()
        }

        private func shouldConsume(_ event: NSEvent) -> Bool {
            switch event.type {
            case .leftMouseDown:
                return handleClick(event)
            case .mouseMoved, .flagsChanged:
                updateCursor(for: event)
                return false
            default:
                return false
            }
        }

        private func handleClick(_ event: NSEvent) -> Bool {
            guard event.modifierFlags.contains(.command), event.window === window else {
                return false
            }
            guard let hit = hit(at: event.locationInWindow) else { return false }

            hit.view.open(hit.target)
            return true
        }

        private func updateCursor(for event: NSEvent) {
            guard
                let window,
                event.modifierFlags.contains(.command),
                event.window === window
            else {
                restoreCursorIfNeeded()
                return
            }

            let windowPoint = event.type == .flagsChanged
                ? window.mouseLocationOutsideOfEventStream
                : event.locationInWindow
            guard hit(at: windowPoint) != nil else {
                restoreCursorIfNeeded()
                return
            }

            guard isShowingPointingHand == false else { return }
            NSCursor.pointingHand.push()
            isShowingPointingHand = true
        }

        private func restoreCursorIfNeeded() {
            guard isShowingPointingHand else { return }
            NSCursor.pop()
            isShowingPointingHand = false
        }

        private func hit(
            at windowPoint: NSPoint
        ) -> (view: LinkCommandClickMonitorView, target: LinkTargetResolution)? {
            cleanupViews()

            for weakView in views.reversed() {
                guard
                    let view = weakView.view,
                    view.window === window,
                    view.isHidden == false
                else {
                    continue
                }

                let point = view.convert(windowPoint, from: nil)
                guard let target = view.actionableTarget(at: point) else { continue }
                return (view, target)
            }

            return nil
        }

        private func cleanupViews() {
            views.removeAll { $0.view == nil }
        }

        private func removeMonitor() {
            if let monitor {
                NSEvent.removeMonitor(monitor)
                self.monitor = nil
            }
        }
    }

    private final class WeakMonitorView {
        weak var view: LinkCommandClickMonitorView?

        init(_ view: LinkCommandClickMonitorView) {
            self.view = view
        }
    }
}

enum MarkdownLinkHitTesting {
    static func actionableTarget(
        at point: NSPoint,
        in size: NSSize,
        attributedString: NSAttributedString,
        links: [MarkdownRenderedLink],
        lineLimit: Int?,
        lineBreakMode: NSLineBreakMode
    ) -> LinkTargetResolution? {
        guard let link = link(
            at: point,
            in: size,
            attributedString: attributedString,
            links: links,
            lineLimit: lineLimit,
            lineBreakMode: lineBreakMode
        ) else {
            return nil
        }

        let target = LinkTargetResolver.classify(link.target)
        return target == .unsupported ? nil : target
    }

    static func link(
        at point: NSPoint,
        in size: NSSize,
        attributedString: NSAttributedString,
        links: [MarkdownRenderedLink],
        lineLimit: Int?,
        lineBreakMode: NSLineBreakMode
    ) -> MarkdownRenderedLink? {
        guard size.width > 0, size.height > 0 else { return nil }

        let textStorage = NSTextStorage(attributedString: attributedString)
        let layoutManager = NSLayoutManager()
        let textContainer = NSTextContainer(size: NSSize(width: size.width, height: CGFloat.greatestFiniteMagnitude))

        textContainer.lineFragmentPadding = 0
        textContainer.maximumNumberOfLines = lineLimit ?? 0
        textContainer.lineBreakMode = lineBreakMode
        textStorage.addLayoutManager(layoutManager)
        layoutManager.addTextContainer(textContainer)
        layoutManager.ensureLayout(for: textContainer)

        let usedRect = layoutManager.usedRect(for: textContainer)
        guard usedRect.contains(point) else { return nil }

        let pointInText = NSPoint(x: point.x - usedRect.minX, y: point.y - usedRect.minY)
        guard pointInText.x >= 0, pointInText.y >= 0 else { return nil }

        let glyphIndex = layoutManager.glyphIndex(for: pointInText, in: textContainer)
        let glyphRect = layoutManager
            .boundingRect(forGlyphRange: NSRange(location: glyphIndex, length: 1), in: textContainer)
            .offsetBy(dx: -usedRect.minX, dy: -usedRect.minY)
        guard glyphRect.contains(pointInText) else { return nil }

        let characterIndex = layoutManager.characterIndexForGlyph(at: glyphIndex)
        guard characterIndex < attributedString.string.utf16.count else { return nil }
        return links.first { NSLocationInRange(characterIndex, $0.range) }
    }
}

private extension Text.TruncationMode {
    var nsLineBreakMode: NSLineBreakMode {
        switch self {
        case .head:
            .byTruncatingHead
        case .middle:
            .byTruncatingMiddle
        case .tail:
            .byTruncatingTail
        @unknown default:
            .byTruncatingTail
        }
    }
}
