import CodaGhosttyKit
import CodaKit
import Combine
import SwiftUI
import UniformTypeIdentifiers

// MARK: - Full-Screen Mobile / Right-Pane iPad & Mac Terminal View (`herdr.dev/mobile/`)

private final class TerminalViewportLocalState: ObservableObject {
    @Published var directInputBuffer: String = ""
    @Published var showDirectInputBar: Bool = false
    @Published var baseFontScale: CGFloat = 1.0
    @Published var atBottom: Bool = true
    var lastHandledRevision: UInt64 = 0
    var suppressDisappearUntil: Date = .distantPast
}

public struct CodaTerminalSessionView: View {
    @ObservedObject public var store: CodaWorkspaceStore
    public let session: CodaAgentSession
    public let isCompact: Bool
    public let isSplitThumb: Bool
    public let isSecondaryLeaf: Bool
    public let isCompactHeight: Bool
    public let hideBottomControls: Bool

    @FocusState private var directKeyboardFocused: Bool
    @StateObject private var local = TerminalViewportLocalState()

    public init(
        store: CodaWorkspaceStore,
        session: CodaAgentSession,
        isCompact: Bool,
        isSplitThumb: Bool = false,
        isSecondaryLeaf: Bool = false,
        isCompactHeight: Bool = false,
        hideBottomControls: Bool = false
    ) {
        self.store = store
        self.session = session
        self.isCompact = isCompact
        self.isSplitThumb = isSplitThumb
        self.isSecondaryLeaf = isSecondaryLeaf
        self.isCompactHeight = isCompactHeight
        self.hideBottomControls = hideBottomControls
    }

    public var body: some View {
        VStack(spacing: 0) {
            topHeaderBar
                .contentShape(Rectangle())
                .gesture(
                    DragGesture(minimumDistance: 28)
                        .onEnded { value in
                            if abs(value.translation.width) > abs(value.translation.height) * 1.3 {
                                store.switchSibling(
                                    delta: value.translation.width < 0 ? 1 : -1,
                                    for: session,
                                    isSecondary: isSecondaryLeaf
                                )
                            }
                        }
                )

            Divider()
                .background(CodaHerdrTheme.borderHairline)

            if session.state == .needsYou {
                blockedActionBanner
                    .padding(.horizontal, 12)
                    .padding(.top, 8)
            }

            ZStack(alignment: .bottom) {
                terminalScrollViewport
                    .simultaneousGesture(
                        MagnificationGesture()
                            .onChanged { value in
                                let scaled = max(9.5, min(20.0, store.terminalFontSize * value / local.baseFontScale))
                                store.terminalFontSize = scaled
                                local.baseFontScale = value
                            }
                            .onEnded { _ in
                                local.baseFontScale = 1.0
                                store.recomputeTerminalGridForFontChange()
                            }
                    )

                if !hideBottomControls {
                    VStack(spacing: 8) {
                        if local.showDirectInputBar || store.showTactileTwoRowKeypad {
                            quickCommandsStrip
                                .transition(.move(edge: .bottom).combined(with: .opacity))
                        }

                        if local.showDirectInputBar {
                            directKeystrokeStrip
                                .padding(.horizontal, 12)
                                .transition(.move(edge: .bottom).combined(with: .opacity))
                        }

                        if store.dpadVisible {
                            dpadRow
                                .transition(.move(edge: .bottom).combined(with: .opacity))
                        }

                        if store.showTactileTwoRowKeypad {
                            CodaDuoTactileTwoRowKeypad(store: store, targetSession: session)
                                .transition(.move(edge: .bottom).combined(with: .opacity))
                        }

                        floatingQuickDock
                    }
                    .padding(.horizontal, 12)
                    .padding(.bottom, (isCompact || isCompactHeight) ? 8 : 14)
                }
            }
        }
        .background(CodaHerdrTheme.canvasBG)
        .sheet(
            isPresented: Binding(
                get: { !isSecondaryLeaf && store.showComposeSheet },
                set: { if !isSecondaryLeaf { store.showComposeSheet = $0 } }
            ),
            onDismiss: {
                if store.pendingOpenSettingsAfterCompose {
                    store.pendingOpenSettingsAfterCompose = false
                    store.settingsInitialFocusPairing = false
                    store.showSettingsSheet = true
                }
            }
        ) {
            CodaComposePromptSheet(store: store, session: store.activeSheetSession ?? session)
        }
        .sheet(isPresented: Binding(
            get: { !isSecondaryLeaf && store.showPaneMinimapSheet },
            set: { if !isSecondaryLeaf { store.showPaneMinimapSheet = $0 } }
        )) {
            CodaPaneMinimapSheet(store: store, session: store.activeSheetSession ?? session)
        }
        .sheet(isPresented: Binding(
            get: { !isSecondaryLeaf && store.showGitDiffSheet },
            set: { if !isSecondaryLeaf { store.showGitDiffSheet = $0 } }
        )) {
            CodaGitDiffSheet(store: store, session: store.activeSheetSession ?? session)
        }
    }

    // MARK: - Top Header Bar

    private var topHeaderBar: some View {
        HStack(spacing: 10) {
            if isCompact {
                Button {
                    store.closeTerminalToInbox()
                } label: {
                    ZStack(alignment: .topTrailing) {
                        HStack(spacing: 5) {
                            Image(systemName: "chevron.left")
                                .font(.system(size: 13, weight: .semibold))
                            Image(systemName: "tray.fill")
                                .font(.system(size: 13))
                        }
                        .foregroundStyle(CodaHerdrTheme.textPrimary)
                        .padding(.horizontal, 10)
                        .padding(.vertical, 7)
                        .background(
                            Capsule()
                                .fill(CodaHerdrTheme.elevatedBG)
                                .overlay(Capsule().strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
                        )

                        if store.attentionBadgeCount > 0 {
                            Circle()
                                .fill(CodaHerdrTheme.redDiff)
                                .frame(width: 8, height: 8)
                                .offset(x: 2, y: -2)
                        }
                    }
                }
                .buttonStyle(.plain)
            } else if store.formFactor == .iphoneDuoInnerLandscape
                || (store.formFactor == .iphoneDuoInnerPortrait && store.foldPosture == .tabletopLaptop)
            {
                let badgeTitle = isSplitThumb ? "1+2" : (isSecondaryLeaf ? "L2" : "L1")
                let badgeTint = isSecondaryLeaf ? CodaHerdrTheme.amberNeedsYou : CodaHerdrTheme.cyanWorking
                Text(badgeTitle)
                    .font(.system(size: 10.5, weight: .bold, design: .monospaced))
                    .foregroundStyle(badgeTint)
                    .frame(width: 32, height: 32)
                    .background(
                        RoundedRectangle(cornerRadius: 8, style: .continuous)
                            .fill(CodaHerdrTheme.elevatedBG)
                            .overlay(
                                RoundedRectangle(cornerRadius: 8, style: .continuous)
                                    .strokeBorder(badgeTint.opacity(0.45), lineWidth: 1)
                            )
                    )
            } else {
                Button {
                    withAnimation(.snappy(duration: 0.2)) {
                        store.showSidebar.toggle()
                    }
                } label: {
                    Image(systemName: "sidebar.left")
                        .font(.system(size: 15, weight: .medium))
                        .foregroundStyle(CodaHerdrTheme.textPrimary)
                        .frame(width: 32, height: 32)
                        .background(
                            RoundedRectangle(cornerRadius: 8, style: .continuous)
                                .fill(CodaHerdrTheme.elevatedBG)
                                .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
                        )
                }
                .buttonStyle(.plain)
                .keyboardShortcut("b", modifiers: [.command])
            }

            Spacer(minLength: 4)

            VStack(spacing: 2) {
                HStack(spacing: 6) {
                    Text(session.taskTitle)
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundStyle(CodaHerdrTheme.textPrimary)
                        .lineLimit(1)
                        .truncationMode(.tail)
                }

                HStack(spacing: 6) {
                    let siblings = store.siblingSessions(for: session)
                    Text("\(session.workspaceName) · \(session.branch) · \(session.hostName)")
                        .font(.system(size: 11))
                        .foregroundStyle(CodaHerdrTheme.textSecondary)
                        .lineLimit(1)

                    if siblings.count > 1 {
                        HStack(spacing: 4) {
                            ForEach(siblings) { sib in
                                Circle()
                                    .fill(sib.id == session.id ? CodaHerdrTheme.textPrimary : CodaHerdrTheme.textDim)
                                    .frame(width: 5, height: 5)
                            }
                        }
                    }
                }
            }

            Spacer(minLength: 4)

            HStack(spacing: 8) {
                let siblings = store.siblingSessions(for: session)
                let duoDiffActive = !isSecondaryLeaf && store.formFactor == .iphoneDuoInnerLandscape && store.duoSplitMode == .studioDiffSplit
                Button {
                    if !isSecondaryLeaf && store.formFactor == .iphoneDuoInnerLandscape {
                        withAnimation(.snappy(duration: 0.18)) {
                            if store.duoSplitMode == .studioDiffSplit {
                                store.setDuoSplitMode(.inboxAndTerminal)
                            } else {
                                store.setDuoSplitMode(.studioDiffSplit)
                                store.loadGitDiff(for: session, presentModal: false)
                            }
                        }
                    } else {
                        store.loadGitDiff(for: session, presentModal: true)
                    }
                } label: {
                    Image(systemName: "plus.forwardslash.minus")
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundStyle(
                            duoDiffActive
                                ? CodaHerdrTheme.cyanWorking
                                : ((session.diffAdded > 0 || session.diffDeleted > 0) ? CodaHerdrTheme.greenDiff : CodaHerdrTheme.textSecondary)
                        )
                        .frame(width: 32, height: 32)
                        .background(
                            RoundedRectangle(cornerRadius: 8, style: .continuous)
                                .fill(duoDiffActive ? CodaHerdrTheme.cyanWorking.opacity(0.14) : CodaHerdrTheme.elevatedBG)
                                .overlay(
                                    RoundedRectangle(cornerRadius: 8, style: .continuous)
                                        .strokeBorder(
                                            duoDiffActive ? CodaHerdrTheme.cyanWorking.opacity(0.55) : CodaHerdrTheme.borderHairline,
                                            lineWidth: 1
                                        )
                                )
                        )
                }
                .buttonStyle(.plain)

                Button {
                    store.openMinimapSheet(for: session, isSecondary: isSecondaryLeaf)
                } label: {
                    CodaMiniPaneMapGlyph(
                        siblings: siblings,
                        selectedId: session.id
                    )
                }
                .buttonStyle(.plain)

                Menu {
                    Button {
                        Task { await store.refreshTerminalScreen(for: session, forceSnapshot: true) }
                    } label: {
                        Label(tr("Refresh Screen", "刷新屏幕", locale: store.locale), systemImage: "arrow.clockwise")
                    }
                    Button {
                        store.interruptSession(session)
                    } label: {
                        Label(tr("Interrupt (Ctrl+C)", "中断任务 (Ctrl+C)", locale: store.locale), systemImage: "stop.circle")
                    }
                    Divider()
                    Button(role: .destructive) {
                        store.killSession(session)
                    } label: {
                        Label(tr("Kill Terminal", "终止终端", locale: store.locale), systemImage: "trash")
                    }
                } label: {
                    Image(systemName: "ellipsis")
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundStyle(CodaHerdrTheme.textSecondary)
                        .frame(width: 28, height: 32)
                        .background(
                            RoundedRectangle(cornerRadius: 8, style: .continuous)
                                .fill(CodaHerdrTheme.elevatedBG)
                                .overlay(
                                    RoundedRectangle(cornerRadius: 8, style: .continuous)
                                        .strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1)
                                )
                        )
                }
                .buttonStyle(.plain)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, (isCompactHeight || store.formFactor == .iphoneDuoCoverPortrait || store.formFactor == .iphoneDuoInnerLandscape) ? 6 : 10)
        .background(CodaHerdrTheme.cardBG.opacity(0.65))
    }

    // MARK: - OSC 7501 Needs-You Banner

    private var blockedActionBanner: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                Image(systemName: "exclamationmark.triangle.fill")
                    .font(.system(size: 13))
                    .foregroundStyle(CodaHerdrTheme.amberNeedsYou)
                Text(session.statusMessage ?? tr("Waiting for your input", "等待你的确认", locale: store.locale))
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(CodaHerdrTheme.textPrimary)
                    .lineLimit(2)
                Spacer()
            }

            HStack(spacing: 8) {
                quickApprovePill("1 · Yes") {
                    store.sendDirectText("1", submit: true, to: session)
                }
                quickApprovePill("2 · Always") {
                    store.sendDirectText("2", submit: true, to: session)
                }
                if isSplitThumb {
                    Spacer(minLength: 28)
                }
                quickApprovePill("Esc") {
                    store.interruptSession(session)
                }
                if !isSplitThumb {
                    Spacer()
                }
            }
        }
        .padding(10)
        .background(
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .fill(CodaHerdrTheme.amberNeedsYou.opacity(0.11))
                .overlay(
                    RoundedRectangle(cornerRadius: 12, style: .continuous)
                        .strokeBorder(CodaHerdrTheme.amberNeedsYou.opacity(0.45), lineWidth: 1)
                )
        )
    }

    private func quickApprovePill(_ label: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(label)
                .font(.system(size: 11, weight: .semibold, design: .monospaced))
                .foregroundStyle(CodaHerdrTheme.amberNeedsYou)
                .padding(.horizontal, 10)
                .padding(.vertical, 5)
                .background(
                    Capsule()
                        .fill(CodaHerdrTheme.canvasBG.opacity(0.85))
                        .overlay(Capsule().strokeBorder(CodaHerdrTheme.amberNeedsYou.opacity(0.45), lineWidth: 1))
                )
        }
        .buttonStyle(.plain)
    }

    // MARK: - Styled ANSI / Git Diff Terminal Scrollback

    private var terminalScrollViewport: some View {
        let lines = store.terminalLinesBySession[session.id] ?? []
        let renderRevision = store.terminalRenderRevisionBySession[session.id] ?? 0

        return ScrollViewReader { proxy in
            ScrollView(.vertical, showsIndicators: true) {
                LazyVStack(alignment: .leading, spacing: 1.5) {
                    if lines.isEmpty {
                        VStack(alignment: .leading, spacing: 8) {
                            HStack(spacing: 8) {
                                ProgressView()
                                    .controlSize(.small)
                                    .tint(CodaHerdrTheme.cyanWorking)
                                Text(tr("Streaming encrypted terminal buffer...", "正在同步加密终端画面...", locale: store.locale))
                                    .font(.system(size: 12, design: .monospaced))
                                    .foregroundStyle(CodaHerdrTheme.textSecondary)
                            }
                            if !session.screenPreview.isEmpty {
                                Text(session.screenPreview)
                                    .font(.system(size: store.terminalFontSize, design: .monospaced))
                                    .foregroundStyle(CodaHerdrTheme.textPrimary.opacity(0.88))
                            }
                        }
                        .padding(14)
                    } else {
                        ForEach(lines) { line in
                            styledLineRow(line)
                                .id(line.id)
                        }
                    }

                    Color.clear
                        .frame(height: hideBottomControls ? 18 : ((local.showDirectInputBar ? 148 : 88) + (store.showTactileTwoRowKeypad ? 86 : 0)))
                        .id("terminal_bottom_anchor")
                        .onAppear {
                            local.atBottom = true
                        }
                        .onDisappear {
                            if local.lastHandledRevision == renderRevision && Date() >= local.suppressDisappearUntil {
                                local.atBottom = false
                            }
                        }
                }
                .padding(.horizontal, 12)
                .padding(.top, 10)
            }
            .simultaneousGesture(
                DragGesture(minimumDistance: 12)
                    .onChanged { value in
                        if value.translation.height > 12 {
                            local.atBottom = false
                        }
                    }
            )
            .codaDismissKeyboardInteractively()
            .onChange(of: session.id) { _, _ in
                local.atBottom = true
                local.lastHandledRevision = renderRevision
                local.suppressDisappearUntil = Date().addingTimeInterval(0.25)
                local.directInputBuffer = ""
                proxy.scrollTo("terminal_bottom_anchor", anchor: .bottom)
            }
            .onChange(of: renderRevision) { _, _ in
                let wasAtBottom = local.atBottom
                local.lastHandledRevision = renderRevision
                if wasAtBottom {
                    local.suppressDisappearUntil = Date().addingTimeInterval(0.25)
                    proxy.scrollTo("terminal_bottom_anchor", anchor: .bottom)
                }
            }
            .onAppear {
                local.atBottom = true
                local.lastHandledRevision = renderRevision
                local.suppressDisappearUntil = Date().addingTimeInterval(0.25)
                proxy.scrollTo("terminal_bottom_anchor", anchor: .bottom)
            }
            .overlay(alignment: .bottomTrailing) {
                Button {
                    local.atBottom = true
                    local.lastHandledRevision = renderRevision
                    local.suppressDisappearUntil = Date().addingTimeInterval(0.25)
                    withAnimation(.easeOut(duration: 0.15)) {
                        proxy.scrollTo("terminal_bottom_anchor", anchor: .bottom)
                    }
                } label: {
                    Image(systemName: "arrow.down")
                        .font(.system(size: 12, weight: .bold))
                        .foregroundStyle(local.atBottom ? CodaHerdrTheme.textSecondary : CodaHerdrTheme.cyanWorking)
                        .frame(width: 28, height: 28)
                        .background(
                            Circle()
                                .fill(CodaHerdrTheme.elevatedBG.opacity(local.atBottom ? 0.8 : 0.98))
                                .overlay(
                                    Circle().strokeBorder(
                                        local.atBottom ? CodaHerdrTheme.borderHairline : CodaHerdrTheme.cyanWorking.opacity(0.7),
                                        lineWidth: 1
                                    )
                                )
                        )
                }
                .buttonStyle(.plain)
                .padding(.trailing, 14)
                .padding(.bottom, hideBottomControls ? 14 : ((local.showDirectInputBar ? 124 : 74) + (store.showTactileTwoRowKeypad ? 86 : 0)))
            }
        }
    }

    private func styledLineRow(_ line: GhosttyStyledLine) -> some View {
        let rowBG: Color
        switch line.kind {
        case .diffAdded: rowBG = CodaHerdrTheme.greenDiff.opacity(0.11)
        case .diffRemoved: rowBG = CodaHerdrTheme.redDiff.opacity(0.11)
        case .diffHunk: rowBG = CodaHerdrTheme.cyanWorking.opacity(0.08)
        case .promptChoice: rowBG = CodaHerdrTheme.elevatedBG.opacity(0.75)
        case .normal: rowBG = .clear
        }

        return Text(buildAttributedLine(line))
            .font(.system(size: store.terminalFontSize, design: .monospaced))
            .textSelection(.enabled)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, rowBG == .clear ? 0 : 6)
            .padding(.vertical, rowBG == .clear ? 0 : 1.5)
            .background(
                RoundedRectangle(cornerRadius: 4, style: .continuous)
                    .fill(rowBG)
            )
    }

    private func buildAttributedLine(_ line: GhosttyStyledLine) -> AttributedString {
        if line.spans.isEmpty {
            return AttributedString(" ")
        }
        var out = AttributedString()
        for span in line.spans {
            var seg = AttributedString(span.text)
            let baseFG: Color
            if let fg = span.fg {
                baseFG = Color(
                    red: Double(fg.r) / 255.0,
                    green: Double(fg.g) / 255.0,
                    blue: Double(fg.b) / 255.0
                )
            } else {
                switch line.kind {
                case .diffAdded: baseFG = CodaHerdrTheme.greenDiff
                case .diffRemoved: baseFG = CodaHerdrTheme.redDiff
                case .diffHunk: baseFG = CodaHerdrTheme.cyanWorking
                case .promptChoice: baseFG = CodaHerdrTheme.textPrimary
                case .normal: baseFG = span.dim ? CodaHerdrTheme.textSecondary : CodaHerdrTheme.textPrimary
                }
            }
            let baseBG: Color? = span.bg.map { bg in
                Color(
                    red: Double(bg.r) / 255.0,
                    green: Double(bg.g) / 255.0,
                    blue: Double(bg.b) / 255.0
                )
            }
            if span.inverse {
                seg.foregroundColor = baseBG ?? CodaHerdrTheme.canvasBG
                seg.backgroundColor = baseFG
            } else {
                seg.foregroundColor = baseFG
                if let baseBG {
                    seg.backgroundColor = baseBG
                }
            }
            if span.bold {
                seg.inlinePresentationIntent = .stronglyEmphasized
            }
            out.append(seg)
        }
        return out
    }

    // MARK: - Direct Software/Hardware Keyboard Capture Strip

    private var quickCommandsStrip: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 6) {
                ForEach(CodaHostHealthSnapshot.quickTerminalCommands, id: \.self) { cmd in
                    Button {
                        store.sendDirectText(cmd, submit: true, to: session)
                    } label: {
                        Text(cmd)
                            .font(.system(size: 11, weight: .medium, design: .monospaced))
                            .foregroundStyle(CodaHerdrTheme.textSecondary)
                            .padding(.horizontal, 10)
                            .padding(.vertical, 5)
                            .background(
                                Capsule()
                                    .fill(CodaHerdrTheme.elevatedBG.opacity(0.95))
                                    .overlay(Capsule().strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
                            )
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(.horizontal, 4)
        }
    }

    private var directKeystrokeStrip: some View {
        HStack(spacing: 8) {
            if store.ctrlActive {
                modifierChip("CTRL")
            }
            if store.altActive {
                modifierChip("ALT")
            }

            TextField(
                tr("Type command or keystroke (Enter to send)...", "输入指令或按键（回车发送）...", locale: store.locale),
                text: $local.directInputBuffer
            )
            .font(.system(size: 13, design: .monospaced))
            .foregroundStyle(CodaHerdrTheme.textPrimary)
            .focused($directKeyboardFocused)
            .codaNoAutocapAndSpelling()
            .onChange(of: local.directInputBuffer) { _, next in
                if (store.ctrlActive || store.altActive), let last = next.last {
                    local.directInputBuffer = ""
                    store.sendDirectText(String(last), submit: false, to: session)
                }
            }
            .onSubmit {
                let text = local.directInputBuffer
                local.directInputBuffer = ""
                if text.isEmpty {
                    store.sendQuickKey(.enter, to: session)
                } else {
                    store.sendDirectText(text, submit: true, to: session)
                }
            }

            Button {
                let text = local.directInputBuffer
                local.directInputBuffer = ""
                if text.isEmpty {
                    store.sendQuickKey(.enter, to: session)
                } else {
                    store.sendDirectText(text, submit: true, to: session)
                }
            } label: {
                Image(systemName: "arrow.up.circle.fill")
                    .font(.system(size: 20))
                    .foregroundStyle(CodaHerdrTheme.cyanWorking)
            }
            .buttonStyle(.plain)

            Button {
                directKeyboardFocused = false
                withAnimation(.snappy(duration: 0.16)) {
                    local.showDirectInputBar = false
                }
            } label: {
                Image(systemName: "xmark.circle.fill")
                    .font(.system(size: 16))
                    .foregroundStyle(CodaHerdrTheme.textDim)
            }
            .buttonStyle(.plain)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .fill(CodaHerdrTheme.elevatedBG)
                .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(CodaHerdrTheme.cyanWorking.opacity(0.4), lineWidth: 1))
        )
        .onAppear {
            Task { @MainActor in
                directKeyboardFocused = true
            }
        }
    }

    private func modifierChip(_ title: String) -> some View {
        Text(title)
            .font(.system(size: 10, weight: .bold, design: .monospaced))
            .foregroundStyle(.black)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(Capsule().fill(CodaHerdrTheme.cyanWorking))
    }

    // MARK: - D-Pad Popover Row (`← ↓ ↑ → ⏎`)

    private var dpadRow: some View {
        HStack(spacing: 8) {
            dpadButton("arrow.left", key: .left)
            dpadButton("arrow.down", key: .down)
            dpadButton("arrow.up", key: .up)
            dpadButton("arrow.right", key: .right)
            dpadButton("return", key: .enter)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .background(
            Capsule()
                .fill(CodaHerdrTheme.elevatedBG.opacity(0.96))
                .overlay(Capsule().strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
                .shadow(color: .black.opacity(0.45), radius: 10, x: 0, y: 4)
        )
    }

    private func dpadButton(_ symbol: String, key: HerdrQuickKey) -> some View {
        Button {
            store.sendQuickKey(key, to: session)
        } label: {
            Image(systemName: symbol)
                .font(.system(size: 14, weight: .semibold))
                .foregroundStyle(CodaHerdrTheme.textPrimary)
                .frame(width: 36, height: 32)
                .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(CodaHerdrTheme.cardBG))
        }
        .buttonStyle(.plain)
    }

    // MARK: - Bottom Floating Quick Bar + Keyboard + Compose

    private var leftQuickKeysCluster: some View {
        HStack(spacing: 4) {
            quickKeyButton("esc") {
                store.sendQuickKey(.esc, to: session)
            }
            quickModifierButton("ctrl", active: store.ctrlActive) {
                store.ctrlActive.toggle()
            }
            quickModifierButton("alt", active: store.altActive) {
                store.altActive.toggle()
            }
            quickKeyButton("tab") {
                store.sendQuickKey(.tab, to: session)
            }
            quickKeyButton("⇧tab") {
                store.sendQuickKey(.shiftTab, to: session)
            }
        }
    }

    private var rightQuickControlsCluster: some View {
        HStack(spacing: 4) {
            Button {
                withAnimation(.snappy(duration: 0.18)) {
                    store.dpadVisible.toggle()
                }
            } label: {
                Image(systemName: "arrow.up.and.down.and.arrow.left.and.right")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(store.dpadVisible ? CodaHerdrTheme.cyanWorking : CodaHerdrTheme.textPrimary)
                    .padding(.horizontal, 9)
                    .padding(.vertical, 7)
                    .background(
                        Capsule()
                            .fill(store.dpadVisible ? CodaHerdrTheme.cyanWorking.opacity(0.16) : .clear)
                    )
            }
            .buttonStyle(.plain)

            Button {
                withAnimation(.snappy(duration: 0.18)) {
                    store.showTactileTwoRowKeypad.toggle()
                }
            } label: {
                Image(systemName: "square.grid.3x2.fill")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(store.showTactileTwoRowKeypad ? CodaHerdrTheme.cyanWorking : CodaHerdrTheme.textPrimary)
                    .padding(.horizontal, 9)
                    .padding(.vertical, 7)
                    .background(
                        Capsule()
                            .fill(store.showTactileTwoRowKeypad ? CodaHerdrTheme.cyanWorking.opacity(0.16) : .clear)
                    )
            }
            .buttonStyle(.plain)

            quickKeyButton("^C", tint: CodaHerdrTheme.redDiff) {
                store.sendQuickKey(.ctrlC, to: session)
            }
        }
    }

    private func dockPillContainer<Content: View>(@ViewBuilder content: () -> Content) -> some View {
        content()
            .padding(.horizontal, 6)
            .padding(.vertical, 4)
            .background(
                Capsule()
                    .fill(CodaHerdrTheme.elevatedBG.opacity(0.96))
                    .overlay(Capsule().strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
                    .shadow(color: .black.opacity(0.45), radius: 14, x: 0, y: 6)
            )
    }

    @ViewBuilder
    private var dockUtilityButtons: some View {
        Button {
            withAnimation(.snappy(duration: 0.16)) {
                local.showDirectInputBar.toggle()
            }
            if local.showDirectInputBar {
                Task { @MainActor in
                    directKeyboardFocused = true
                }
            } else {
                directKeyboardFocused = false
            }
        } label: {
            Image(systemName: local.showDirectInputBar ? "keyboard.chevron.compact.down" : "keyboard")
                .font(.system(size: 15, weight: .semibold))
                .foregroundStyle(local.showDirectInputBar ? CodaHerdrTheme.cyanWorking : CodaHerdrTheme.textPrimary)
                .frame(width: 40, height: 40)
                .background(
                    Circle()
                        .fill(CodaHerdrTheme.elevatedBG.opacity(0.96))
                        .overlay(
                            Circle().strokeBorder(
                                local.showDirectInputBar ? CodaHerdrTheme.cyanWorking.opacity(0.6) : CodaHerdrTheme.borderHairline,
                                lineWidth: 1
                            )
                        )
                )
        }
        .buttonStyle(.plain)

        Button {
            store.openComposeSheet(for: session, isSecondary: isSecondaryLeaf)
        } label: {
            Image(systemName: "square.and.pencil")
                .font(.system(size: 15, weight: .semibold))
                .foregroundStyle(CodaHerdrTheme.textPrimary)
                .frame(width: 40, height: 40)
                .background(
                    Circle()
                        .fill(CodaHerdrTheme.elevatedBG.opacity(0.96))
                        .overlay(Circle().strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
                )
        }
        .buttonStyle(.plain)
    }

    @ViewBuilder
    private var floatingQuickDock: some View {
        if isSplitThumb {
            HStack(spacing: 8) {
                dockPillContainer {
                    leftQuickKeysCluster
                }

                Spacer(minLength: 28)

                dockPillContainer {
                    rightQuickControlsCluster
                }

                dockUtilityButtons
            }
        } else {
            HStack(spacing: 8) {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 4) {
                        leftQuickKeysCluster
                        rightQuickControlsCluster
                    }
                    .padding(.horizontal, 6)
                    .padding(.vertical, 4)
                }
                .background(
                    Capsule()
                        .fill(CodaHerdrTheme.elevatedBG.opacity(0.96))
                        .overlay(Capsule().strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
                        .shadow(color: .black.opacity(0.45), radius: 14, x: 0, y: 6)
                )

                dockUtilityButtons
            }
        }
    }

    private func quickKeyButton(_ title: String, tint: Color = CodaHerdrTheme.textPrimary, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(title)
                .font(.system(size: 12, weight: .medium, design: .monospaced))
                .foregroundStyle(tint)
                .padding(.horizontal, 9)
                .padding(.vertical, 7)
                .background(
                    Capsule()
                        .fill(CodaHerdrTheme.cardBG.opacity(0.7))
                )
        }
        .buttonStyle(.plain)
    }

    private func quickModifierButton(_ title: String, active: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(title)
                .font(.system(size: 12, weight: .semibold, design: .monospaced))
                .foregroundStyle(active ? CodaHerdrTheme.cyanWorking : CodaHerdrTheme.textPrimary)
                .padding(.horizontal, 9)
                .padding(.vertical, 7)
                .background(
                    Capsule()
                        .fill(active ? CodaHerdrTheme.cyanWorking.opacity(0.18) : CodaHerdrTheme.cardBG.opacity(0.7))
                        .overlay(
                            Capsule().strokeBorder(active ? CodaHerdrTheme.cyanWorking.opacity(0.55) : .clear, lineWidth: 1)
                        )
                )
        }
        .buttonStyle(.plain)
    }
}

// MARK: - Top-Right Interactive Live Pane Minimap Button

public struct CodaMiniPaneMapGlyph: View {
    public let siblings: [CodaAgentSession]
    public let selectedId: String

    public init(siblings: [CodaAgentSession], selectedId: String) {
        self.siblings = siblings
        self.selectedId = selectedId
    }

    public var body: some View {
        let items = Array(siblings.prefix(4))
        let hasAttention = siblings.contains { $0.state == .needsYou || $0.state == .working }

        HStack(spacing: 3) {
            if items.isEmpty {
                RoundedRectangle(cornerRadius: 2).fill(CodaHerdrTheme.textSecondary).frame(width: 8, height: 14)
            } else {
                ForEach(items) { item in
                    let color: Color = {
                        switch item.state {
                        case .needsYou: return CodaHerdrTheme.amberNeedsYou
                        case .working: return CodaHerdrTheme.cyanWorking
                        case .done: return item.unreadCompletion ? CodaHerdrTheme.blueUnread : CodaHerdrTheme.textDim
                        case .error: return CodaHerdrTheme.redDiff
                        case .idle: return item.id == selectedId ? CodaHerdrTheme.textPrimary : CodaHerdrTheme.textDim
                        }
                    }()
                    RoundedRectangle(cornerRadius: 2)
                        .fill(color.opacity(item.id == selectedId ? 0.95 : 0.45))
                        .frame(width: max(5, 16 / CGFloat(items.count)), height: 14)
                }
            }
        }
        .padding(.horizontal, 7)
        .frame(height: 32)
        .background(
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .fill(CodaHerdrTheme.elevatedBG)
                .overlay(
                    RoundedRectangle(cornerRadius: 8, style: .continuous)
                        .strokeBorder(hasAttention ? CodaHerdrTheme.amberNeedsYou.opacity(0.45) : CodaHerdrTheme.borderHairline, lineWidth: 1)
                )
        )
    }
}

// MARK: - Compose & File Attachment Sheet

private final class ComposeSheetLocalState: ObservableObject {
    @Published var showFileImporter: Bool = false
}

public struct CodaComposePromptSheet: View {
    @ObservedObject public var store: CodaWorkspaceStore
    public let session: CodaAgentSession
    @Environment(\.dismiss) private var dismiss
    @StateObject private var local = ComposeSheetLocalState()

    public var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                VStack(alignment: .leading, spacing: 2) {
                    Text(tr("Compose Prompt", "撰写指令", locale: store.locale))
                        .font(.system(size: 16, weight: .semibold))
                        .foregroundStyle(CodaHerdrTheme.textPrimary)
                    Text("\(session.workspaceName) · \(session.agentName) · \(session.hostName)")
                        .font(.system(size: 11))
                        .foregroundStyle(CodaHerdrTheme.textSecondary)
                }
                Spacer()
                Button(tr("Close", "关闭", locale: store.locale)) {
                    dismiss()
                }
                .foregroundStyle(CodaHerdrTheme.textSecondary)
            }

            TextEditor(text: $store.composeText)
                .font(.system(size: 14, design: .monospaced))
                .foregroundStyle(CodaHerdrTheme.textPrimary)
                .scrollContentBackground(.hidden)
                .padding(10)
                .frame(minHeight: 150)
                .background(
                    RoundedRectangle(cornerRadius: 12, style: .continuous)
                        .fill(CodaHerdrTheme.canvasBG)
                        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
                )

            if let uploadBanner = store.uploadStatusBanner {
                Text(uploadBanner)
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(CodaHerdrTheme.cyanWorking)
            }

            if let voiceErr = store.voiceError {
                Text(voiceErr)
                    .font(.system(size: 11))
                    .foregroundStyle(CodaHerdrTheme.redDiff)
            }

            HStack(spacing: 8) {
                Button {
                    local.showFileImporter = true
                } label: {
                    Label(tr("Attach", "上传文件", locale: store.locale), systemImage: "paperclip")
                        .font(.system(size: 12, weight: .medium))
                        .foregroundStyle(CodaHerdrTheme.textPrimary)
                        .padding(.horizontal, 12)
                        .padding(.vertical, 8)
                        .background(
                            Capsule()
                                .fill(CodaHerdrTheme.elevatedBG)
                                .overlay(Capsule().strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
                        )
                }
                .buttonStyle(.plain)

                Button {
                    Task { await store.toggleVoiceDictation() }
                } label: {
                    HStack(spacing: 6) {
                        if store.isTranscribingVoice {
                            ProgressView()
                                .controlSize(.small)
                                .tint(CodaHerdrTheme.cyanWorking)
                        } else {
                            Image(systemName: store.isRecordingVoice ? "stop.circle.fill" : "mic.fill")
                        }
                        Text(
                            store.isTranscribingVoice
                                ? tr("Transcribing...", "转写中...", locale: store.locale)
                                : (store.isRecordingVoice
                                    ? tr("Stop", "停止录音", locale: store.locale)
                                    : tr("Voice", "语音输入", locale: store.locale))
                        )
                    }
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(
                        store.isRecordingVoice
                            ? .white
                            : (store.isTranscribingVoice ? CodaHerdrTheme.cyanWorking : CodaHerdrTheme.textPrimary)
                    )
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                    .background(
                        Capsule()
                            .fill(store.isRecordingVoice ? CodaHerdrTheme.redDiff : CodaHerdrTheme.elevatedBG)
                            .overlay(
                                Capsule().strokeBorder(
                                    store.isRecordingVoice ? CodaHerdrTheme.redDiff : CodaHerdrTheme.borderHairline,
                                    lineWidth: 1
                                )
                            )
                    )
                }
                .buttonStyle(.plain)
                .disabled(store.isTranscribingVoice)

                Button {
                    store.cancelVoiceDictation()
                    store.pendingOpenSettingsAfterCompose = true
                    dismiss()
                } label: {
                    Image(systemName: "slider.horizontal.3")
                        .font(.system(size: 12, weight: .medium))
                        .foregroundStyle(CodaHerdrTheme.textSecondary)
                        .frame(width: 32, height: 32)
                        .background(
                            Circle()
                                .fill(CodaHerdrTheme.elevatedBG)
                                .overlay(Circle().strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
                        )
                }
                .buttonStyle(.plain)

                Spacer()

                Button {
                    store.submitComposePrompt(for: session)
                    dismiss()
                } label: {
                    HStack(spacing: 6) {
                        Text(tr("Send", "发送", locale: store.locale))
                        Image(systemName: "arrow.up")
                    }
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(.black)
                    .padding(.horizontal, 16)
                    .padding(.vertical, 9)
                    .background(Capsule().fill(CodaHerdrTheme.textPrimary))
                }
                .buttonStyle(.plain)
                .disabled(store.composeText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
        }
        .padding(18)
        .background(CodaHerdrTheme.cardBG)
        .presentationDetents([.medium, .large])
        .onDisappear {
            store.cancelVoiceDictation()
        }
        .fileImporter(
            isPresented: $local.showFileImporter,
            allowedContentTypes: [.item]
        ) { result in
            if case .success(let url) = result {
                store.uploadAttachment(fileURL: url, for: session)
            }
        }
    }
}

// MARK: - Live Pane Minimap Overlay Sheet

public struct CodaPaneMinimapSheet: View {
    @ObservedObject public var store: CodaWorkspaceStore
    public let session: CodaAgentSession
    @Environment(\.dismiss) private var dismiss

    public var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack {
                VStack(alignment: .leading, spacing: 2) {
                    Text("\(session.workspaceName) · \(tr("Pane Map", "窗格总览", locale: store.locale))")
                        .font(.system(size: 17, weight: .semibold))
                        .foregroundStyle(CodaHerdrTheme.textPrimary)
                    Text("\(session.hostName) · \(session.branch)")
                        .font(.system(size: 12))
                        .foregroundStyle(CodaHerdrTheme.textSecondary)
                }
                Spacer()
                if let devId = session.deviceId,
                   let device = store.remoteMachines.first(where: { $0.id == devId }) {
                    Menu {
                        Button("Claude Code") { store.launchAgent(agent: "claude", on: device, cwd: session.cwd); dismiss() }
                        Button("Codex") { store.launchAgent(agent: "codex", on: device, cwd: session.cwd); dismiss() }
                        Button("Grok") { store.launchAgent(agent: "grok", on: device, cwd: session.cwd); dismiss() }
                        Button("Antigravity (AGY)") { store.launchAgent(agent: "agy", on: device, cwd: session.cwd); dismiss() }
                        Button("Gemini CLI") { store.launchAgent(agent: "gemini", on: device, cwd: session.cwd); dismiss() }
                        Divider()
                        Button("Shell") { store.launchAgent(agent: "shell", on: device, cwd: session.cwd); dismiss() }
                    } label: {
                        Label(tr("New Pane", "新建窗格", locale: store.locale), systemImage: "plus")
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundStyle(.black)
                            .padding(.horizontal, 12)
                            .padding(.vertical, 6)
                            .background(Capsule().fill(CodaHerdrTheme.textPrimary))
                    }
                }
            }

            ScrollView {
                LazyVGrid(columns: [GridItem(.flexible(), spacing: 10), GridItem(.flexible(), spacing: 10)], spacing: 10) {
                    ForEach(store.siblingSessions(for: session)) { sib in
                        VStack(alignment: .leading, spacing: 6) {
                            HStack(spacing: 6) {
                                Text(sib.agentName)
                                    .font(.system(size: 12, weight: .semibold, design: .monospaced))
                                    .foregroundStyle(CodaHerdrTheme.textPrimary)
                                    .lineLimit(1)
                                Spacer()
                                CodaSessionStatusBadge(session: sib, locale: store.locale)
                                Button {
                                    store.killSession(sib)
                                    if sib.id == session.id {
                                        dismiss()
                                    }
                                } label: {
                                    Image(systemName: "xmark.circle.fill")
                                        .font(.system(size: 13))
                                        .foregroundStyle(CodaHerdrTheme.textDim)
                                }
                                .buttonStyle(.plain)
                            }
                            Button {
                                if store.duoSplitMode == .dualTerminalSplit && store.activeSheetIsSecondary {
                                    store.selectSecondarySession(sib)
                                } else {
                                    store.selectSession(sib)
                                }
                                dismiss()
                            } label: {
                                VStack(alignment: .leading, spacing: 4) {
                                    Text(sib.taskTitle)
                                        .font(.system(size: 11, weight: .medium))
                                        .foregroundStyle(CodaHerdrTheme.textSecondary)
                                        .lineLimit(1)

                                    Text(sib.screenPreview.isEmpty ? "…" : String(sib.screenPreview.suffix(120)))
                                        .font(.system(size: 9, design: .monospaced))
                                        .foregroundStyle(CodaHerdrTheme.textDim)
                                        .lineLimit(4)
                                        .frame(maxWidth: .infinity, alignment: .leading)
                                        .frame(height: 44, alignment: .topLeading)
                                }
                            }
                            .buttonStyle(.plain)
                        }
                        .padding(10)
                        .background(
                            RoundedRectangle(cornerRadius: 12, style: .continuous)
                                .fill(CodaHerdrTheme.canvasBG)
                                .overlay(
                                    RoundedRectangle(cornerRadius: 12, style: .continuous)
                                        .strokeBorder(sib.id == session.id ? CodaHerdrTheme.cyanWorking : CodaHerdrTheme.borderHairline, lineWidth: sib.id == session.id ? 1.5 : 1)
                                )
                        )
                    }
                }
            }
        }
        .padding(18)
        .background(CodaHerdrTheme.cardBG)
        .presentationDetents([.medium, .large])
    }
}

// MARK: - Live Remote Git Diff Inspector Sheet

public struct CodaGitDiffSheet: View {
    @ObservedObject public var store: CodaWorkspaceStore
    public let session: CodaAgentSession
    @Environment(\.dismiss) private var dismiss

    public var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                VStack(alignment: .leading, spacing: 2) {
                    Text("\(session.workspaceName) · Git Diff")
                        .font(.system(size: 16, weight: .semibold))
                        .foregroundStyle(CodaHerdrTheme.textPrimary)
                    Text(store.gitDiffResult?.branch ?? session.branch)
                        .font(.system(size: 12, design: .monospaced))
                        .foregroundStyle(CodaHerdrTheme.textSecondary)
                }
                Spacer()
                Button(tr("Close", "关闭", locale: store.locale)) { dismiss() }
                    .foregroundStyle(CodaHerdrTheme.textSecondary)
            }

            Divider().background(CodaHerdrTheme.borderHairline)

            if store.gitDiffLoading {
                ProgressView()
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if let diff = store.gitDiffResult {
                ScrollView {
                    VStack(alignment: .leading, spacing: 4) {
                        if !diff.statusShort.isEmpty {
                            Text(diff.statusShort)
                                .font(.system(size: 11, design: .monospaced))
                                .foregroundStyle(CodaHerdrTheme.cyanWorking)
                                .padding(.bottom, 8)
                        }
                        let rawLines = (diff.diffUnified.isEmpty ? tr("Working tree clean", "工作区无未提交变更", locale: store.locale) : diff.diffUnified).components(separatedBy: "\n")
                        ForEach(Array(rawLines.enumerated()), id: \.offset) { _, line in
                            let fg: Color = line.hasPrefix("+") && !line.hasPrefix("+++")
                                ? CodaHerdrTheme.greenDiff
                                : (line.hasPrefix("-") && !line.hasPrefix("---")
                                    ? CodaHerdrTheme.redDiff
                                    : (line.hasPrefix("@@") ? CodaHerdrTheme.cyanWorking : CodaHerdrTheme.textPrimary))
                            Text(line.isEmpty ? " " : line)
                                .font(.system(size: 11.5, design: .monospaced))
                                .foregroundStyle(fg)
                                .frame(maxWidth: .infinity, alignment: .leading)
                        }
                    }
                }
            } else {
                Text(tr("No diff loaded.", "暂无差异输出。", locale: store.locale))
                    .foregroundStyle(CodaHerdrTheme.textSecondary)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .padding(18)
        .background(CodaHerdrTheme.cardBG)
        .presentationDetents([.medium, .large])
    }
}
