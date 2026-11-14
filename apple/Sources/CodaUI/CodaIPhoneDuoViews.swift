import CodaGhosttyKit
import CodaKit
import Combine
import SwiftUI

// MARK: - iOS 27 Structural Split Around Physical Hinge / `.division` Reserved Region

public struct CodaFoldCreaseGutter: View {
    public let axis: Axis
    public let thickness: CGFloat

    public init(axis: Axis, thickness: CGFloat = 24) {
        self.axis = axis
        self.thickness = thickness
    }

    public var body: some View {
        ZStack {
            Rectangle()
                .fill(CodaHerdrTheme.canvasBG)

            if axis == .vertical {
                LinearGradient(
                    colors: [.clear, Color.black.opacity(0.45), .clear],
                    startPoint: .leading,
                    endPoint: .trailing
                )
                Rectangle()
                    .fill(CodaHerdrTheme.borderHairline.opacity(0.85))
                    .frame(width: 1)
                    .overlay(
                        LinearGradient(
                            colors: [.clear, CodaHerdrTheme.cyanWorking.opacity(0.28), .clear],
                            startPoint: .top,
                            endPoint: .bottom
                        )
                    )
            } else {
                LinearGradient(
                    colors: [.clear, Color.black.opacity(0.45), .clear],
                    startPoint: .top,
                    endPoint: .bottom
                )
                HStack(spacing: 8) {
                    Rectangle()
                        .fill(CodaHerdrTheme.borderHairline.opacity(0.9))
                        .frame(height: 1)
                    Text("FOLD CREASE")
                        .font(.system(size: 8.5, weight: .semibold, design: .monospaced))
                        .foregroundStyle(CodaHerdrTheme.textDim)
                    Rectangle()
                        .fill(CodaHerdrTheme.borderHairline.opacity(0.9))
                        .frame(height: 1)
                }
                .padding(.horizontal, 14)
            }
        }
        .frame(
            width: axis == .vertical ? thickness : nil,
            height: axis == .horizontal ? thickness : nil
        )
    }
}

/// Models iOS 27 `ArrangementView(.split)` by displacing primary and secondary panes
/// cleanly outside the 24pt `.division` fold crease centered on the hardware fold line.
public struct CodaArrangementSplitView<Primary: View, Secondary: View>: View {
    public let axis: Axis
    public let leadingOffset: CGFloat
    public let totalWindowSpan: CGFloat
    public let creaseThickness: CGFloat
    public let primary: Primary
    public let secondary: Secondary

    public init(
        axis: Axis,
        leadingOffset: CGFloat,
        totalWindowSpan: CGFloat,
        creaseThickness: CGFloat = 24,
        @ViewBuilder primary: () -> Primary,
        @ViewBuilder secondary: () -> Secondary
    ) {
        self.axis = axis
        self.leadingOffset = leadingOffset
        self.totalWindowSpan = totalWindowSpan
        self.creaseThickness = creaseThickness
        self.primary = primary()
        self.secondary = secondary()
    }

    public var body: some View {
        let mid = totalWindowSpan * 0.5
        let halfCrease = creaseThickness * 0.5
        let firstSpan = max(140, floor(mid - halfCrease - leadingOffset))

        if axis == .horizontal {
            HStack(spacing: 0) {
                primary
                    .frame(width: firstSpan)
                    .clipped()
                CodaFoldCreaseGutter(axis: .vertical, thickness: creaseThickness)
                secondary
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .clipped()
            }
        } else {
            VStack(spacing: 0) {
                primary
                    .frame(height: firstSpan)
                    .clipped()
                CodaFoldCreaseGutter(axis: .horizontal, thickness: creaseThickness)
                secondary
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .clipped()
            }
        }
    }
}

// MARK: - iOS 27 Vertical 84pt Side Toolbar Rail (Cover Landscape, Duo Inner Landscape, Slab Landscape)

public enum CodaSideRailKind: Sendable, Equatable {
    case duoInner
    case compactLandscape
}

public struct CodaVerticalSideToolbarRail: View {
    @ObservedObject public var store: CodaWorkspaceStore
    public let kind: CodaSideRailKind
    public let edge: HorizontalEdge

    public init(store: CodaWorkspaceStore, kind: CodaSideRailKind, edge: HorizontalEdge = .leading) {
        self.store = store
        self.kind = kind
        self.edge = edge
    }

    public var body: some View {
        let extraEdgePadding: CGFloat = edge == .leading
            ? store.edgeSafeAreas.leading
            : (store.edgeSafeAreas.trailing < 64 ? store.edgeSafeAreas.trailing : 0)

        return VStack(spacing: 8) {
            VStack(spacing: 4) {
                HStack(spacing: 4) {
                    Circle()
                        .fill(store.remoteMachines.contains(where: { $0.isOnline }) ? CodaHerdrTheme.greenDiff : CodaHerdrTheme.textDim)
                        .frame(width: 7, height: 7)
                    Text(kind == .duoInner ? "Duo" : "Coda")
                        .font(.system(size: 12, weight: .bold))
                        .foregroundStyle(CodaHerdrTheme.textPrimary)
                }

                Text("\(store.terminalColumns)×\(store.terminalRows)")
                    .font(.system(size: 9.5, weight: .medium, design: .monospaced))
                    .foregroundStyle(CodaHerdrTheme.greenDiff)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 2)
                    .background(
                        Capsule()
                            .fill(CodaHerdrTheme.canvasBG)
                            .overlay(Capsule().strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
                    )
            }
            .padding(.top, max(8, store.edgeSafeAreas.top + 4))

            Divider().background(CodaHerdrTheme.borderHairline)

            if kind == .duoInner {
                VStack(spacing: 5) {
                    ForEach(CodaDuoSplitMode.allCases) { mode in
                        let selected = store.duoSplitMode == mode
                        Button {
                            withAnimation(.snappy(duration: 0.18)) {
                                store.setDuoSplitMode(mode)
                            }
                        } label: {
                            VStack(spacing: 2) {
                                Image(systemName: iconForMode(mode))
                                    .font(.system(size: 13, weight: .semibold))
                                Text(mode.title(locale: store.locale))
                                    .font(.system(size: 10, weight: selected ? .semibold : .medium))
                            }
                            .foregroundStyle(selected ? CodaHerdrTheme.textPrimary : CodaHerdrTheme.textSecondary)
                            .frame(maxWidth: .infinity)
                            .padding(.vertical, 7)
                            .background(
                                RoundedRectangle(cornerRadius: 10, style: .continuous)
                                    .fill(selected ? CodaHerdrTheme.pillActiveBG : .clear)
                                    .overlay(
                                        RoundedRectangle(cornerRadius: 10, style: .continuous)
                                            .strokeBorder(selected ? CodaHerdrTheme.cyanWorking.opacity(0.45) : .clear, lineWidth: 1)
                                    )
                            )
                        }
                        .buttonStyle(.plain)
                    }
                }
            } else {
                VStack(spacing: 6) {
                    compactTabButton(.inbox, icon: "tray.fill", label: tr("Inbox", "收件", locale: store.locale), badge: store.attentionBadgeCount)
                    compactTabButton(.spaces, icon: "square.grid.2x2.fill", label: tr("Spaces", "空间", locale: store.locale), badge: 0)
                }
            }

            if !store.filteredSessions.isEmpty {
                Divider().background(CodaHerdrTheme.borderHairline)

                ScrollView(.vertical, showsIndicators: false) {
                    VStack(spacing: 6) {
                        ForEach(store.filteredSessions.prefix(5)) { s in
                            let active = store.selectedSessionID == s.id
                            Button {
                                store.selectSession(s)
                            } label: {
                                ZStack(alignment: .topTrailing) {
                                    CodaWorkspaceAvatar(name: s.workspaceName, size: 32)
                                        .overlay(
                                            RoundedRectangle(cornerRadius: 8, style: .continuous)
                                                .strokeBorder(active ? CodaHerdrTheme.cyanWorking : .clear, lineWidth: 1.5)
                                        )
                                    if s.state == .needsYou {
                                        Circle()
                                            .fill(CodaHerdrTheme.amberNeedsYou)
                                            .frame(width: 8, height: 8)
                                            .offset(x: 2, y: -2)
                                    } else if s.state == .working {
                                        Circle()
                                            .fill(CodaHerdrTheme.cyanWorking)
                                            .frame(width: 8, height: 8)
                                            .offset(x: 2, y: -2)
                                    }
                                }
                            }
                            .buttonStyle(.plain)
                        }
                    }
                    .padding(.vertical, 2)
                }
            }

            Spacer(minLength: 4)

            VStack(spacing: 6) {
                Button {
                    store.setLocale(store.locale == .en ? .zh : .en)
                } label: {
                    Text(store.locale == .en ? "EN" : "中文")
                        .font(.system(size: 10, weight: .semibold))
                        .foregroundStyle(CodaHerdrTheme.textSecondary)
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 5)
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
                        .font(.system(size: 14, weight: .medium))
                        .foregroundStyle(CodaHerdrTheme.textPrimary)
                        .frame(width: 34, height: 34)
                        .background(
                            Circle()
                                .fill(CodaHerdrTheme.elevatedBG)
                                .overlay(Circle().strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
                        )
                }
                .buttonStyle(.plain)
            }
            .padding(.bottom, max(10, store.edgeSafeAreas.bottom + 4))
        }
        .padding(.horizontal, 6)
        .padding(edge == .leading ? .leading : .trailing, extraEdgePadding)
        .frame(width: CodaEdgeSafeAreas.duoSideRailWidth + extraEdgePadding)
        .frame(maxHeight: .infinity)
        .background(CodaHerdrTheme.cardBG)
        .overlay(alignment: edge == .leading ? .trailing : .leading) {
            Rectangle()
                .fill(CodaHerdrTheme.borderHairline)
                .frame(width: 1)
        }
    }

    private func iconForMode(_ mode: CodaDuoSplitMode) -> String {
        switch mode {
        case .inboxAndTerminal: return "sidebar.left"
        case .studioDiffSplit: return "plus.forwardslash.minus"
        case .dualTerminalSplit: return "rectangle.split.2x1"
        case .expandedCanvas: return "rectangle.expand.diagonal"
        }
    }

    private func compactTabButton(_ tab: CodaMobileTab, icon: String, label: String, badge: Int) -> some View {
        let selected = store.selectedSessionID == nil && store.selectedTab == tab
        return Button {
            store.closeTerminalToInbox()
            store.selectedTab = tab
        } label: {
            ZStack(alignment: .topTrailing) {
                VStack(spacing: 3) {
                    Image(systemName: icon)
                        .font(.system(size: 13, weight: .semibold))
                    Text(label)
                        .font(.system(size: 10, weight: selected ? .semibold : .medium))
                }
                .foregroundStyle(selected ? CodaHerdrTheme.textPrimary : CodaHerdrTheme.textSecondary)
                .frame(maxWidth: .infinity)
                .padding(.vertical, 7)
                .background(
                    RoundedRectangle(cornerRadius: 10, style: .continuous)
                        .fill(selected ? CodaHerdrTheme.pillActiveBG : .clear)
                        .overlay(
                            RoundedRectangle(cornerRadius: 10, style: .continuous)
                                .strokeBorder(selected ? CodaHerdrTheme.borderHairline : .clear, lineWidth: 1)
                        )
                )

                if badge > 0 {
                    Text("\(badge)")
                        .font(.system(size: 9, weight: .bold))
                        .foregroundStyle(.white)
                        .padding(.horizontal, 4.5)
                        .padding(.vertical, 1)
                        .background(Capsule().fill(CodaHerdrTheme.redDiff))
                        .offset(x: -2, y: 2)
                }
            }
        }
        .buttonStyle(.plain)
    }
}

// MARK: - Landscape Side-Rail Layout (Slab iPhones 14-18 Landscape)

public struct CodaCompactLandscapeSideRailLayout: View {
    @ObservedObject public var store: CodaWorkspaceStore

    public init(store: CodaWorkspaceStore) {
        self.store = store
    }

    public var body: some View {
        HStack(spacing: 0) {
            CodaVerticalSideToolbarRail(store: store, kind: .compactLandscape, edge: .leading)

            Group {
                if let active = store.selectedSession {
                    CodaTerminalSessionView(
                        store: store,
                        session: active,
                        isCompact: true,
                        isCompactHeight: true
                    )
                } else {
                    switch store.selectedTab {
                    case .inbox:
                        CodaMobileInboxTabView(store: store)
                    case .spaces:
                        CodaMobileSpacesTabView(store: store)
                    }
                }
            }
            .padding(.trailing, store.edgeSafeAreas.trailing)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .background(CodaHerdrTheme.canvasBG)
    }
}

// MARK: - iPhone Duo 5.4" Outer Cover Layout (466x678 Portrait & 678x466 Landscape, Right 84pt System Rail)

public struct CodaDuoCoverLayout: View {
    @ObservedObject public var store: CodaWorkspaceStore

    public init(store: CodaWorkspaceStore) {
        self.store = store
    }

    public var body: some View {
        let needsFallbackRail = store.edgeSafeAreas.trailing < 64
        let isLandscape = store.formFactor == .iphoneDuoCoverLandscape

        HStack(spacing: 0) {
            coverPrimaryContent(needsFallbackRail: needsFallbackRail, isLandscape: isLandscape)
                .frame(maxWidth: .infinity, maxHeight: .infinity)

            if needsFallbackRail {
                CodaVerticalSideToolbarRail(store: store, kind: .compactLandscape, edge: .trailing)
            }
        }
        .background(CodaHerdrTheme.canvasBG)
#if os(iOS)
        .toolbar {
            ToolbarItemGroup(placement: .primaryAction) {
                Button {
                    store.closeTerminalToInbox()
                    store.selectedTab = .inbox
                } label: {
                    Label(tr("Inbox", "收件箱", locale: store.locale), systemImage: "tray.fill")
                }
                Button {
                    store.closeTerminalToInbox()
                    store.selectedTab = .spaces
                } label: {
                    Label(tr("Spaces", "空间", locale: store.locale), systemImage: "square.grid.2x2.fill")
                }
                Button {
                    store.setLocale(store.locale == .en ? .zh : .en)
                } label: {
                    Text(store.locale == .en ? "EN" : "中文")
                }
                Button {
                    store.settingsInitialFocusPairing = false
                    store.showSettingsSheet = true
                } label: {
                    Label(tr("Settings", "设置", locale: store.locale), systemImage: "gearshape")
                }
            }
        }
#endif
    }

    @ViewBuilder
    private func coverPrimaryContent(needsFallbackRail: Bool, isLandscape: Bool) -> some View {
        if let active = store.selectedSession {
            CodaTerminalSessionView(
                store: store,
                session: active,
                isCompact: true,
                isCompactHeight: isLandscape
            )
        } else {
            ZStack(alignment: .bottom) {
                Group {
                    switch store.selectedTab {
                    case .inbox:
                        CodaMobileInboxTabView(store: store)
                    case .spaces:
                        CodaMobileSpacesTabView(store: store)
                    }
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)

                if !needsFallbackRail {
                    CodaFloatingBottomTabPill(store: store)
                        .padding(.bottom, 12)
                }
            }
        }
    }
}

// MARK: - Shared Directory Pane (Used in Duo Inner Landscape & Portrait Flat Sidebar)

private final class DuoDirectoryLocalState: ObservableObject {
    @Published var tab: CodaMobileTab = .inbox
}

public struct CodaDuoDirectoryPane: View {
    @ObservedObject public var store: CodaWorkspaceStore
    @StateObject private var local = DuoDirectoryLocalState()

    public init(store: CodaWorkspaceStore) {
        self.store = store
    }

    public var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 6) {
                HStack(spacing: 3) {
                    tabButton(.inbox, title: tr("Inbox", "收件箱", locale: store.locale))
                    tabButton(.spaces, title: tr("Spaces", "空间", locale: store.locale))
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
                    if local.tab == .inbox {
                        if store.groupedInboxSections.isEmpty {
                            emptyCard
                        } else {
                            ForEach(store.groupedInboxSections) { section in
                                Text(section.title.uppercased())
                                    .font(.system(size: 10.5, weight: .semibold))
                                    .tracking(0.5)
                                    .foregroundStyle(section.accentKind == .needsYou ? CodaHerdrTheme.amberNeedsYou : CodaHerdrTheme.textSecondary)
                                    .padding(.horizontal, 4)

                                ForEach(section.sessions) { session in
                                    sessionCard(session)
                                }
                            }
                        }
                    } else {
                        if store.spacesByMachine.isEmpty {
                            emptyCard
                        } else {
                            ForEach(store.spacesByMachine) { group in
                                machineCard(group)
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
                    Label(tr("Add", "添加", locale: store.locale), systemImage: "plus")
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

    private func tabButton(_ tab: CodaMobileTab, title: String) -> some View {
        let active = local.tab == tab
        return Button {
            local.tab = tab
        } label: {
            Text(title)
                .font(.system(size: 11.5, weight: active ? .semibold : .medium))
                .foregroundStyle(active ? CodaHerdrTheme.textPrimary : CodaHerdrTheme.textSecondary)
                .padding(.horizontal, 10)
                .padding(.vertical, 5)
                .background(RoundedRectangle(cornerRadius: 6, style: .continuous).fill(active ? CodaHerdrTheme.pillActiveBG : .clear))
        }
        .buttonStyle(.plain)
    }

    private func sessionCard(_ session: CodaAgentSession) -> some View {
        let isSelected = store.selectedSessionID == session.id
        return Button {
            store.selectSession(session)
        } label: {
            HStack(spacing: 9) {
                CodaWorkspaceAvatar(name: session.workspaceName, size: 28)
                VStack(alignment: .leading, spacing: 2) {
                    HStack(spacing: 5) {
                        Text(session.workspaceName)
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundStyle(CodaHerdrTheme.textPrimary)
                        Text(session.taskTitle)
                            .font(.system(size: 12))
                            .foregroundStyle(CodaHerdrTheme.textPrimary.opacity(0.86))
                            .lineLimit(1)
                    }
                    Text("\(session.agentName) · \(session.branch)")
                        .font(.system(size: 10.5))
                        .foregroundStyle(CodaHerdrTheme.textSecondary)
                        .lineLimit(1)
                }
                Spacer(minLength: 4)
                CodaSessionStatusBadge(session: session, locale: store.locale)
            }
            .padding(.horizontal, 9)
            .padding(.vertical, 8)
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

    private func machineCard(_ group: CodaMachineSpacesGroup) -> some View {
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
                            .font(.system(size: 10, design: .monospaced))
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

    private var emptyCard: some View {
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
}

// MARK: - iPhone Duo 7.6" Inner Unfolded Landscape Workbench (890 x 626 pt: 433 + 24 Crease + 349 + 84 Right Rail)

public struct CodaDuoInnerLandscapeWorkbenchView: View {
    @ObservedObject public var store: CodaWorkspaceStore

    public init(store: CodaWorkspaceStore) {
        self.store = store
    }

    public var body: some View {
        GeometryReader { geo in
            let fullDisplayWidth = geo.size.width + store.edgeSafeAreas.leading + store.edgeSafeAreas.trailing
            let needsFallbackRail = store.edgeSafeAreas.trailing < 64

            HStack(spacing: 0) {
                VStack(spacing: 0) {
                    duoLandscapeHeaderBar
                        .frame(height: 34)

                    Divider().background(CodaHerdrTheme.borderHairline)

                    if store.duoSplitMode == .expandedCanvas {
                        if let active = store.selectedSession {
                            CodaTerminalSessionView(
                                store: store,
                                session: active,
                                isCompact: false,
                                isSplitThumb: true,
                                isCompactHeight: true
                            )
                            .frame(maxWidth: .infinity, maxHeight: .infinity)
                        } else {
                            duoEmptyDetailPane
                        }
                    } else {
                        CodaArrangementSplitView(
                            axis: .horizontal,
                            leadingOffset: store.edgeSafeAreas.leading,
                            totalWindowSpan: fullDisplayWidth,
                            creaseThickness: 24
                        ) {
                            leftLeafView
                        } secondary: {
                            rightLeafView
                        }
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                    }
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)

                if needsFallbackRail {
                    CodaVerticalSideToolbarRail(store: store, kind: .duoInner, edge: .trailing)
                }
            }
        }
        .background(CodaHerdrTheme.canvasBG)
#if os(iOS)
        .toolbar {
            ToolbarItemGroup(placement: .primaryAction) {
                ForEach(CodaDuoSplitMode.allCases) { mode in
                    Button {
                        withAnimation(.snappy(duration: 0.18)) {
                            store.setDuoSplitMode(mode)
                        }
                    } label: {
                        Text(mode.title(locale: store.locale))
                    }
                }
            }
        }
#endif
    }

    private var duoLandscapeHeaderBar: some View {
        HStack(spacing: 6) {
            Circle()
                .fill(store.remoteMachines.contains(where: { $0.isOnline }) ? CodaHerdrTheme.greenDiff : CodaHerdrTheme.textDim)
                .frame(width: 7, height: 7)

            Text("Coda Duo")
                .font(.system(size: 12.5, weight: .bold))
                .foregroundStyle(CodaHerdrTheme.textPrimary)

            Text("\(store.terminalColumns)×\(store.terminalRows)")
                .font(.system(size: 10, weight: .medium, design: .monospaced))
                .foregroundStyle(CodaHerdrTheme.greenDiff)
                .padding(.horizontal, 6)
                .padding(.vertical, 2)
                .background(
                    Capsule()
                        .fill(CodaHerdrTheme.canvasBG)
                        .overlay(Capsule().strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
                )

            Spacer(minLength: 32)

            HStack(spacing: 2) {
                ForEach(CodaDuoSplitMode.allCases) { mode in
                    let selected = store.duoSplitMode == mode
                    Button {
                        withAnimation(.snappy(duration: 0.18)) {
                            store.setDuoSplitMode(mode)
                        }
                    } label: {
                        Text(mode.title(locale: store.locale))
                            .font(.system(size: 10, weight: selected ? .semibold : .medium))
                            .foregroundStyle(selected ? CodaHerdrTheme.textPrimary : CodaHerdrTheme.textSecondary)
                            .padding(.horizontal, 7)
                            .padding(.vertical, 3.5)
                            .background(
                                Capsule().fill(selected ? CodaHerdrTheme.pillActiveBG : .clear)
                            )
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(2)
            .background(
                Capsule()
                    .fill(CodaHerdrTheme.canvasBG)
                    .overlay(Capsule().strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
            )

            Button {
                store.setLocale(store.locale == .en ? .zh : .en)
            } label: {
                Text(store.locale == .en ? "EN" : "中文")
                    .font(.system(size: 10, weight: .semibold))
                    .foregroundStyle(CodaHerdrTheme.textSecondary)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 3.5)
                    .background(Capsule().fill(CodaHerdrTheme.elevatedBG))
            }
            .buttonStyle(.plain)

            Button {
                store.settingsInitialFocusPairing = false
                store.showSettingsSheet = true
            } label: {
                Image(systemName: "gearshape")
                    .font(.system(size: 11.5))
                    .foregroundStyle(CodaHerdrTheme.textPrimary)
                    .frame(width: 24, height: 24)
                    .background(Circle().fill(CodaHerdrTheme.elevatedBG))
            }
            .buttonStyle(.plain)
        }
        .padding(.horizontal, 10)
        .background(CodaHerdrTheme.cardBG)
    }

    @ViewBuilder
    private var leftLeafView: some View {
        switch store.duoSplitMode {
        case .inboxAndTerminal:
            CodaDuoDirectoryPane(store: store)
        case .studioDiffSplit, .dualTerminalSplit, .expandedCanvas:
            if let active = store.selectedSession {
                CodaTerminalSessionView(
                    store: store,
                    session: active,
                    isCompact: false,
                    isCompactHeight: true
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
                    isCompact: false,
                    isCompactHeight: true
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
                        isSecondaryLeaf: true,
                        isCompactHeight: true
                    )
                }
            } else {
                duoEmptyDetailPane
            }
        case .expandedCanvas:
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
        .padding(.vertical, 5)
        .background(CodaHerdrTheme.cardBG.opacity(0.9))
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

// MARK: - iPhone Duo 7.6" Inner Unfolded Portrait Workbench (626 x 890 pt: Right 84pt System Rail + Flat 80-Col <-> Tabletop Laptop Pose)

public struct CodaDuoInnerPortraitWorkbenchView: View {
    @ObservedObject public var store: CodaWorkspaceStore

    public init(store: CodaWorkspaceStore) {
        self.store = store
    }

    public var body: some View {
        GeometryReader { geo in
            let headerHeight: CGFloat = 38
            let fullDisplayHeight = geo.size.height + store.edgeSafeAreas.top + store.edgeSafeAreas.bottom
            let needsFallbackRail = store.edgeSafeAreas.trailing < 64

            HStack(spacing: 0) {
                VStack(spacing: 0) {
                    portraitHeaderBar
                        .frame(height: headerHeight)

                    Divider().background(CodaHerdrTheme.borderHairline)

                    if store.foldPosture == .tabletopLaptop {
                        CodaArrangementSplitView(
                            axis: .vertical,
                            leadingOffset: store.edgeSafeAreas.top + headerHeight + 1,
                            totalWindowSpan: fullDisplayHeight,
                            creaseThickness: 24
                        ) {
                            if let active = store.selectedSession {
                                CodaTerminalSessionView(
                                    store: store,
                                    session: active,
                                    isCompact: false,
                                    isCompactHeight: true,
                                    hideBottomControls: true
                                )
                            } else {
                                emptyPortraitTerminal
                            }
                        } secondary: {
                            CodaDuoTabletopControlDeck(store: store, session: store.selectedSession)
                        }
                    } else {
                        HStack(spacing: 0) {
                            if store.showSidebar {
                                CodaDuoDirectoryPane(store: store)
                                    .frame(width: 212)

                                Divider().background(CodaHerdrTheme.borderHairline)
                            }

                            if let active = store.selectedSession {
                                CodaTerminalSessionView(
                                    store: store,
                                    session: active,
                                    isCompact: false
                                )
                                .frame(maxWidth: .infinity, maxHeight: .infinity)
                            } else {
                                emptyPortraitTerminal
                            }
                        }
                    }
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)

                if needsFallbackRail {
                    CodaVerticalSideToolbarRail(store: store, kind: .duoInner, edge: .trailing)
                }
            }
        }
        .background(CodaHerdrTheme.canvasBG)
#if os(iOS)
        .toolbar {
            ToolbarItemGroup(placement: .primaryAction) {
                Button {
                    store.setFoldPosture(.flat)
                } label: {
                    Label(tr("80-Col Flat", "80列平铺", locale: store.locale), systemImage: "rectangle")
                }
                Button {
                    store.setFoldPosture(.tabletopLaptop)
                } label: {
                    Label(tr("Tabletop", "笔电半折", locale: store.locale), systemImage: "laptopcomputer")
                }
            }
        }
#endif
    }

    private var portraitHeaderBar: some View {
        HStack(spacing: 8) {
            if store.foldPosture != .tabletopLaptop {
                Button {
                    withAnimation(.snappy(duration: 0.18)) {
                        store.showSidebar.toggle()
                    }
                } label: {
                    Image(systemName: "sidebar.left")
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundStyle(store.showSidebar ? CodaHerdrTheme.cyanWorking : CodaHerdrTheme.textPrimary)
                        .frame(width: 28, height: 26)
                        .background(
                            RoundedRectangle(cornerRadius: 7, style: .continuous)
                                .fill(CodaHerdrTheme.elevatedBG)
                                .overlay(RoundedRectangle(cornerRadius: 7, style: .continuous).strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
                        )
                }
                .buttonStyle(.plain)
            }

            Circle()
                .fill(store.remoteMachines.contains(where: { $0.isOnline }) ? CodaHerdrTheme.greenDiff : CodaHerdrTheme.textDim)
                .frame(width: 7, height: 7)

            Text("Coda Duo")
                .font(.system(size: 13, weight: .bold))
                .foregroundStyle(CodaHerdrTheme.textPrimary)

            Text("\(store.terminalColumns)×\(store.terminalRows)")
                .font(.system(size: 10, weight: .medium, design: .monospaced))
                .foregroundStyle(CodaHerdrTheme.greenDiff)

            Spacer()

            HStack(spacing: 3) {
                posturePill(.flat, title: tr("80-Col Flat", "80列平铺", locale: store.locale))
                posturePill(.tabletopLaptop, title: tr("Tabletop", "笔电半折", locale: store.locale))
            }
            .padding(2)
            .background(Capsule().fill(CodaHerdrTheme.canvasBG).overlay(Capsule().strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1)))

            Button {
                store.setLocale(store.locale == .en ? .zh : .en)
            } label: {
                Text(store.locale == .en ? "EN" : "中文")
                    .font(.system(size: 10, weight: .semibold))
                    .foregroundStyle(CodaHerdrTheme.textSecondary)
                    .padding(.horizontal, 7)
                    .padding(.vertical, 4)
                    .background(Capsule().fill(CodaHerdrTheme.elevatedBG))
            }
            .buttonStyle(.plain)

            Button {
                store.settingsInitialFocusPairing = false
                store.showSettingsSheet = true
            } label: {
                Image(systemName: "gearshape")
                    .font(.system(size: 12))
                    .foregroundStyle(CodaHerdrTheme.textPrimary)
                    .frame(width: 26, height: 26)
                    .background(Circle().fill(CodaHerdrTheme.elevatedBG))
            }
            .buttonStyle(.plain)
        }
        .padding(.horizontal, 10)
        .background(CodaHerdrTheme.cardBG)
    }

    private func posturePill(_ posture: CodaFoldPosture, title: String) -> some View {
        let active = (posture == .tabletopLaptop && store.foldPosture == .tabletopLaptop)
            || (posture == .flat && store.foldPosture != .tabletopLaptop)
        return Button {
            withAnimation(.snappy(duration: 0.18)) {
                store.setFoldPosture(posture)
            }
        } label: {
            Text(title)
                .font(.system(size: 10.5, weight: active ? .semibold : .medium))
                .foregroundStyle(active ? CodaHerdrTheme.textPrimary : CodaHerdrTheme.textSecondary)
                .padding(.horizontal, 9)
                .padding(.vertical, 3.5)
                .background(
                    Capsule().fill(active ? CodaHerdrTheme.pillActiveBG : .clear)
                )
        }
        .buttonStyle(.plain)
    }

    private var emptyPortraitTerminal: some View {
        VStack(spacing: 8) {
            Image(systemName: "terminal")
                .font(.system(size: 26))
                .foregroundStyle(CodaHerdrTheme.textSecondary)
            Text(tr("Open sidebar or select a session", "请从侧边栏选择一个会话", locale: store.locale))
                .font(.system(size: 13))
                .foregroundStyle(CodaHerdrTheme.textSecondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(CodaHerdrTheme.canvasBG)
    }
}

// MARK: - Lower-Half Tabletop Control Deck (Below Horizontal Crease on 626x890 Half-Folded Duo)

private final class TabletopDeckLocalState: ObservableObject {
    @Published var commandDraft: String = ""
}

public struct CodaDuoTabletopControlDeck: View {
    @ObservedObject public var store: CodaWorkspaceStore
    public let session: CodaAgentSession?
    @StateObject private var local = TabletopDeckLocalState()
    @FocusState private var inputFocused: Bool

    public init(store: CodaWorkspaceStore, session: CodaAgentSession?) {
        self.store = store
        self.session = session
    }

    public var body: some View {
        VStack(spacing: 10) {
            HStack(spacing: 6) {
                Text(tr("SESSIONS", "会话切换", locale: store.locale))
                    .font(.system(size: 10, weight: .semibold, design: .monospaced))
                    .foregroundStyle(CodaHerdrTheme.textDim)

                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 6) {
                        ForEach(store.filteredSessions) { sess in
                            let selected = store.selectedSessionID == sess.id
                            Button {
                                store.selectSession(sess)
                            } label: {
                                HStack(spacing: 5) {
                                    Circle()
                                        .fill(sess.state == .needsYou ? CodaHerdrTheme.amberNeedsYou : (sess.state == .working ? CodaHerdrTheme.cyanWorking : CodaHerdrTheme.textDim))
                                        .frame(width: 6, height: 6)
                                    Text("\(sess.workspaceName)/\(sess.agentName)")
                                        .font(.system(size: 11, weight: selected ? .semibold : .medium))
                                        .foregroundStyle(selected ? CodaHerdrTheme.textPrimary : CodaHerdrTheme.textSecondary)
                                }
                                .padding(.horizontal, 9)
                                .padding(.vertical, 5)
                                .background(
                                    Capsule()
                                        .fill(selected ? CodaHerdrTheme.pillActiveBG : CodaHerdrTheme.cardBG)
                                        .overlay(
                                            Capsule().strokeBorder(
                                                selected ? CodaHerdrTheme.cyanWorking.opacity(0.5) : CodaHerdrTheme.borderHairline,
                                                lineWidth: 1
                                            )
                                        )
                                )
                            }
                            .buttonStyle(.plain)
                        }
                    }
                }

                if let active = session {
                    Button {
                        store.loadGitDiffForSelected()
                    } label: {
                        Image(systemName: "plus.forwardslash.minus")
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundStyle(active.diffAdded > 0 ? CodaHerdrTheme.greenDiff : CodaHerdrTheme.textSecondary)
                            .frame(width: 28, height: 28)
                            .background(RoundedRectangle(cornerRadius: 7).fill(CodaHerdrTheme.elevatedBG))
                    }
                    .buttonStyle(.plain)
                }
            }

            if let active = session, active.state == .needsYou {
                HStack(spacing: 8) {
                    Image(systemName: "exclamationmark.triangle.fill")
                        .font(.system(size: 12))
                        .foregroundStyle(CodaHerdrTheme.amberNeedsYou)
                    Text(active.statusMessage ?? tr("Waiting for confirmation", "等待输入确认", locale: store.locale))
                        .font(.system(size: 11.5, weight: .medium))
                        .foregroundStyle(CodaHerdrTheme.textPrimary)
                        .lineLimit(1)
                    Spacer()
                    deckApproveButton("1 · Yes") { store.sendDirectText("1", submit: true, to: active) }
                    deckApproveButton("2 · Always") { store.sendDirectText("2", submit: true, to: active) }
                    deckApproveButton("Esc") { store.interruptSession(active) }
                }
                .padding(.horizontal, 10)
                .padding(.vertical, 7)
                .background(
                    RoundedRectangle(cornerRadius: 10, style: .continuous)
                        .fill(CodaHerdrTheme.amberNeedsYou.opacity(0.12))
                        .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(CodaHerdrTheme.amberNeedsYou.opacity(0.45), lineWidth: 1))
                )
            }

            HStack(spacing: 8) {
                TextField(
                    tr("Command or prompt to upper screen (Enter sends)...", "向下半屏发送命令或追问（回车发送）...", locale: store.locale),
                    text: $local.commandDraft
                )
                .font(.system(size: 13, design: .monospaced))
                .foregroundStyle(CodaHerdrTheme.textPrimary)
                .focused($inputFocused)
                .codaNoAutocapAndSpelling()
                .onSubmit {
                    submitCommand()
                }

                Button {
                    submitCommand()
                } label: {
                    Image(systemName: "arrow.up.circle.fill")
                        .font(.system(size: 22))
                        .foregroundStyle(CodaHerdrTheme.cyanWorking)
                }
                .buttonStyle(.plain)

                Button {
                    store.showComposeSheet = true
                } label: {
                    Image(systemName: "square.and.pencil")
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundStyle(CodaHerdrTheme.textPrimary)
                        .frame(width: 32, height: 32)
                        .background(Circle().fill(CodaHerdrTheme.elevatedBG))
                }
                .buttonStyle(.plain)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 9)
            .background(
                RoundedRectangle(cornerRadius: 12, style: .continuous)
                    .fill(CodaHerdrTheme.cardBG)
                    .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
            )

            CodaDuoTactileTwoRowKeypad(store: store, targetSession: session)

            Spacer(minLength: 0)
        }
        .padding(.horizontal, 14)
        .padding(.top, 10)
        .padding(.bottom, max(12, store.edgeSafeAreas.bottom + 4))
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(CodaHerdrTheme.canvasBG)
    }

    private func submitCommand() {
        let trimmed = local.commandDraft.trimmingCharacters(in: .newlines)
        local.commandDraft = ""
        if trimmed.isEmpty {
            store.sendQuickKey(.enter, to: session)
        } else {
            store.sendDirectText(trimmed, submit: true, to: session)
        }
    }

    private func deckApproveButton(_ title: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(title)
                .font(.system(size: 11, weight: .semibold, design: .monospaced))
                .foregroundStyle(CodaHerdrTheme.amberNeedsYou)
                .padding(.horizontal, 9)
                .padding(.vertical, 4.5)
                .background(
                    Capsule()
                        .fill(CodaHerdrTheme.canvasBG)
                        .overlay(Capsule().strokeBorder(CodaHerdrTheme.amberNeedsYou.opacity(0.5), lineWidth: 1))
                )
        }
        .buttonStyle(.plain)
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

// MARK: - 2-Row Dedicated Tactile Terminal Keypad (Cover, Unfolded Canvas & Tabletop Control Deck)

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
