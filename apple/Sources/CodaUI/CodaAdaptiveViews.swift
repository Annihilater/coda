import CodaKit
import SwiftUI

// MARK: - Adaptive Root View (`iPhone` Compact Tab/Stack <-> `iPadOS` / `Mac` Split View)

public struct CodaAdaptiveRootView: View {
    @StateObject private var store = CodaWorkspaceStore()
    @FocusState private var sidebarSearchFocused: Bool
#if os(iOS)
    @Environment(\.horizontalSizeClass) private var horizontalSizeClass
    @Environment(\.verticalSizeClass) private var verticalSizeClass
#endif

    public init() {}

    private var isHorizontalRegular: Bool? {
#if os(iOS)
        return horizontalSizeClass == .regular
#else
        return nil
#endif
    }

    private var isVerticalRegular: Bool? {
#if os(iOS)
        return verticalSizeClass == .regular
#else
        return nil
#endif
    }

    private func edgeSafeAreas(from insets: EdgeInsets) -> CodaEdgeSafeAreas {
        CodaEdgeSafeAreas(
            top: insets.top,
            leading: insets.leading,
            bottom: insets.bottom,
            trailing: insets.trailing
        )
    }

    @ViewBuilder
    private func applyingSizeClassObservers<Content: View>(
        geo: GeometryProxy,
        safeAreas: CodaEdgeSafeAreas,
        @ViewBuilder content: () -> Content
    ) -> some View {
#if os(iOS)
        content()
            .onChange(of: horizontalSizeClass) { _, _ in
                store.updateViewportGeometry(
                    size: geo.size,
                    safeAreas: safeAreas,
                    isHorizontalRegular: isHorizontalRegular,
                    isVerticalRegular: isVerticalRegular
                )
            }
            .onChange(of: verticalSizeClass) { _, _ in
                store.updateViewportGeometry(
                    size: geo.size,
                    safeAreas: safeAreas,
                    isHorizontalRegular: isHorizontalRegular,
                    isVerticalRegular: isVerticalRegular
                )
            }
#else
        content()
#endif
    }

    public var body: some View {
        GeometryReader { geo in
            let safeAreas = edgeSafeAreas(from: geo.safeAreaInsets)
            let isHReg = isHorizontalRegular
            let isVReg = isVerticalRegular
            let factor = CodaFormFactor.detect(
                width: geo.size.width,
                height: geo.size.height,
                isHorizontalRegular: isHReg,
                isVerticalRegular: isVReg
            )

            applyingSizeClassObservers(geo: geo, safeAreas: safeAreas) {
                ZStack(alignment: .top) {
                    switch factor {
                    case .iphonePortrait, .iphoneDuoCoverPortrait:
                        compactPhoneLayout
                    case .iphoneLandscape, .iphoneDuoCoverLandscape:
                        CodaCompactLandscapeSideRailLayout(store: store)
                    case .iphoneDuoInnerLandscape:
                        CodaDuoInnerLandscapeWorkbenchView(store: store)
                    case .iphoneDuoInnerPortrait:
                        CodaDuoInnerPortraitWorkbenchView(store: store)
                    case .ipadOrDesktop:
                        regularIPadSplitLayout
                    }

                if let toast = store.activeToast {
                    CodaToastBannerView(
                        toast: toast,
                        locale: store.locale,
                        onTap: {
                            if let match = store.sessions.first(where: { $0.id == toast.sessionId }) {
                                store.selectSession(match)
                            }
                            store.activeToast = nil
                        },
                        onDismiss: {
                            store.activeToast = nil
                        }
                    )
                    .padding(.top, 10)
                    .padding(.horizontal, 16)
                    .transition(.move(edge: .top).combined(with: .opacity))
                    .zIndex(50)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(CodaHerdrTheme.canvasBG)
            .onAppear {
                store.updateViewportGeometry(
                    size: geo.size,
                    safeAreas: safeAreas,
                    isHorizontalRegular: isHReg,
                    isVerticalRegular: isVReg
                )
                store.startLiveSync()
                if !factor.isCompact, store.selectedSessionID == nil, let first = store.filteredSessions.first {
                    store.selectSession(first)
                }
            }
            .onChange(of: geo.size) { _, next in
                store.updateViewportGeometry(
                    size: next,
                    safeAreas: safeAreas,
                    isHorizontalRegular: isHReg,
                    isVerticalRegular: isVReg
                )
            }
            .onChange(of: store.showSidebar) { _, _ in
                store.updateViewportGeometry(
                    size: geo.size,
                    safeAreas: safeAreas,
                    isHorizontalRegular: isHReg,
                    isVerticalRegular: isVReg
                )
            }
            .onChange(of: store.sessions.count) { _, _ in
                if !factor.isCompact, store.selectedSessionID == nil, let first = store.filteredSessions.first {
                    store.selectSession(first)
                }
                if !factor.isCompact {
                    store.updateViewportGeometry(
                        size: geo.size,
                        safeAreas: safeAreas,
                        isHorizontalRegular: isHReg,
                        isVerticalRegular: isVReg
                    )
                }
            }
            }
        }
        .sheet(isPresented: $store.showSettingsSheet) {
            CodaSettingsAndPairingSheet(store: store)
        }
        .preferredColorScheme(.dark)
    }

    // MARK: - iPhone Compact Layout (`Inbox | Spaces` + Full-Screen Terminal)

    @ViewBuilder
    private var compactPhoneLayout: some View {
        if let active = store.selectedSession {
            CodaTerminalSessionView(store: store, session: active, isCompact: true)
                .transition(.move(edge: .trailing))
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

                CodaFloatingBottomTabPill(store: store)
                    .padding(.bottom, 12)
            }
        }
    }

    // MARK: - iPadOS / Unfolded Foldable / Mac Split Layout

    private var regularIPadSplitLayout: some View {
        HStack(spacing: 0) {
            if store.showSidebar {
                regularSidebar
                    .frame(width: 288)
                    .background(CodaHerdrTheme.cardBG)

                Divider()
                    .background(CodaHerdrTheme.borderHairline)
            }

            if let active = store.selectedSession {
                CodaTerminalSessionView(store: store, session: active, isCompact: false)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                emptyWorkspaceDetailPane
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
    }

    private var regularSidebar: some View {
        VStack(spacing: 0) {
            // Top Brand Bar
            HStack(spacing: 8) {
                Circle()
                    .fill(store.remoteMachines.contains(where: { $0.isOnline }) ? CodaHerdrTheme.greenDiff : CodaHerdrTheme.textDim)
                    .frame(width: 8, height: 8)

                Text("Coda")
                    .font(.system(size: 16, weight: .bold))
                    .foregroundStyle(CodaHerdrTheme.textPrimary)

                Spacer()

                if store.attentionBadgeCount > 0 {
                    Text("\(store.attentionBadgeCount)")
                        .font(.system(size: 10, weight: .bold))
                        .foregroundStyle(.black)
                        .padding(.horizontal, 6)
                        .padding(.vertical, 2)
                        .background(Capsule().fill(CodaHerdrTheme.amberNeedsYou))
                }

                Button {
                    store.setLocale(store.locale == .en ? .zh : .en)
                } label: {
                    Text(store.locale == .en ? "EN" : "中文")
                        .font(.system(size: 10, weight: .semibold))
                        .foregroundStyle(CodaHerdrTheme.textSecondary)
                        .padding(.horizontal, 7)
                        .padding(.vertical, 3)
                        .background(
                            Capsule()
                                .fill(CodaHerdrTheme.elevatedBG)
                                .overlay(Capsule().strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
                        )
                }
                .buttonStyle(.plain)
            }
            .padding(.horizontal, 14)
            .padding(.top, 14)
            .padding(.bottom, 10)

            // Search Input
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
                .focused($sidebarSearchFocused)
                .codaNoAutocapAndSpelling()

                Button {
                    sidebarSearchFocused = true
                } label: {
                    Text("⌘K")
                        .font(.system(size: 10, design: .monospaced))
                        .foregroundStyle(CodaHerdrTheme.textDim)
                }
                .buttonStyle(.plain)
                .keyboardShortcut("k", modifiers: [.command])
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 7)
            .background(
                RoundedRectangle(cornerRadius: 9, style: .continuous)
                    .fill(CodaHerdrTheme.canvasBG)
                    .overlay(RoundedRectangle(cornerRadius: 9, style: .continuous).strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
            )
            .padding(.horizontal, 12)
            .padding(.bottom, 10)

            ScrollView {
                LazyVStack(alignment: .leading, spacing: 14) {
                    // Spaces Tree
                    if !store.spacesByMachine.isEmpty {
                        VStack(alignment: .leading, spacing: 8) {
                            Text(tr("SPACES", "工作空间", locale: store.locale))
                                .font(.system(size: 10, weight: .semibold))
                                .tracking(0.6)
                                .foregroundStyle(CodaHerdrTheme.textDim)
                                .padding(.horizontal, 4)

                            ForEach(store.spacesByMachine) { group in
                                sidebarMachineGroup(group)
                            }
                        }
                    }

                    // Inbox Priority Sections
                    VStack(alignment: .leading, spacing: 8) {
                        Text(tr("INBOX", "收件箱", locale: store.locale))
                            .font(.system(size: 10, weight: .semibold))
                            .tracking(0.6)
                            .foregroundStyle(CodaHerdrTheme.textDim)
                            .padding(.horizontal, 4)

                        if store.groupedInboxSections.isEmpty {
                            Text(store.isSignedIn
                                ? tr("No active terminals", "暂无活跃终端", locale: store.locale)
                                : tr("Sign in via Settings to connect", "请在设置中登录并配对主机", locale: store.locale))
                                .font(.system(size: 12))
                                .foregroundStyle(CodaHerdrTheme.textSecondary)
                                .padding(.horizontal, 6)
                                .padding(.vertical, 4)
                        } else {
                            ForEach(store.groupedInboxSections) { section in
                                Text(section.title)
                                    .font(.system(size: 10.5, weight: .semibold))
                                    .foregroundStyle(section.accentKind == .needsYou ? CodaHerdrTheme.amberNeedsYou : CodaHerdrTheme.textSecondary)
                                    .padding(.horizontal, 6)
                                    .padding(.top, 2)

                                ForEach(section.sessions) { session in
                                    sidebarSessionRow(session)
                                }
                            }
                        }
                    }
                }
                .padding(.horizontal, 10)
                .padding(.vertical, 6)
            }

            Divider()
                .background(CodaHerdrTheme.borderHairline)

            // Pinned Sidebar Bottom Footer (`Settings` + `+ Add machine`)
            HStack(spacing: 8) {
                Button {
                    store.settingsInitialFocusPairing = false
                    store.showSettingsSheet = true
                } label: {
                    Label(tr("Settings", "设置", locale: store.locale), systemImage: "gearshape")
                        .font(.system(size: 12, weight: .medium))
                        .foregroundStyle(CodaHerdrTheme.textSecondary)
                        .padding(.horizontal, 8)
                        .padding(.vertical, 6)
                        .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.elevatedBG))
                }
                .buttonStyle(.plain)

                Spacer()

                Button {
                    store.settingsInitialFocusPairing = true
                    store.showSettingsSheet = true
                } label: {
                    Label(tr("Add machine", "添加主机", locale: store.locale), systemImage: "plus")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(CodaHerdrTheme.textPrimary)
                        .padding(.horizontal, 10)
                        .padding(.vertical, 6)
                        .background(
                            RoundedRectangle(cornerRadius: 8)
                                .fill(CodaHerdrTheme.elevatedBG)
                                .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
                        )
                }
                .buttonStyle(.plain)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 10)
        }
    }

    @ViewBuilder
    private func sidebarMachineGroup(_ group: CodaMachineSpacesGroup) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Circle()
                    .fill(group.device.isOnline ? CodaHerdrTheme.greenDiff : CodaHerdrTheme.textDim)
                    .frame(width: 6, height: 6)
                Text(group.device.name)
                    .font(.system(size: 11.5, weight: .semibold))
                    .foregroundStyle(CodaHerdrTheme.textSecondary)
                Spacer()
                if group.device.isOnline {
                    Menu {
                        Button("Claude Code") { store.launchAgent(agent: "claude", on: group.device, cwd: group.repos.first?.rootPath) }
                        Button("Codex") { store.launchAgent(agent: "codex", on: group.device, cwd: group.repos.first?.rootPath) }
                        Button("Gemini CLI") { store.launchAgent(agent: "gemini", on: group.device, cwd: group.repos.first?.rootPath) }
                        Button("Shell (zsh)") { store.launchAgent(agent: "zsh", on: group.device, cwd: group.repos.first?.rootPath) }
                    } label: {
                        Image(systemName: "plus")
                            .font(.system(size: 11, weight: .semibold))
                            .foregroundStyle(CodaHerdrTheme.textSecondary)
                    }
                }
            }
            .padding(.horizontal, 6)

            ForEach(group.repos) { repo in
                let isExpanded = store.expandedWorkspaces.contains(repo.id)
                let matching = group.sessions.filter { $0.cwd.hasPrefix(repo.rootPath) || $0.workspaceName == repo.repoName }
                VStack(alignment: .leading, spacing: 4) {
                    Button {
                        if isExpanded {
                            store.expandedWorkspaces.remove(repo.id)
                        } else {
                            store.expandedWorkspaces.insert(repo.id)
                        }
                    } label: {
                        HStack(spacing: 8) {
                            CodaWorkspaceAvatar(name: repo.repoName, size: 22)
                            Text(repo.repoName)
                                .font(.system(size: 12.5, weight: .semibold))
                                .foregroundStyle(CodaHerdrTheme.textPrimary)
                                .lineLimit(1)
                            Spacer()
                            if let b = repo.branches.first, (b.diffAdded > 0 || b.diffDeleted > 0) {
                                Text("+\(b.diffAdded)/-\(b.diffDeleted)")
                                    .font(.system(size: 10, design: .monospaced))
                                    .foregroundStyle(CodaHerdrTheme.greenDiff)
                            }
                            Image(systemName: isExpanded ? "chevron.down" : "chevron.right")
                                .font(.system(size: 10, weight: .semibold))
                                .foregroundStyle(CodaHerdrTheme.textDim)
                        }
                        .padding(.horizontal, 8)
                        .padding(.vertical, 6)
                        .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.canvasBG.opacity(0.6)))
                    }
                    .buttonStyle(.plain)

                    if isExpanded {
                        ForEach(matching) { sib in
                            sidebarSessionRow(sib)
                                .padding(.leading, 12)
                        }
                    }
                }
            }
        }
    }

    private func sidebarSessionRow(_ session: CodaAgentSession) -> some View {
        let isSelected = store.selectedSessionID == session.id
        return Button {
            store.selectSession(session)
        } label: {
            HStack(spacing: 8) {
                CodaWorkspaceAvatar(name: session.workspaceName, size: 26)
                VStack(alignment: .leading, spacing: 2) {
                    Text(session.taskTitle)
                        .font(.system(size: 12.5, weight: isSelected ? .semibold : .medium))
                        .foregroundStyle(CodaHerdrTheme.textPrimary)
                        .lineLimit(1)
                    Text("\(session.agentName) · \(session.branch)")
                        .font(.system(size: 10.5))
                        .foregroundStyle(CodaHerdrTheme.textSecondary)
                        .lineLimit(1)
                }
                Spacer(minLength: 4)
                CodaSessionStatusBadge(session: session, locale: store.locale)
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 7)
            .background(
                RoundedRectangle(cornerRadius: 9, style: .continuous)
                    .fill(isSelected ? CodaHerdrTheme.pillActiveBG : .clear)
                    .overlay(
                        RoundedRectangle(cornerRadius: 9, style: .continuous)
                            .strokeBorder(isSelected ? CodaHerdrTheme.borderHairline : .clear, lineWidth: 1)
                    )
            )
        }
        .buttonStyle(.plain)
    }

    private var emptyWorkspaceDetailPane: some View {
        VStack(spacing: 0) {
            HStack {
                if !store.showSidebar {
                    Button {
                        withAnimation(.snappy(duration: 0.2)) {
                            store.showSidebar = true
                        }
                    } label: {
                        Image(systemName: "sidebar.left")
                            .font(.system(size: 15))
                            .foregroundStyle(CodaHerdrTheme.textPrimary)
                            .frame(width: 32, height: 32)
                            .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.elevatedBG))
                    }
                    .buttonStyle(.plain)
                    .keyboardShortcut("b", modifiers: [.command])
                }
                Spacer()
            }
            .padding(14)

            Spacer()

            VStack(spacing: 14) {
                Image(systemName: "terminal.fill")
                    .font(.system(size: 30))
                    .foregroundStyle(CodaHerdrTheme.textSecondary)

                Text(store.isSignedIn
                    ? tr("Select or launch a terminal", "选择或新建终端会话", locale: store.locale)
                    : tr("Connect Coda Agent Mesh", "连接 Coda Agent 加密工作网", locale: store.locale))
                    .font(.system(size: 18, weight: .semibold))
                    .foregroundStyle(CodaHerdrTheme.textPrimary)

                Text(store.isSignedIn
                    ? tr("Pick any session from Inbox or Spaces on the left to stream live ANSI output.", "从左侧栏选择一个工作区会话，实时查看加密终端并下发指令。", locale: store.locale)
                    : tr("Sign in and pair a host machine to control agents across iPhone, iPad, Mac, and Linux.", "登录并配对工作站后，即可在 iPhone、iPad 与 Mac 间同步接管 AI Coding Agent。", locale: store.locale))
                    .font(.system(size: 13))
                    .foregroundStyle(CodaHerdrTheme.textSecondary)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 400)

                HStack(spacing: 10) {
                    Button {
                        store.settingsInitialFocusPairing = store.isSignedIn
                        store.showSettingsSheet = true
                    } label: {
                        Label(
                            store.isSignedIn ? tr("Add machine", "添加主机", locale: store.locale) : tr("Sign in", "立即登录", locale: store.locale),
                            systemImage: store.isSignedIn ? "plus" : "person.badge.key.fill"
                        )
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundStyle(.black)
                        .padding(.horizontal, 16)
                        .padding(.vertical, 9)
                        .background(Capsule().fill(CodaHerdrTheme.textPrimary))
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(24)

            Spacer()
        }
        .background(CodaHerdrTheme.canvasBG)
    }
}

// MARK: - Top Floating Transition Toast Banner

public struct CodaToastBannerView: View {
    public let toast: CodaToastBanner
    public let locale: CodaUILocale
    public let onTap: () -> Void
    public let onDismiss: () -> Void

    public var body: some View {
        HStack(spacing: 10) {
            Circle()
                .fill(toast.state == .needsYou ? CodaHerdrTheme.amberNeedsYou : CodaHerdrTheme.blueUnread)
                .frame(width: 8, height: 8)

            VStack(alignment: .leading, spacing: 1) {
                Text("\(toast.workspaceName) · \(toast.state == .needsYou ? tr("Needs you", "待确认", locale: locale) : tr("Done", "已完成", locale: locale))")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(CodaHerdrTheme.textPrimary)
                Text(toast.title)
                    .font(.system(size: 11))
                    .foregroundStyle(CodaHerdrTheme.textSecondary)
                    .lineLimit(1)
            }

            Spacer(minLength: 8)

            Button(action: onDismiss) {
                Image(systemName: "xmark")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(CodaHerdrTheme.textDim)
            }
            .buttonStyle(.plain)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .background(
            Capsule()
                .fill(CodaHerdrTheme.elevatedBG.opacity(0.97))
                .overlay(Capsule().strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
                .shadow(color: .black.opacity(0.45), radius: 14, x: 0, y: 6)
        )
        .onTapGesture(perform: onTap)
    }
}
