import CodaGhosttyKit
import CodaKit
import Combine
import SwiftUI
import UniformTypeIdentifiers
#if canImport(PhotosUI)
import PhotosUI
#endif

// MARK: - Full-Screen Mobile / Right-Pane iPad & Mac Terminal View (`herdr.dev/mobile/`)

private final class TerminalViewportLocalState: ObservableObject {
    @Published var directInputBuffer: String = ""
    @Published var showDirectInputBar: Bool = false
    @Published var useMultiLineComposer: Bool = false
    @Published var showSearchBar: Bool = false
    @Published var searchQuery: String = ""
    @Published var searchMatchIndex: Int = 0
    @Published var baseFontScale: CGFloat = 1.0
    @Published var atBottom: Bool = true
    #if canImport(PhotosUI)
    @Published var selectedPhotoItem: PhotosPickerItem? = nil
    #endif
    var lastHandledRevision: UInt64 = 0
    var suppressDisappearUntil: Date = .distantPast
    var lastAltDragY: CGFloat = 0
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

    private var shouldShowTopBlockedBanner: Bool {
        guard session.state == .needsYou else { return false }
        if hideBottomControls { return true }
        // On compact viewports or when the direct input composer is open, avoid stacking two
        // redundant Needs-You bars (`blockedActionBanner` + `floatingNeedsYouApprovalBar`) so
        // terminal lines remain visible on iPhone SE, landscape, and Duo Cover!
        if isCompactHeight || local.showDirectInputBar {
            return false
        }
        return true
    }

    private var shouldShowQuickCommandsStrip: Bool {
        guard local.showDirectInputBar || store.showTactileTwoRowKeypad else { return false }
        if isCompactHeight && local.showDirectInputBar {
            return false
        }
        return true
    }

    private var bottomOverlayClearance: CGFloat {
        if hideBottomControls {
            return 18
        }
        var total: CGFloat = 56 // floatingQuickDock base height
        if session.status == .needsYou {
            total += 46
        }
        if shouldShowQuickCommandsStrip {
            total += 36
        }
        if local.showDirectInputBar {
            total += local.useMultiLineComposer ? (isCompactHeight ? 92 : 112) : 76
        }
        if store.dpadVisible && !(isCompactHeight && local.showDirectInputBar) {
            total += 48
        }
        if store.showTactileTwoRowKeypad && !(isCompactHeight && local.showDirectInputBar) {
            total += 90
        }
        total += (isCompact || isCompactHeight) ? 8 : 14
        return total
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

            if local.showSearchBar {
                terminalSearchBar
                    .padding(.horizontal, 12)
                    .padding(.top, 6)
            }

            if shouldShowTopBlockedBanner {
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
                        if session.status == .needsYou {
                            floatingNeedsYouApprovalBar
                                .transition(.move(edge: .bottom).combined(with: .opacity))
                        }

                        if shouldShowQuickCommandsStrip {
                            quickCommandsStrip
                                .transition(.move(edge: .bottom).combined(with: .opacity))
                        }

                        if local.showDirectInputBar {
                            directKeystrokeStrip
                                .padding(.horizontal, 4)
                                .transition(.move(edge: .bottom).combined(with: .opacity))
                        }

                        if store.dpadVisible && !(isCompactHeight && local.showDirectInputBar) {
                            dpadRow
                                .transition(.move(edge: .bottom).combined(with: .opacity))
                        }

                        if store.showTactileTwoRowKeypad && !(isCompactHeight && local.showDirectInputBar) {
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
        .onDisappear {
            if !store.showComposeSheet && (store.voiceDictationSessionID == nil || store.voiceDictationSessionID == session.id) {
                store.cancelVoiceDictation()
            }
        }
        #if canImport(PhotosUI)
        .onChange(of: local.selectedPhotoItem) { _, nextItem in
            guard let item = nextItem else { return }
            local.selectedPhotoItem = nil
            Task { @MainActor in
                await uploadPickedPhotoItem(item, intoDirectInput: true)
            }
        }
        #endif
        .sheet(
            isPresented: Binding(
                get: { !isSecondaryLeaf && store.showComposeSheet },
                set: { if !isSecondaryLeaf { store.showComposeSheet = $0 } }
            ),
            onDismiss: {
                store.reconcileActiveSheetSessionOnDismissIfNeeded()
                if store.pendingOpenSettingsAfterCompose {
                    store.pendingOpenSettingsAfterCompose = false
                    store.settingsInitialFocusPairing = false
                    store.showSettingsSheet = true
                }
            }
        ) {
            CodaComposePromptSheet(store: store, session: store.activeSheetSession ?? session)
        }
        .sheet(
            isPresented: Binding(
                get: { !isSecondaryLeaf && store.showPaneMinimapSheet },
                set: { if !isSecondaryLeaf { store.showPaneMinimapSheet = $0 } }
            ),
            onDismiss: {
                store.reconcileActiveSheetSessionOnDismissIfNeeded()
            }
        ) {
            CodaPaneMinimapSheet(store: store, session: store.activeSheetSession ?? session)
        }
        .sheet(
            isPresented: Binding(
                get: { !isSecondaryLeaf && store.showGitDiffSheet },
                set: { if !isSecondaryLeaf { store.showGitDiffSheet = $0 } }
            ),
            onDismiss: {
                store.reconcileActiveSheetSessionOnDismissIfNeeded()
            }
        ) {
            CodaGitDiffSheet(store: store, session: store.activeSheetSession ?? session)
        }
        .sheet(
            isPresented: Binding(
                get: { !isSecondaryLeaf && store.showRemoteFilesSheet },
                set: { if !isSecondaryLeaf { store.showRemoteFilesSheet = $0 } }
            ),
            onDismiss: {
                store.reconcileActiveSheetSessionOnDismissIfNeeded()
            }
        ) {
            CodaRemoteFilesSheet(store: store, session: store.activeSheetSession ?? session)
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
                    if store.isSessionInAltScreen(session.id) {
                        Text("ALT")
                            .font(.system(size: 9, weight: .bold, design: .monospaced))
                            .foregroundStyle(CodaHerdrTheme.cyanWorking)
                            .padding(.horizontal, 5)
                            .padding(.vertical, 1.5)
                            .background(Capsule().fill(CodaHerdrTheme.cyanWorking.opacity(0.16)))
                    }
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
                        store.openRemoteInspector(for: session, mode: .changes)
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
                        withAnimation(.snappy(duration: 0.16)) {
                            local.showSearchBar.toggle()
                        }
                    } label: {
                        Label(tr("Find in Terminal", "终端内搜索", locale: store.locale), systemImage: "magnifyingglass")
                    }
                    Button {
                        store.openRemoteInspector(for: session, mode: .files)
                    } label: {
                        Label(tr("Workspace Files (E2EE)", "工作区文件浏览", locale: store.locale), systemImage: "folder")
                    }
                    Button {
                        store.openRemoteInspector(for: session, mode: .changes)
                    } label: {
                        Label(tr("Changes (Git Diff)", "代码变更 (Git Diff)", locale: store.locale), systemImage: "plus.forwardslash.minus")
                    }
                    Button {
                        store.openRemoteInspector(for: session, mode: .portPreview)
                    } label: {
                        Label(tr("Localhost Port Preview", "本地端口加密预览", locale: store.locale), systemImage: "globe")
                    }
                    ShareLink(
                        item: store.sessionMarkdownTranscript(for: session),
                        subject: Text("Coda Session — \(session.taskTitle)")
                    ) {
                        Label(tr("Export Session (.md)", "导出会话日志 (.md)", locale: store.locale), systemImage: "square.and.arrow.up")
                    }
                    Divider()
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
                    store.sendQuickKey(.esc, to: session)
                }
                quickApprovePill(tr("Interrupt (^C)", "中断 (^C)", locale: store.locale)) {
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

    private var currentSearchMatchIndices: [Int] {
        guard local.showSearchBar else { return [] }
        let lines = store.terminalLinesBySession[session.id] ?? []
        let plain = lines.map { $0.spans.map(\.text).joined() }
        return CodaTerminalTranscriptHelper.findMatchingLineIndices(in: plain, query: local.searchQuery)
    }

    private var terminalSearchBar: some View {
        let matches = currentSearchMatchIndices
        let activeOrd = matches.isEmpty ? 0 : (local.searchMatchIndex % matches.count) + 1
        return HStack(spacing: 8) {
            Image(systemName: "magnifyingglass")
                .font(.system(size: 12))
                .foregroundStyle(CodaHerdrTheme.textSecondary)

            TextField(
                tr("Find in scrollback…", "在终端滚动缓存中搜索…", locale: store.locale),
                text: $local.searchQuery
            )
            .font(.system(size: 12.5, design: .monospaced))
            .foregroundStyle(CodaHerdrTheme.textPrimary)
            .codaNoAutocapAndSpelling()
            .onChange(of: local.searchQuery) { _, _ in
                local.searchMatchIndex = 0
            }

            Text("\(activeOrd)/\(matches.count)")
                .font(.system(size: 11, design: .monospaced))
                .foregroundStyle(CodaHerdrTheme.textDim)

            Button {
                if !matches.isEmpty {
                    local.searchMatchIndex = (local.searchMatchIndex + matches.count - 1) % matches.count
                }
            } label: {
                Image(systemName: "chevron.up")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(CodaHerdrTheme.textPrimary)
                    .padding(5)
            }
            .buttonStyle(.plain)

            Button {
                if !matches.isEmpty {
                    local.searchMatchIndex = (local.searchMatchIndex + 1) % matches.count
                }
            } label: {
                Image(systemName: "chevron.down")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(CodaHerdrTheme.textPrimary)
                    .padding(5)
            }
            .buttonStyle(.plain)

            Button {
                withAnimation(.snappy(duration: 0.16)) {
                    local.showSearchBar = false
                }
            } label: {
                Image(systemName: "xmark")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(CodaHerdrTheme.textSecondary)
                    .padding(5)
            }
            .buttonStyle(.plain)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .background(
            RoundedRectangle(cornerRadius: 9, style: .continuous)
                .fill(CodaHerdrTheme.elevatedBG)
                .overlay(RoundedRectangle(cornerRadius: 9, style: .continuous).strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
        )
    }

    private var terminalScrollViewport: some View {
        let lines = store.terminalLinesBySession[session.id] ?? []
        let renderRevision = store.terminalRenderRevisionBySession[session.id] ?? 0
        let matchIndices = currentSearchMatchIndices
        let matchSet = Set(matchIndices)
        let activeMatchLineIdx: Int? = matchIndices.isEmpty
            ? nil
            : matchIndices[local.searchMatchIndex % matchIndices.count]

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
                        ForEach(Array(lines.enumerated()), id: \.element.id) { idx, line in
                            styledLineRow(
                                line,
                                isSearchHit: matchSet.contains(idx),
                                isActiveMatch: activeMatchLineIdx == idx
                            )
                            .id(line.id)
                        }
                    }

                    Color.clear
                        .frame(height: bottomOverlayClearance + 14)
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
                .contentShape(Rectangle())
                .onTapGesture {
                    guard !hideBottomControls else { return }
                    if !local.showDirectInputBar {
                        withAnimation(.snappy(duration: 0.16)) {
                            local.showDirectInputBar = true
                        }
                    }
                    Task { @MainActor in
                        directKeyboardFocused = true
                    }
                }
            }
            .simultaneousGesture(
                DragGesture(minimumDistance: 12)
                    .onChanged { value in
                        if store.isSessionInAltScreen(session.id) {
                            let deltaY = value.translation.height - local.lastAltDragY
                            if abs(deltaY) >= 24 {
                                let steps = max(1, min(4, Int(abs(deltaY) / 24)))
                                local.lastAltDragY = value.translation.height
                                store.sendTerminalScroll(up: deltaY > 0, lines: steps, to: session)
                            }
                        } else if value.translation.height > 12 {
                            local.atBottom = false
                        }
                    }
                    .onEnded { _ in
                        local.lastAltDragY = 0
                    }
            )
            .codaDismissKeyboardInteractively()
            .onChange(of: session.id) { _, _ in
                store.cancelVoiceDictation()
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
            .onChange(of: local.searchMatchIndex) { _, _ in
                if let targetIdx = activeMatchLineIdx, targetIdx < lines.count {
                    local.atBottom = false
                    withAnimation(.easeOut(duration: 0.15)) {
                        proxy.scrollTo(lines[targetIdx].id, anchor: .center)
                    }
                }
            }
            .onChange(of: local.searchQuery) { _, _ in
                if let firstIdx = matchIndices.first, firstIdx < lines.count {
                    local.atBottom = false
                    withAnimation(.easeOut(duration: 0.15)) {
                        proxy.scrollTo(lines[firstIdx].id, anchor: .center)
                    }
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
                .padding(.bottom, hideBottomControls ? 14 : (bottomOverlayClearance + 6))
            }
        }
    }

    private func styledLineRow(
        _ line: GhosttyStyledLine,
        isSearchHit: Bool = false,
        isActiveMatch: Bool = false
    ) -> some View {
        let rowBG: Color
        if isActiveMatch {
            rowBG = CodaHerdrTheme.amberNeedsYou.opacity(0.36)
        } else if isSearchHit {
            rowBG = CodaHerdrTheme.amberNeedsYou.opacity(0.18)
        } else {
            switch line.kind {
            case .diffAdded: rowBG = CodaHerdrTheme.greenDiff.opacity(0.11)
            case .diffRemoved: rowBG = CodaHerdrTheme.redDiff.opacity(0.11)
            case .diffHunk: rowBG = CodaHerdrTheme.cyanWorking.opacity(0.08)
            case .promptChoice: rowBG = CodaHerdrTheme.elevatedBG.opacity(0.75)
            case .normal: rowBG = .clear
            }
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

    private var floatingNeedsYouApprovalBar: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 6) {
                HStack(spacing: 4) {
                    Circle()
                        .fill(CodaHerdrTheme.amberNeedsYou)
                        .frame(width: 7, height: 7)
                    Text(tr("Needs you", "待审批", locale: store.locale))
                        .font(.system(size: 11, weight: .bold))
                        .foregroundStyle(CodaHerdrTheme.amberNeedsYou)
                }
                .padding(.leading, 6)
                .padding(.trailing, 2)

                approvalCapsuleButton(
                    tr("Allow (y)", "允许 (y)", locale: store.locale),
                    tint: CodaHerdrTheme.greenDiff
                ) {
                    store.sendDirectText("y", submit: true, to: session)
                }
                approvalCapsuleButton(
                    tr("Reject (Esc)", "拒绝 (Esc)", locale: store.locale),
                    tint: CodaHerdrTheme.amberNeedsYou
                ) {
                    store.sendQuickKey(.esc, to: session)
                }
                approvalCapsuleButton(
                    tr("Interrupt (^C)", "中断 (^C)", locale: store.locale),
                    tint: CodaHerdrTheme.redDiff
                ) {
                    store.interruptSession(session)
                }
                approvalCapsuleButton("1", tint: CodaHerdrTheme.amberNeedsYou) {
                    store.sendDirectText("1", submit: true, to: session)
                }
                approvalCapsuleButton("2", tint: CodaHerdrTheme.amberNeedsYou) {
                    store.sendDirectText("2", submit: true, to: session)
                }
                approvalCapsuleButton("3", tint: CodaHerdrTheme.amberNeedsYou) {
                    store.sendDirectText("3", submit: true, to: session)
                }
                approvalCapsuleButton("Enter", tint: CodaHerdrTheme.cyanWorking) {
                    store.sendQuickKey(.enter, to: session)
                }
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 6)
        }
        .background(
            Capsule()
                .fill(CodaHerdrTheme.elevatedBG.opacity(0.97))
                .overlay(Capsule().strokeBorder(CodaHerdrTheme.amberNeedsYou.opacity(0.65), lineWidth: 1.2))
                .shadow(color: .black.opacity(0.5), radius: 14, x: 0, y: 6)
        )
    }

    private func approvalCapsuleButton(
        _ title: String,
        tint: Color,
        action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            Text(title)
                .font(.system(size: 11.5, weight: .semibold, design: .monospaced))
                .foregroundStyle(tint)
                .padding(.horizontal, 10)
                .padding(.vertical, 6)
                .background(
                    Capsule()
                        .fill(tint.opacity(0.14))
                        .overlay(Capsule().strokeBorder(tint.opacity(0.45), lineWidth: 1))
                )
        }
        .buttonStyle(.plain)
    }

    private var directKeystrokeStrip: some View {
        let isVoiceActiveHere = (store.isRecordingVoice || store.isTranscribingVoice)
            && (store.voiceDictationSessionID == nil || store.voiceDictationSessionID == session.id)
        return VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 5) {
                Button {
                    withAnimation(.snappy(duration: 0.16)) {
                        local.useMultiLineComposer.toggle()
                    }
                    directKeyboardFocused = true
                } label: {
                    HStack(spacing: 4) {
                        Image(systemName: local.useMultiLineComposer ? "text.alignleft" : "terminal")
                            .font(.system(size: 10.5, weight: .semibold))
                        Text(
                            local.useMultiLineComposer
                                ? tr("Composer", "多行编排", locale: store.locale)
                                : tr("Direct", "单行按键", locale: store.locale)
                        )
                        .font(.system(size: 10.5, weight: .semibold))
                    }
                    .foregroundStyle(local.useMultiLineComposer ? CodaHerdrTheme.cyanWorking : CodaHerdrTheme.textSecondary)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 3.5)
                    .background(
                        Capsule()
                            .fill(local.useMultiLineComposer ? CodaHerdrTheme.cyanWorking.opacity(0.14) : CodaHerdrTheme.cardBG)
                    )
                }
                .buttonStyle(.plain)

                if store.ctrlActive {
                    modifierChip("CTRL")
                }
                if store.altActive {
                    modifierChip("ALT")
                }

                Spacer(minLength: 4)

                #if canImport(PhotosUI)
                PhotosPicker(selection: $local.selectedPhotoItem, matching: .images) {
                    Image(systemName: "photo")
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(CodaHerdrTheme.textPrimary)
                        .padding(.horizontal, 7)
                        .padding(.vertical, 4)
                        .background(Capsule().fill(CodaHerdrTheme.cardBG))
                }
                .buttonStyle(.plain)
                #endif

                Button {
                    pasteClipboardIntoDirectBar()
                } label: {
                    Image(systemName: "doc.on.clipboard")
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(CodaHerdrTheme.textPrimary)
                        .padding(.horizontal, 7)
                        .padding(.vertical, 4)
                        .background(Capsule().fill(CodaHerdrTheme.cardBG))
                }
                .buttonStyle(.plain)

                Button {
                    local.useMultiLineComposer = true
                    Task {
                        await store.toggleVoiceDictation(for: session.id) { transcript in
                            appendTokenToDirectInput(transcript, trailingSpace: false)
                        }
                    }
                } label: {
                    HStack(spacing: 4) {
                        if isVoiceActiveHere && store.isTranscribingVoice {
                            ProgressView().controlSize(.mini).tint(CodaHerdrTheme.cyanWorking)
                        } else {
                            Image(systemName: (isVoiceActiveHere && store.isRecordingVoice) ? "stop.circle.fill" : "mic.fill")
                                .font(.system(size: 11, weight: .semibold))
                        }
                        if isVoiceActiveHere && store.isRecordingVoice {
                            Text(String(format: "%d:%02d", store.voiceRecordingSeconds / 60, store.voiceRecordingSeconds % 60))
                                .font(.system(size: 10.5, weight: .bold, design: .monospaced))
                            Capsule()
                                .fill(Color.white.opacity(0.9))
                                .frame(width: max(4, CGFloat(store.voiceAudioLevel) * 14), height: 6)
                        } else {
                            Text(tr("Dictate", "语音", locale: store.locale))
                                .font(.system(size: 10.5, weight: .semibold))
                        }
                    }
                    .foregroundStyle((isVoiceActiveHere && store.isRecordingVoice) ? .white : CodaHerdrTheme.textPrimary)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 3.5)
                    .background(
                        Capsule()
                            .fill((isVoiceActiveHere && store.isRecordingVoice) ? CodaHerdrTheme.redDiff : CodaHerdrTheme.cardBG)
                    )
                }
                .buttonStyle(.plain)
                .disabled(store.isTranscribingVoice)

                Button {
                    directKeyboardFocused = false
                    store.cancelVoiceDictation()
                    withAnimation(.snappy(duration: 0.16)) {
                        local.showDirectInputBar = false
                    }
                } label: {
                    Image(systemName: "xmark.circle.fill")
                        .font(.system(size: 15))
                        .foregroundStyle(CodaHerdrTheme.textDim)
                }
                .buttonStyle(.plain)
            }

            if isVoiceActiveHere && !store.voicePartialTranscript.isEmpty {
                Text(store.voicePartialTranscript)
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(CodaHerdrTheme.cyanWorking)
                    .lineLimit(2)
            }

            if let uploadMsg = store.uploadStatusBanner {
                Text(uploadMsg)
                    .font(.system(size: 10.5, design: .monospaced))
                    .foregroundStyle(store.uploadBannerIsError ? CodaHerdrTheme.redDiff : CodaHerdrTheme.cyanWorking)
                    .lineLimit(2)
            }

            if let voiceErr = store.voiceError {
                Text(voiceErr)
                    .font(.system(size: 11))
                    .foregroundStyle(CodaHerdrTheme.redDiff)
            }

            if local.useMultiLineComposer {
                CodaPromptSnippetsBarView(
                    store: store,
                    draftText: local.directInputBuffer,
                    compact: true,
                    onInsertSnippet: { snippetPrompt in
                        insertSnippetIntoDirectInput(snippetPrompt)
                    },
                    onSaveDraft: {
                        let draft = local.directInputBuffer
                        Task { _ = await store.saveDraftAsSnippet(draft) }
                    }
                )
            }

            HStack(alignment: .bottom, spacing: 8) {
                if local.useMultiLineComposer {
                    TextField(
                        tr(
                            "Compose multi-line prompt or review dictation...",
                            "编写或修改多行提示词...",
                            locale: store.locale
                        ),
                        text: $local.directInputBuffer,
                        axis: .vertical
                    )
                    .lineLimit(isCompactHeight ? 2...4 : 3...6)
                    .font(.system(size: 13, design: .monospaced))
                    .foregroundStyle(CodaHerdrTheme.textPrimary)
                    .focused($directKeyboardFocused)
                    .codaNoAutocapAndSpelling()
                } else {
                    TextField(
                        tr("Type command or keystroke (Enter to send)...", "输入指令或按键（回车发送）...", locale: store.locale),
                        text: $local.directInputBuffer
                    )
                    .font(.system(size: 13, design: .monospaced))
                    .foregroundStyle(CodaHerdrTheme.textPrimary)
                    .focused($directKeyboardFocused)
                    .codaNoAutocapAndSpelling()
                    .onChange(of: local.directInputBuffer) { oldValue, next in
                        guard store.ctrlActive || store.altActive else { return }
                        if next.count == oldValue.count + 1, next.hasPrefix(oldValue) {
                            let typed = String(next.dropFirst(oldValue.count))
                            local.directInputBuffer = oldValue
                            store.sendDirectText(typed, submit: false, to: session)
                        }
                    }
                    .onSubmit {
                        submitDirectOrComposerInput()
                    }
                }

                Button {
                    submitDirectOrComposerInput()
                } label: {
                    HStack(spacing: 4) {
                        if local.useMultiLineComposer {
                            Text(tr("Send", "发送", locale: store.locale))
                                .font(.system(size: 12, weight: .semibold))
                        }
                        Image(systemName: "arrow.up.circle.fill")
                            .font(.system(size: 20))
                    }
                    .foregroundStyle(local.useMultiLineComposer ? .black : CodaHerdrTheme.cyanWorking)
                    .padding(.horizontal, local.useMultiLineComposer ? 10 : 0)
                    .padding(.vertical, local.useMultiLineComposer ? 5 : 0)
                    .background(
                        Capsule()
                            .fill(local.useMultiLineComposer ? CodaHerdrTheme.cyanWorking : .clear)
                    )
                }
                .buttonStyle(.plain)
                .disabled(store.isUploadingAttachment)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 9)
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

    private func appendTokenToDirectInput(_ token: String, trailingSpace: Bool) {
        guard !token.isEmpty else { return }
        if local.directInputBuffer.isEmpty
            || local.directInputBuffer.hasSuffix(" ")
            || local.directInputBuffer.hasSuffix("\n") {
            local.directInputBuffer += token + (trailingSpace ? " " : "")
        } else {
            local.directInputBuffer += " " + token + (trailingSpace ? " " : "")
        }
    }

    private func insertSnippetIntoDirectInput(_ prompt: String) {
        let clean = prompt.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !clean.isEmpty else { return }
        if local.directInputBuffer.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            local.directInputBuffer = clean
        } else if local.directInputBuffer.hasSuffix("\n") {
            local.directInputBuffer += clean
        } else {
            local.directInputBuffer += "\n" + clean
        }
        CodaHaptics.lightTap()
    }

    private func pasteClipboardIntoDirectBar() {
        switch CodaClipboardBridge.readPreferredPasteContent() {
        case .text(let text):
            if text.contains("\n") {
                local.useMultiLineComposer = true
            }
            appendTokenToDirectInput(text, trailingSpace: false)
        case .imagePNG(let imgData):
            let filename = "clipboard-\(Int(Date().timeIntervalSince1970)).png"
            store.uploadAttachmentData(data: imgData, filename: filename, for: session) { inserted in
                appendTokenToDirectInput(inserted, trailingSpace: true)
            }
        case .empty:
            store.uploadBannerIsError = false
            store.uploadStatusBanner = tr("Clipboard is empty.", "剪贴板为空。", locale: store.locale)
        }
    }

    #if canImport(PhotosUI)
    private func uploadPickedPhotoItem(_ item: PhotosPickerItem, intoDirectInput: Bool) async {
        let ext = item.supportedContentTypes.first?.preferredFilenameExtension ?? "png"
        guard let data = try? await item.loadTransferable(type: Data.self), !data.isEmpty else {
            store.uploadBannerIsError = true
            store.uploadStatusBanner = tr("Could not load selected photo.", "无法读取所选照片。", locale: store.locale)
            return
        }
        let filename = "photo-\(Int(Date().timeIntervalSince1970)).\(ext)"
        if intoDirectInput {
            store.uploadAttachmentData(data: data, filename: filename, for: session) { inserted in
                appendTokenToDirectInput(inserted, trailingSpace: true)
            }
        } else {
            store.uploadAttachmentData(data: data, filename: filename, for: session)
        }
    }
    #endif

    private func submitDirectOrComposerInput() {
        let text = local.directInputBuffer
        local.directInputBuffer = ""
        if text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            if !local.useMultiLineComposer {
                store.sendQuickKey(.enter, to: session)
            }
        } else if local.useMultiLineComposer || text.contains("\n") {
            store.sendBracketedPastePrompt(text, to: session)
        } else {
            store.sendDirectText(text, submit: true, to: session)
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
                store.cancelVoiceDictation()
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

// MARK: - Cross-Device Saved Prompt Snippets Capsule Bar & Compose Sheet

public struct CodaPromptSnippetsBarView: View {
    @ObservedObject public var store: CodaWorkspaceStore
    public let draftText: String
    public let compact: Bool
    public let onInsertSnippet: (String) -> Void
    public let onSaveDraft: () -> Void
    public var onToggleManageList: (() -> Void)? = nil
    public var isManagingList: Bool = false

    public init(
        store: CodaWorkspaceStore,
        draftText: String,
        compact: Bool = false,
        onInsertSnippet: @escaping (String) -> Void,
        onSaveDraft: @escaping () -> Void,
        onToggleManageList: (() -> Void)? = nil,
        isManagingList: Bool = false
    ) {
        self.store = store
        self.draftText = draftText
        self.compact = compact
        self.onInsertSnippet = onInsertSnippet
        self.onSaveDraft = onSaveDraft
        self.onToggleManageList = onToggleManageList
        self.isManagingList = isManagingList
    }

    public var body: some View {
        let canSaveDraft = !draftText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty && !store.isSavingSnippets
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 6) {
                ForEach(store.promptSnippets) { snippet in
                    HStack(spacing: 4) {
                        Button {
                            onInsertSnippet(snippet.prompt)
                        } label: {
                            HStack(spacing: 4) {
                                Image(systemName: "sparkles")
                                    .font(.system(size: compact ? 9.5 : 10.5, weight: .semibold))
                                    .foregroundStyle(CodaHerdrTheme.cyanWorking)
                                Text(snippet.title)
                                    .font(.system(size: compact ? 11 : 12, weight: .medium))
                                    .foregroundStyle(CodaHerdrTheme.textPrimary)
                                    .lineLimit(1)
                            }
                        }
                        .buttonStyle(.plain)

                        Button {
                            Task { await store.deletePromptSnippet(id: snippet.id) }
                        } label: {
                            Image(systemName: "xmark")
                                .font(.system(size: compact ? 8.5 : 9.5, weight: .bold))
                                .foregroundStyle(CodaHerdrTheme.textDim)
                                .padding(2)
                        }
                        .buttonStyle(.plain)
                    }
                    .padding(.leading, compact ? 9 : 10)
                    .padding(.trailing, compact ? 6 : 7)
                    .padding(.vertical, compact ? 4.5 : 6)
                    .background(
                        Capsule()
                            .fill(CodaHerdrTheme.cardBG)
                            .overlay(Capsule().strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
                    )
                    .contextMenu {
                        Button {
                            onInsertSnippet(snippet.prompt)
                        } label: {
                            Label(tr("Insert into Composer", "插入到输入框", locale: store.locale), systemImage: "text.insert")
                        }
                        Button(role: .destructive) {
                            Task { await store.deletePromptSnippet(id: snippet.id) }
                        } label: {
                            Label(tr("Delete Snippet", "删除快捷片段", locale: store.locale), systemImage: "trash")
                        }
                    }
                }

                Button {
                    onSaveDraft()
                } label: {
                    HStack(spacing: 4) {
                        Image(systemName: "plus")
                            .font(.system(size: compact ? 9.5 : 10.5, weight: .bold))
                        Text(tr("Save Draft as Snippet", "存为快捷片段", locale: store.locale))
                            .font(.system(size: compact ? 11 : 11.5, weight: .semibold))
                            .lineLimit(1)
                    }
                    .foregroundStyle(canSaveDraft ? CodaHerdrTheme.cyanWorking : CodaHerdrTheme.textDim)
                    .padding(.horizontal, compact ? 9 : 11)
                    .padding(.vertical, compact ? 4.5 : 6)
                    .background(
                        Capsule()
                            .fill(canSaveDraft ? CodaHerdrTheme.cyanWorking.opacity(0.14) : CodaHerdrTheme.canvasBG)
                            .overlay(
                                Capsule().strokeBorder(
                                    canSaveDraft ? CodaHerdrTheme.cyanWorking.opacity(0.45) : CodaHerdrTheme.borderHairline,
                                    lineWidth: 1
                                )
                            )
                    )
                }
                .buttonStyle(.plain)
                .disabled(!canSaveDraft)

                if let onToggleManageList, !store.promptSnippets.isEmpty {
                    Button {
                        onToggleManageList()
                    } label: {
                        Image(systemName: isManagingList ? "chevron.up.circle.fill" : "slider.horizontal.below.rectangle")
                            .font(.system(size: 12, weight: .medium))
                            .foregroundStyle(isManagingList ? CodaHerdrTheme.cyanWorking : CodaHerdrTheme.textSecondary)
                            .padding(.horizontal, 8)
                            .padding(.vertical, 6)
                            .background(
                                Capsule()
                                    .fill(CodaHerdrTheme.elevatedBG)
                                    .overlay(Capsule().strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
                            )
                    }
                    .buttonStyle(.plain)
                }
            }
        }
    }
}

private final class ComposeSheetLocalState: ObservableObject {
    @Published var showFileImporter: Bool = false
    @Published var showManageSnippetsList: Bool = false
    #if canImport(PhotosUI)
    @Published var selectedPhotoItem: PhotosPickerItem? = nil
    #endif
}

public struct CodaComposePromptSheet: View {
    @ObservedObject public var store: CodaWorkspaceStore
    public let session: CodaAgentSession
    @Environment(\.dismiss) private var dismiss
    @StateObject private var local = ComposeSheetLocalState()

    private var sessionDraftBinding: Binding<String> {
        Binding(
            get: { store.composeDraft(for: session.id) },
            set: { store.setComposeDraft($0, for: session.id) }
        )
    }

    public var body: some View {
        let draftText = store.composeDraft(for: session.id)
        let isVoiceActiveHere = (store.isRecordingVoice || store.isTranscribingVoice)
            && (store.voiceDictationSessionID == nil || store.voiceDictationSessionID == session.id)

        return VStack(alignment: .leading, spacing: 12) {
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

            CodaPromptSnippetsBarView(
                store: store,
                draftText: draftText,
                compact: false,
                onInsertSnippet: { snippetPrompt in
                    store.insertSnippetPrompt(snippetPrompt, for: session.id)
                },
                onSaveDraft: {
                    let current = store.composeDraft(for: session.id)
                    Task { _ = await store.saveDraftAsSnippet(current) }
                },
                onToggleManageList: {
                    withAnimation(.snappy(duration: 0.16)) {
                        local.showManageSnippetsList.toggle()
                    }
                },
                isManagingList: local.showManageSnippetsList
            )

            if local.showManageSnippetsList && !store.promptSnippets.isEmpty {
                List {
                    ForEach(store.promptSnippets) { snippet in
                        Button {
                            store.insertSnippetPrompt(snippet.prompt, for: session.id)
                        } label: {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(snippet.title)
                                    .font(.system(size: 12.5, weight: .semibold))
                                    .foregroundStyle(CodaHerdrTheme.textPrimary)
                                Text(snippet.prompt)
                                    .font(.system(size: 11, design: .monospaced))
                                    .foregroundStyle(CodaHerdrTheme.textSecondary)
                                    .lineLimit(2)
                            }
                            .padding(.vertical, 2)
                        }
                        .buttonStyle(.plain)
                        .listRowBackground(CodaHerdrTheme.canvasBG)
                        .swipeActions(edge: .trailing, allowsFullSwipe: true) {
                            Button(role: .destructive) {
                                Task { await store.deletePromptSnippet(id: snippet.id) }
                            } label: {
                                Label(tr("Delete", "删除", locale: store.locale), systemImage: "trash")
                            }
                        }
                    }
                }
                .listStyle(.plain)
                .scrollContentBackground(.hidden)
                .frame(maxHeight: 132)
                .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
            }

            TextEditor(text: sessionDraftBinding)
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

            if isVoiceActiveHere && !store.voicePartialTranscript.isEmpty {
                Text(store.voicePartialTranscript)
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(CodaHerdrTheme.cyanWorking)
                    .lineLimit(2)
            }

            if let uploadBanner = store.uploadStatusBanner {
                HStack(spacing: 6) {
                    if store.isUploadingAttachment {
                        ProgressView().controlSize(.mini).tint(CodaHerdrTheme.cyanWorking)
                    } else if store.uploadBannerIsError {
                        Image(systemName: "exclamationmark.triangle.fill")
                            .font(.system(size: 11))
                            .foregroundStyle(CodaHerdrTheme.redDiff)
                    }
                    Text(uploadBanner)
                        .font(.system(size: 11, design: .monospaced))
                        .foregroundStyle(store.uploadBannerIsError ? CodaHerdrTheme.redDiff : CodaHerdrTheme.cyanWorking)
                        .lineLimit(2)
                }
            }

            if let voiceErr = store.voiceError {
                Text(voiceErr)
                    .font(.system(size: 11))
                    .foregroundStyle(CodaHerdrTheme.redDiff)
            }

            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 8) {
                    Button {
                        local.showFileImporter = true
                    } label: {
                        Label(tr("Files", "文件", locale: store.locale), systemImage: "paperclip")
                            .font(.system(size: 12, weight: .medium))
                            .foregroundStyle(CodaHerdrTheme.textPrimary)
                            .padding(.horizontal, 11)
                            .padding(.vertical, 8)
                            .background(
                                Capsule()
                                    .fill(CodaHerdrTheme.elevatedBG)
                                    .overlay(Capsule().strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
                            )
                    }
                    .buttonStyle(.plain)

                    #if canImport(PhotosUI)
                    let photosTitle = tr("Photos", "相册", locale: store.locale)
                    PhotosPicker(selection: $local.selectedPhotoItem, matching: .images) {
                        Label(photosTitle, systemImage: "photo.on.rectangle")
                            .font(.system(size: 12, weight: .medium))
                            .foregroundStyle(CodaHerdrTheme.textPrimary)
                            .padding(.horizontal, 11)
                            .padding(.vertical, 8)
                            .background(
                                Capsule()
                                    .fill(CodaHerdrTheme.elevatedBG)
                                    .overlay(Capsule().strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
                            )
                    }
                    .buttonStyle(.plain)
                    #endif

                    Button {
                        pasteClipboardIntoCompose()
                    } label: {
                        Label(tr("Paste", "粘贴", locale: store.locale), systemImage: "doc.on.clipboard")
                            .font(.system(size: 12, weight: .medium))
                            .foregroundStyle(CodaHerdrTheme.textPrimary)
                            .padding(.horizontal, 11)
                            .padding(.vertical, 8)
                            .background(
                                Capsule()
                                    .fill(CodaHerdrTheme.elevatedBG)
                                    .overlay(Capsule().strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
                            )
                    }
                    .buttonStyle(.plain)

                    Button {
                        Task { await store.toggleVoiceDictation(for: session.id) }
                    } label: {
                        HStack(spacing: 6) {
                            if isVoiceActiveHere && store.isTranscribingVoice {
                                ProgressView()
                                    .controlSize(.small)
                                    .tint(CodaHerdrTheme.cyanWorking)
                            } else {
                                Image(systemName: (isVoiceActiveHere && store.isRecordingVoice) ? "stop.circle.fill" : "mic.fill")
                            }
                            if isVoiceActiveHere && store.isRecordingVoice {
                                Text(String(format: "%d:%02d", store.voiceRecordingSeconds / 60, store.voiceRecordingSeconds % 60))
                                    .font(.system(size: 11.5, weight: .bold, design: .monospaced))
                                Capsule()
                                    .fill(Color.white.opacity(0.9))
                                    .frame(width: max(4, CGFloat(store.voiceAudioLevel) * 16), height: 7)
                            } else {
                                Text(
                                    (isVoiceActiveHere && store.isTranscribingVoice)
                                        ? tr("Transcribing...", "转写中...", locale: store.locale)
                                        : tr("Voice", "语音", locale: store.locale)
                                )
                            }
                        }
                        .font(.system(size: 12, weight: .medium))
                        .foregroundStyle(
                            (isVoiceActiveHere && store.isRecordingVoice)
                                ? .white
                                : ((isVoiceActiveHere && store.isTranscribingVoice) ? CodaHerdrTheme.cyanWorking : CodaHerdrTheme.textPrimary)
                        )
                        .padding(.horizontal, 11)
                        .padding(.vertical, 8)
                        .background(
                            Capsule()
                                .fill((isVoiceActiveHere && store.isRecordingVoice) ? CodaHerdrTheme.redDiff : CodaHerdrTheme.elevatedBG)
                                .overlay(
                                    Capsule().strokeBorder(
                                        (isVoiceActiveHere && store.isRecordingVoice) ? CodaHerdrTheme.redDiff : CodaHerdrTheme.borderHairline,
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
                }
            }

            HStack {
                Spacer()
                Button {
                    store.submitComposePrompt(for: session)
                    dismiss()
                } label: {
                    HStack(spacing: 6) {
                        if store.isUploadingAttachment {
                            ProgressView().controlSize(.small).tint(.black)
                        }
                        Text(store.isUploadingAttachment ? tr("Uploading…", "上传中…", locale: store.locale) : tr("Send", "发送", locale: store.locale))
                        Image(systemName: "arrow.up")
                    }
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(.black)
                    .padding(.horizontal, 18)
                    .padding(.vertical, 9)
                    .background(Capsule().fill(CodaHerdrTheme.textPrimary))
                }
                .buttonStyle(.plain)
                .disabled(
                    draftText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                        || store.isUploadingAttachment
                )
            }
        }
        .padding(18)
        .background(CodaHerdrTheme.cardBG)
        .presentationDetents([.medium, .large])
        .onAppear {
            Task { await store.loadPromptSnippets() }
        }
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
        #if canImport(PhotosUI)
        .onChange(of: local.selectedPhotoItem) { _, nextItem in
            guard let item = nextItem else { return }
            local.selectedPhotoItem = nil
            Task { @MainActor in
                let ext = item.supportedContentTypes.first?.preferredFilenameExtension ?? "png"
                guard let data = try? await item.loadTransferable(type: Data.self), !data.isEmpty else {
                    store.uploadBannerIsError = true
                    store.uploadStatusBanner = tr("Could not load selected photo.", "无法读取所选照片。", locale: store.locale)
                    return
                }
                let filename = "photo-\(Int(Date().timeIntervalSince1970)).\(ext)"
                store.uploadAttachmentData(data: data, filename: filename, for: session)
            }
        }
        #endif
    }

    private func pasteClipboardIntoCompose() {
        switch CodaClipboardBridge.readPreferredPasteContent() {
        case .text(let text):
            store.appendComposeDraft(text, for: session.id)
        case .imagePNG(let imgData):
            let filename = "clipboard-\(Int(Date().timeIntervalSince1970)).png"
            store.uploadAttachmentData(data: imgData, filename: filename, for: session)
        case .empty:
            store.uploadBannerIsError = false
            store.uploadStatusBanner = tr("Clipboard has no image or text.", "剪贴板中没有图片或文本。", locale: store.locale)
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
                        Divider()
                        Button {
                            dismiss()
                            store.openBootstrapWorkspaceSheet(for: device)
                        } label: {
                            Label(tr("New Workspace / Git Clone…", "新建目录 / Git Clone…", locale: store.locale), systemImage: "folder.badge.plus")
                        }
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
                Text("\(session.workspaceName) · Git Diff")
                    .font(.system(size: 16, weight: .semibold))
                    .foregroundStyle(CodaHerdrTheme.textPrimary)
                Spacer()
                Button(tr("Close", "关闭", locale: store.locale)) { dismiss() }
                    .foregroundStyle(CodaHerdrTheme.textSecondary)
            }

            Divider().background(CodaHerdrTheme.borderHairline)

            CodaGitDiffInteractiveView(
                store: store,
                session: session,
                onOpenFilePreview: { relPath in
                    let rawRepoRoot = store.gitDiffResult?.repoRoot?.trimmingCharacters(in: .whitespacesAndNewlines)
                    let repoRoot = (rawRepoRoot?.isEmpty == false)
                        ? rawRepoRoot
                        : store.matchedRepoRoot(for: session)
                    dismiss()
                    Task { @MainActor in
                        try? await Task.sleep(nanoseconds: 220_000_000)
                        store.activeSheetSessionID = session.id
                        store.inspectorSheetMode = .files
                        store.showRemoteFilesSheet = true
                        store.previewRemoteFile(relPath, for: session, baseRootOverride: repoRoot)
                    }
                }
            )
        }
        .padding(18)
        .background(CodaHerdrTheme.cardBG)
        .presentationDetents([.medium, .large])
    }
}
