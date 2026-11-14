import CodaGhosttyKit
import CodaKit
import Combine
import SwiftUI

// MARK: - Unfolded iPhone Duo Book-Fold Workbench (7.6" 50/50 Hinge-Aligned Split)

private final class DuoLeftLeafLocalState: ObservableObject {
    @Published var tab: CodaMobileTab = .inbox
}

public struct CodaDuoUnfoldedWorkbenchView: View {
    @ObservedObject public var store: CodaWorkspaceStore
    @StateObject private var leftState = DuoLeftLeafLocalState()

    public init(store: CodaWorkspaceStore) {
        self.store = store
    }

    public var body: some View {
        VStack(spacing: 0) {
            duoModeTopBar

            Divider()
                .background(CodaHerdrTheme.borderHairline)

            GeometryReader { geo in
                let leftLeafWidth = max(240, floor((geo.size.width - 1) * 0.5))

                if store.duoSplitMode == .fullPanorama {
                    if let active = store.selectedSession {
                        CodaTerminalSessionView(
                            store: store,
                            session: active,
                            isCompact: false,
                            isSplitThumb: true
                        )
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                    } else {
                        duoEmptyDetailPane
                    }
                } else {
                    HStack(spacing: 0) {
                        leftLeafView(geoWidth: leftLeafWidth)
                            .frame(width: leftLeafWidth, height: geo.size.height)
                            .clipped()

                        CodaDuoHingeDivider()

                        rightLeafView
                            .frame(maxWidth: .infinity, maxHeight: .infinity)
                            .clipped()
                    }
                }
            }
        }
        .background(CodaHerdrTheme.canvasBG)
    }

    private var duoModeTopBar: some View {
        HStack(spacing: 8) {
            HStack(spacing: 6) {
                Circle()
                    .fill(store.remoteMachines.contains(where: { $0.isOnline }) ? CodaHerdrTheme.greenDiff : CodaHerdrTheme.textDim)
                    .frame(width: 7, height: 7)

                Text("Coda Duo")
                    .font(.system(size: 13.5, weight: .bold))
                    .foregroundStyle(CodaHerdrTheme.textPrimary)

                if let route = store.activeRouteModeForSelected {
                    Text(route.rawValue)
                        .font(.system(size: 9.5, weight: .medium, design: .monospaced))
                        .foregroundStyle(CodaHerdrTheme.greenDiff)
                        .padding(.horizontal, 6)
                        .padding(.vertical, 2)
                        .background(
                            Capsule()
                                .fill(CodaHerdrTheme.cardBG)
                                .overlay(Capsule().strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
                        )
                        .lineLimit(1)
                }

                if store.attentionBadgeCount > 0 {
                    Text("\(store.attentionBadgeCount)")
                        .font(.system(size: 10, weight: .bold))
                        .foregroundStyle(.black)
                        .padding(.horizontal, 5.5)
                        .padding(.vertical, 1.5)
                        .background(Capsule().fill(CodaHerdrTheme.amberNeedsYou))
                }
            }

            Spacer(minLength: 4)

            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 3) {
                    ForEach(CodaDuoSplitMode.allCases) { mode in
                        let isSelected = store.duoSplitMode == mode
                        Button {
                            withAnimation(.snappy(duration: 0.18)) {
                                store.setDuoSplitMode(mode)
                            }
                        } label: {
                            Text(mode.title(locale: store.locale))
                                .font(.system(size: 11, weight: isSelected ? .semibold : .medium))
                                .foregroundStyle(isSelected ? CodaHerdrTheme.textPrimary : CodaHerdrTheme.textSecondary)
                                .padding(.horizontal, 9)
                                .padding(.vertical, 5)
                                .background(
                                    Capsule()
                                        .fill(isSelected ? CodaHerdrTheme.pillActiveBG : .clear)
                                        .overlay(
                                            Capsule().strokeBorder(
                                                isSelected ? CodaHerdrTheme.borderHairline : .clear,
                                                lineWidth: 1
                                            )
                                        )
                                )
                        }
                        .buttonStyle(.plain)
                    }
                }
                .padding(3)
                .background(
                    Capsule()
                        .fill(CodaHerdrTheme.canvasBG)
                        .overlay(Capsule().strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
                )
            }

            Spacer(minLength: 4)

            HStack(spacing: 6) {
                Button {
                    store.setLocale(store.locale == .en ? .zh : .en)
                } label: {
                    Text(store.locale == .en ? "EN" : "中文")
                        .font(.system(size: 10, weight: .semibold))
                        .foregroundStyle(CodaHerdrTheme.textSecondary)
                        .padding(.horizontal, 7)
                        .padding(.vertical, 4)
                        .background(
                            Capsule()
                                .fill(CodaHerdrTheme.elevatedBG)
                                .overlay(Capsule().strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
                        )
                }
                .buttonStyle(.plain)

                Button {
                    store.settingsInitialFocusPairing = false
                    store.showSettingsSheet = true
                } label: {
                    Image(systemName: "gearshape")
                        .font(.system(size: 13))
                        .foregroundStyle(CodaHerdrTheme.textPrimary)
                        .frame(width: 28, height: 28)
                        .background(
                            Circle()
                                .fill(CodaHerdrTheme.elevatedBG)
                                .overlay(Circle().strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
                        )
                }
                .buttonStyle(.plain)
            }
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 7)
        .background(CodaHerdrTheme.cardBG)
    }

    @ViewBuilder
    private func leftLeafView(geoWidth: CGFloat) -> some View {
        switch store.duoSplitMode {
        case .inboxAndTerminal:
            duoCompactDirectoryLeaf
        case .studioDiffSplit, .dualTerminalSplit, .fullPanorama:
            if let active = store.selectedSession {
                CodaTerminalSessionView(
                    store: store,
                    session: active,
                    isCompact: false
                )
            } else {
                duoEmptyDetailPane
            }
        }
    }

    @ViewBuilder
    private var rightLeafView: some View {
        switch store.duoSplitMode {
        case .inboxAndTerminal:
            if let active = store.selectedSession {
                CodaTerminalSessionView(
                    store: store,
                    session: active,
                    isCompact: false
                )
            } else {
                duoEmptyDetailPane
            }
        case .studioDiffSplit:
            if let active = store.selectedSession {
                CodaDuoInlineGitDiffLeaf(store: store, session: active)
            } else {
                duoEmptyDetailPane
            }
        case .dualTerminalSplit:
            if let secondary = store.secondarySession {
                VStack(spacing: 0) {
                    dualSecondaryPickerBar(current: secondary)
                    Divider().background(CodaHerdrTheme.borderHairline)
                    CodaTerminalSessionView(
                        store: store,
                        session: secondary,
                        isCompact: false,
                        isSplitThumb: false,
                        isSecondaryLeaf: true
                    )
                }
            } else {
                duoEmptyDetailPane
            }
        case .fullPanorama:
            EmptyView()
        }
    }

    private func dualSecondaryPickerBar(current: CodaAgentSession) -> some View {
        HStack(spacing: 6) {
            Text(tr("LEAF 2", "副屏终端", locale: store.locale))
                .font(.system(size: 10, weight: .semibold, design: .monospaced))
                .foregroundStyle(CodaHerdrTheme.textDim)

            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 5) {
                    ForEach(store.filteredSessions) { sess in
                        let selected = sess.id == current.id
                        Button {
                            store.selectSecondarySession(sess)
                        } label: {
                            HStack(spacing: 4) {
                                Circle()
                                    .fill(sess.state == .needsYou ? CodaHerdrTheme.amberNeedsYou : (sess.state == .working ? CodaHerdrTheme.cyanWorking : CodaHerdrTheme.textDim))
                                    .frame(width: 6, height: 6)
                                Text("\(sess.workspaceName)/\(sess.agentName)")
                                    .font(.system(size: 11, weight: selected ? .semibold : .regular))
                                    .foregroundStyle(selected ? CodaHerdrTheme.textPrimary : CodaHerdrTheme.textSecondary)
                                    .lineLimit(1)
                            }
                            .padding(.horizontal, 8)
                            .padding(.vertical, 4)
                            .background(
                                Capsule()
                                    .fill(selected ? CodaHerdrTheme.pillActiveBG : CodaHerdrTheme.canvasBG)
                                    .overlay(Capsule().strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
                            )
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .background(CodaHerdrTheme.cardBG.opacity(0.9))
    }

    private var duoCompactDirectoryLeaf: some View {
        VStack(spacing: 0) {
            HStack(spacing: 6) {
                HStack(spacing: 3) {
                    directoryTabButton(.inbox, title: tr("Inbox", "收件箱", locale: store.locale))
                    directoryTabButton(.spaces, title: tr("Spaces", "空间", locale: store.locale))
                }
                .padding(3)
                .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.canvasBG))

                Spacer()

                Button {
                    Task { await store.refreshMachinesAndSessions() }
                } label: {
                    Image(systemName: "arrow.clockwise")
                        .font(.system(size: 12))
                        .foregroundStyle(CodaHerdrTheme.textSecondary)
                        .frame(width: 26, height: 26)
                        .background(Circle().fill(CodaHerdrTheme.elevatedBG))
                }
                .buttonStyle(.plain)
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 8)

            HStack(spacing: 7) {
                Image(systemName: "magnifyingglass")
                    .font(.system(size: 12))
                    .foregroundStyle(CodaHerdrTheme.textSecondary)
                TextField(
                    tr("Jump to a terminal...", "跳转到终端...", locale: store.locale),
                    text: $store.searchQuery
                )
                .font(.system(size: 12.5))
                .foregroundStyle(CodaHerdrTheme.textPrimary)
                .codaNoAutocapAndSpelling()
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 7)
            .background(
                RoundedRectangle(cornerRadius: 9, style: .continuous)
                    .fill(CodaHerdrTheme.canvasBG)
                    .overlay(RoundedRectangle(cornerRadius: 9, style: .continuous).strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
            )
            .padding(.horizontal, 10)
            .padding(.bottom, 8)

            ScrollView {
                LazyVStack(alignment: .leading, spacing: 10) {
                    if leftState.tab == .inbox {
                        if store.groupedInboxSections.isEmpty {
                            duoSignedOutOrEmptyCard
                        } else {
                            ForEach(store.groupedInboxSections) { section in
                                Text(section.title.uppercased())
                                    .font(.system(size: 10.5, weight: .semibold))
                                    .tracking(0.5)
                                    .foregroundStyle(section.accentKind == .needsYou ? CodaHerdrTheme.amberNeedsYou : CodaHerdrTheme.textSecondary)
                                    .padding(.horizontal, 4)

                                ForEach(section.sessions) { session in
                                    duoInboxSessionCard(session)
                                }
                            }
                        }
                    } else {
                        if store.spacesByMachine.isEmpty {
                            duoSignedOutOrEmptyCard
                        } else {
                            ForEach(store.spacesByMachine) { group in
                                duoMachineCard(group)
                            }
                        }
                    }
                }
                .padding(.horizontal, 10)
                .padding(.bottom, 12)
            }

            Divider().background(CodaHerdrTheme.borderHairline)

            HStack(spacing: 8) {
                Button {
                    store.settingsInitialFocusPairing = false
                    store.showSettingsSheet = true
                } label: {
                    Label(tr("Settings", "设置", locale: store.locale), systemImage: "gearshape")
                        .font(.system(size: 11.5, weight: .medium))
                        .foregroundStyle(CodaHerdrTheme.textSecondary)
                        .padding(.horizontal, 8)
                        .padding(.vertical, 5)
                        .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.elevatedBG))
                }
                .buttonStyle(.plain)

                Spacer()

                Button {
                    store.settingsInitialFocusPairing = true
                    store.showSettingsSheet = true
                } label: {
                    Label(tr("Add machine", "添加主机", locale: store.locale), systemImage: "plus")
                        .font(.system(size: 11.5, weight: .semibold))
                        .foregroundStyle(CodaHerdrTheme.textPrimary)
                        .padding(.horizontal, 9)
                        .padding(.vertical, 5)
                        .background(
                            RoundedRectangle(cornerRadius: 8)
                                .fill(CodaHerdrTheme.elevatedBG)
                                .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
                        )
                }
                .buttonStyle(.plain)
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 8)
        }
        .background(CodaHerdrTheme.cardBG)
    }

    private func directoryTabButton(_ tab: CodaMobileTab, title: String) -> some View {
        let active = leftState.tab == tab
        return Button {
            leftState.tab = tab
        } label: {
            Text(title)
                .font(.system(size: 11.5, weight: active ? .semibold : .medium))
                .foregroundStyle(active ? CodaHerdrTheme.textPrimary : CodaHerdrTheme.textSecondary)
                .padding(.horizontal, 10)
                .padding(.vertical, 5)
                .background(
                    RoundedRectangle(cornerRadius: 6, style: .continuous)
                        .fill(active ? CodaHerdrTheme.pillActiveBG : .clear)
                )
        }
        .buttonStyle(.plain)
    }

    private func duoInboxSessionCard(_ session: CodaAgentSession) -> some View {
        let isSelected = store.selectedSessionID == session.id
        return Button {
            store.selectSession(session)
        } label: {
            HStack(spacing: 10) {
                CodaWorkspaceAvatar(name: session.workspaceName, size: 30)
                VStack(alignment: .leading, spacing: 2) {
                    HStack(spacing: 5) {
                        Text(session.workspaceName)
                            .font(.system(size: 12.5, weight: .semibold))
                            .foregroundStyle(CodaHerdrTheme.textPrimary)
                        Text(session.taskTitle)
                            .font(.system(size: 12.5))
                            .foregroundStyle(CodaHerdrTheme.textPrimary.opacity(0.86))
                            .lineLimit(1)
                    }
                    Text("\(session.agentName) · \(session.branch) · \(session.hostName)")
                        .font(.system(size: 11))
                        .foregroundStyle(CodaHerdrTheme.textSecondary)
                        .lineLimit(1)
                }
                Spacer(minLength: 4)
                CodaSessionStatusBadge(session: session, locale: store.locale)
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 9)
            .background(
                RoundedRectangle(cornerRadius: 10, style: .continuous)
                    .fill(isSelected ? CodaHerdrTheme.pillActiveBG : CodaHerdrTheme.canvasBG.opacity(0.7))
                    .overlay(
                        RoundedRectangle(cornerRadius: 10, style: .continuous)
                            .strokeBorder(
                                isSelected
                                    ? CodaHerdrTheme.cyanWorking.opacity(0.55)
                                    : (session.state == .needsYou ? CodaHerdrTheme.amberNeedsYou.opacity(0.45) : CodaHerdrTheme.borderHairline),
                                lineWidth: 1
                            )
                    )
            )
        }
        .buttonStyle(.plain)
    }

    private func duoMachineCard(_ group: CodaMachineSpacesGroup) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Circle()
                    .fill(group.device.isOnline ? CodaHerdrTheme.greenDiff : CodaHerdrTheme.textDim)
                    .frame(width: 7, height: 7)
                Text(group.device.name)
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(CodaHerdrTheme.textPrimary)
                Spacer()
                if let route = group.routeMode {
                    Text(route.rawValue)
                        .font(.system(size: 10, design: .monospaced))
                        .foregroundStyle(CodaHerdrTheme.cyanWorking)
                }
            }

            ForEach(group.repos) { repo in
                HStack(spacing: 8) {
                    CodaWorkspaceAvatar(name: repo.repoName, size: 22)
                    Text(repo.repoName)
                        .font(.system(size: 12, weight: .medium))
                        .foregroundStyle(CodaHerdrTheme.textPrimary)
                    Spacer()
                    if let b = repo.branches.first {
                        Text("\(b.branch) +\(b.diffAdded)/-\(b.diffDeleted)")
                            .font(.system(size: 10.5, design: .monospaced))
                            .foregroundStyle(CodaHerdrTheme.greenDiff)
                    }
                }
                .padding(.horizontal, 8)
                .padding(.vertical, 6)
                .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.canvasBG))
            }
        }
        .padding(8)
        .background(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .fill(CodaHerdrTheme.canvasBG.opacity(0.55))
                .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
        )
    }

    private var duoSignedOutOrEmptyCard: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(store.isSignedIn
                ? tr("No active agent sessions", "暂无活跃 Agent 会话", locale: store.locale)
                : tr("Sign in & pair a workstation", "登录并配对远端工作站", locale: store.locale))
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(CodaHerdrTheme.textPrimary)

            Button {
                store.settingsInitialFocusPairing = store.isSignedIn
                store.showSettingsSheet = true
            } label: {
                Text(store.isSignedIn ? tr("Add machine", "添加主机", locale: store.locale) : tr("Open Settings", "打开设置", locale: store.locale))
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(.black)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 6)
                    .background(Capsule().fill(CodaHerdrTheme.textPrimary))
            }
            .buttonStyle(.plain)
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .fill(CodaHerdrTheme.canvasBG)
                .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
        )
    }

    private var duoEmptyDetailPane: some View {
        VStack(spacing: 10) {
            Image(systemName: "rectangle.split.2x1")
                .font(.system(size: 26))
                .foregroundStyle(CodaHerdrTheme.textSecondary)
            Text(tr("Select an active terminal on the left leaf", "请在左侧分屏中选择一个活跃终端", locale: store.locale))
                .font(.system(size: 13))
                .foregroundStyle(CodaHerdrTheme.textSecondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(CodaHerdrTheme.canvasBG)
    }
}

// MARK: - 1pt Physical Fold / Hinge Crease Hairline

public struct CodaDuoHingeDivider: View {
    public init() {}

    public var body: some View {
        Rectangle()
            .fill(CodaHerdrTheme.borderHairline)
            .frame(width: 1)
            .overlay(
                LinearGradient(
                    colors: [.clear, CodaHerdrTheme.cyanWorking.opacity(0.22), .clear],
                    startPoint: .top,
                    endPoint: .bottom
                )
            )
    }
}

// MARK: - Right-Leaf Inline Studio Git Diff Inspector (Unfolded iPhone Duo)

public struct CodaDuoInlineGitDiffLeaf: View {
    @ObservedObject public var store: CodaWorkspaceStore
    public let session: CodaAgentSession

    public init(store: CodaWorkspaceStore, session: CodaAgentSession) {
        self.store = store
        self.session = session
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                VStack(alignment: .leading, spacing: 2) {
                    HStack(spacing: 6) {
                        Text("\(session.workspaceName) · Studio Git Diff")
                            .font(.system(size: 13.5, weight: .semibold))
                            .foregroundStyle(CodaHerdrTheme.textPrimary)
                        if session.diffAdded > 0 || session.diffDeleted > 0 {
                            Text("+\(session.diffAdded) -\(session.diffDeleted)")
                                .font(.system(size: 11, weight: .semibold, design: .monospaced))
                                .foregroundStyle(CodaHerdrTheme.greenDiff)
                        }
                    }
                    Text(store.gitDiffResult?.branch ?? session.branch)
                        .font(.system(size: 11, design: .monospaced))
                        .foregroundStyle(CodaHerdrTheme.textSecondary)
                }

                Spacer()

                Button {
                    store.loadGitDiffForSelected(presentModal: false)
                } label: {
                    Image(systemName: "arrow.clockwise")
                        .font(.system(size: 12, weight: .medium))
                        .foregroundStyle(CodaHerdrTheme.textPrimary)
                        .frame(width: 28, height: 28)
                        .background(
                            RoundedRectangle(cornerRadius: 7, style: .continuous)
                                .fill(CodaHerdrTheme.elevatedBG)
                                .overlay(RoundedRectangle(cornerRadius: 7, style: .continuous).strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
                        )
                }
                .buttonStyle(.plain)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 10)
            .background(CodaHerdrTheme.cardBG.opacity(0.65))

            Divider().background(CodaHerdrTheme.borderHairline)

            if store.gitDiffLoading {
                ProgressView()
                    .tint(CodaHerdrTheme.cyanWorking)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if let diff = store.gitDiffResult {
                ScrollView {
                    VStack(alignment: .leading, spacing: 3) {
                        if !diff.statusShort.isEmpty {
                            Text(diff.statusShort)
                                .font(.system(size: 11, design: .monospaced))
                                .foregroundStyle(CodaHerdrTheme.cyanWorking)
                                .padding(.bottom, 8)
                        }
                        let rawLines = (diff.diffUnified.isEmpty
                            ? tr("Working tree clean", "工作区无未提交变更", locale: store.locale)
                            : diff.diffUnified).components(separatedBy: "\n")

                        ForEach(Array(rawLines.enumerated()), id: \.offset) { _, line in
                            let isAdd = line.hasPrefix("+") && !line.hasPrefix("+++")
                            let isDel = line.hasPrefix("-") && !line.hasPrefix("---")
                            let isHunk = line.hasPrefix("@@")
                            let fg: Color = isAdd
                                ? CodaHerdrTheme.greenDiff
                                : (isDel ? CodaHerdrTheme.redDiff : (isHunk ? CodaHerdrTheme.cyanWorking : CodaHerdrTheme.textPrimary))
                            let bg: Color = isAdd
                                ? CodaHerdrTheme.greenDiff.opacity(0.10)
                                : (isDel ? CodaHerdrTheme.redDiff.opacity(0.10) : (isHunk ? CodaHerdrTheme.cyanWorking.opacity(0.08) : .clear))

                            Text(line.isEmpty ? " " : line)
                                .font(.system(size: 11, design: .monospaced))
                                .foregroundStyle(fg)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .padding(.horizontal, 6)
                                .padding(.vertical, 1)
                                .background(RoundedRectangle(cornerRadius: 3).fill(bg))
                        }
                    }
                    .padding(12)
                }
            } else {
                VStack(spacing: 8) {
                    Text(tr("Tap refresh to load remote diff over E2EE", "点击右上角刷新按钮通过加密通道拉取远程变更", locale: store.locale))
                        .font(.system(size: 12))
                        .foregroundStyle(CodaHerdrTheme.textSecondary)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .background(CodaHerdrTheme.canvasBG)
    }
}

// MARK: - 2-Row Dedicated Tactile Terminal Keypad (Cover & Unfolded Duo)

public struct CodaDuoTactileTwoRowKeypad: View {
    @ObservedObject public var store: CodaWorkspaceStore
    public let targetSession: CodaAgentSession?

    public init(store: CodaWorkspaceStore, targetSession: CodaAgentSession? = nil) {
        self.store = store
        self.targetSession = targetSession
    }

    public var body: some View {
        HStack(spacing: 5) {
            VStack(spacing: 5) {
                HStack(spacing: 5) {
                    tactileKey("esc") { store.sendQuickKey(.esc, to: targetSession) }
                    tactileToggleKey("ctrl", active: store.ctrlActive) { store.ctrlActive.toggle() }
                    tactileKey("tab") { store.sendQuickKey(.tab, to: targetSession) }
                    tactileKey("⇧tab") { store.sendQuickKey(.shiftTab, to: targetSession) }
                    tactileKey("↑") { store.sendQuickKey(.up, to: targetSession) }
                    tactileKey("⌫") { store.sendQuickKey(.backspace, to: targetSession) }
                }

                HStack(spacing: 5) {
                    tactileKey("^C", tint: CodaHerdrTheme.redDiff) { store.sendQuickKey(.ctrlC, to: targetSession) }
                    tactileKey("^D") { store.sendQuickKey(.ctrlD, to: targetSession) }
                    tactileKey("/") { store.sendQuickKey(.slash, to: targetSession) }
                    tactileKey("←") { store.sendQuickKey(.left, to: targetSession) }
                    tactileKey("↓") { store.sendQuickKey(.down, to: targetSession) }
                    tactileKey("→") { store.sendQuickKey(.right, to: targetSession) }
                }
            }

            Button {
                store.sendQuickKey(.enter, to: targetSession)
            } label: {
                Image(systemName: "return")
                    .font(.system(size: 16, weight: .bold))
                    .foregroundStyle(.black)
                    .frame(width: 44, height: 73)
                    .background(
                        RoundedRectangle(cornerRadius: 10, style: .continuous)
                            .fill(CodaHerdrTheme.cyanWorking)
                    )
            }
            .buttonStyle(.plain)
        }
        .padding(7)
        .background(
            RoundedRectangle(cornerRadius: 14, style: .continuous)
                .fill(CodaHerdrTheme.cardBG.opacity(0.97))
                .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
                .shadow(color: .black.opacity(0.55), radius: 14, x: 0, y: 6)
        )
    }

    private func tactileKey(_ label: String, tint: Color = CodaHerdrTheme.textPrimary, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(label)
                .font(.system(size: 11.5, weight: .semibold, design: .monospaced))
                .foregroundStyle(tint)
                .frame(maxWidth: .infinity)
                .frame(height: 34)
                .background(
                    RoundedRectangle(cornerRadius: 8, style: .continuous)
                        .fill(CodaHerdrTheme.elevatedBG)
                        .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
                )
        }
        .buttonStyle(.plain)
    }

    private func tactileToggleKey(_ label: String, active: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(label)
                .font(.system(size: 11.5, weight: .semibold, design: .monospaced))
                .foregroundStyle(active ? .black : CodaHerdrTheme.textPrimary)
                .frame(maxWidth: .infinity)
                .frame(height: 34)
                .background(
                    RoundedRectangle(cornerRadius: 8, style: .continuous)
                        .fill(active ? CodaHerdrTheme.cyanWorking : CodaHerdrTheme.elevatedBG)
                        .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(active ? CodaHerdrTheme.cyanWorking : CodaHerdrTheme.borderHairline, lineWidth: 1))
                )
        }
        .buttonStyle(.plain)
    }
}

// MARK: - 5.4" Folded Cover Screen Single-Hand Vertical Thumb Action Rail

public struct CodaCoverThumbRail: View {
    @ObservedObject public var store: CodaWorkspaceStore
    public let session: CodaAgentSession

    public init(store: CodaWorkspaceStore, session: CodaAgentSession) {
        self.store = store
        self.session = session
    }

    public var body: some View {
        VStack(spacing: 6) {
            if session.state == .needsYou {
                Button {
                    store.sendDirectText("1", submit: true, to: session)
                } label: {
                    Text("1")
                        .font(.system(size: 12, weight: .bold, design: .monospaced))
                        .foregroundStyle(.black)
                        .frame(width: 34, height: 30)
                        .background(Capsule().fill(CodaHerdrTheme.amberNeedsYou))
                }
                .buttonStyle(.plain)
            }

            thumbIconButton("chevron.up") {
                store.sendQuickKey(.up, to: session)
            }
            thumbIconButton("chevron.down") {
                store.sendQuickKey(.down, to: session)
            }
            thumbIconButton("return", tint: CodaHerdrTheme.cyanWorking) {
                store.sendQuickKey(.enter, to: session)
            }
            Button {
                store.sendQuickKey(.esc, to: session)
            } label: {
                Text("esc")
                    .font(.system(size: 9.5, weight: .bold, design: .monospaced))
                    .foregroundStyle(CodaHerdrTheme.redDiff)
                    .frame(width: 34, height: 28)
                    .background(Capsule().fill(CodaHerdrTheme.canvasBG.opacity(0.85)))
            }
            .buttonStyle(.plain)
        }
        .padding(.vertical, 6)
        .padding(.horizontal, 4)
        .background(
            Capsule()
                .fill(CodaHerdrTheme.elevatedBG.opacity(0.92))
                .overlay(Capsule().strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
                .shadow(color: .black.opacity(0.5), radius: 12, x: 0, y: 4)
        )
    }

    private func thumbIconButton(_ symbol: String, tint: Color = CodaHerdrTheme.textPrimary, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: symbol)
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(tint)
                .frame(width: 34, height: 28)
                .background(Circle().fill(CodaHerdrTheme.canvasBG.opacity(0.75)))
        }
        .buttonStyle(.plain)
    }
}
