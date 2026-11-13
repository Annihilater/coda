import CodaKit
import SwiftUI

// MARK: - iPhone Mobile Inbox View (`herdr.dev/mobile/` Standard)

public struct CodaMobileInboxTabView: View {
    @ObservedObject public var store: CodaWorkspaceStore
    @FocusState private var searchFocused: Bool

    public init(store: CodaWorkspaceStore) {
        self.store = store
    }

    public var body: some View {
        VStack(spacing: 0) {
            headerBar
            searchBar
                .padding(.horizontal, 16)
                .padding(.top, 8)
                .padding(.bottom, 10)

            if !store.isSignedIn {
                ScrollView {
                    signedOutCard
                        .padding(.horizontal, 16)
                        .padding(.top, 12)
                        .padding(.bottom, 96)
                }
            } else if store.groupedInboxSections.isEmpty {
                ScrollView {
                    emptyInboxCard
                        .padding(.horizontal, 16)
                        .padding(.top, 12)
                        .padding(.bottom, 96)
                }
                .refreshable {
                    await store.refreshMachinesAndSessions()
                }
            } else {
                List {
                    ForEach(store.groupedInboxSections) { section in
                        Section {
                            ForEach(Array(section.sessions.enumerated()), id: \.element.id) { index, session in
                                Button {
                                    store.selectSession(session)
                                } label: {
                                    CodaInboxRowView(
                                        session: session,
                                        locale: store.locale,
                                        isFirst: index == 0,
                                        isLast: index == section.sessions.count - 1
                                    )
                                }
                                .buttonStyle(.plain)
                                .listRowBackground(Color.clear)
                                .listRowSeparator(.hidden)
                                .listRowInsets(EdgeInsets(top: 0, leading: 16, bottom: 0, trailing: 16))
                                .swipeActions(edge: .leading, allowsFullSwipe: true) {
                                    Button {
                                        if session.unreadCompletion {
                                            store.markSessionRead(session)
                                        } else {
                                            store.togglePin(session)
                                        }
                                    } label: {
                                        Label(
                                            session.unreadCompletion
                                                ? tr("Read", "已读", locale: store.locale)
                                                : (session.isPinned
                                                    ? tr("Unpin", "取消置顶", locale: store.locale)
                                                    : tr("Pin", "置顶", locale: store.locale)),
                                            systemImage: session.isPinned ? "pin.slash.fill" : "pin.fill"
                                        )
                                    }
                                    .tint(CodaHerdrTheme.blueUnread)
                                }
                                .swipeActions(edge: .trailing, allowsFullSwipe: true) {
                                    Button(role: .destructive) {
                                        store.interruptSession(session)
                                    } label: {
                                        Label("Esc", systemImage: "xmark.octagon.fill")
                                    }
                                    .tint(CodaHerdrTheme.amberNeedsYou)
                                }
                            }
                        } header: {
                            Text(section.title.uppercased())
                                .font(.system(size: 11, weight: .semibold))
                                .tracking(0.6)
                                .foregroundStyle(section.accentKind == .needsYou ? CodaHerdrTheme.amberNeedsYou : CodaHerdrTheme.textSecondary)
                                .padding(.top, 10)
                                .padding(.bottom, 4)
                                .padding(.horizontal, 4)
                        }
                    }

                    // Bottom safe clearance for floating bottom tab pill
                    Color.clear
                        .frame(height: 88)
                        .listRowBackground(Color.clear)
                        .listRowSeparator(.hidden)
                }
                .listStyle(.plain)
                .scrollContentBackground(.hidden)
                .codaDismissKeyboardInteractively()
                .refreshable {
                    await store.refreshMachinesAndSessions()
                }
            }
        }
        .background(CodaHerdrTheme.canvasBG)
    }

    private var headerBar: some View {
        HStack(spacing: 10) {
            Text(tr("Inbox", "收件箱", locale: store.locale))
                .font(.system(size: 30, weight: .bold))
                .foregroundStyle(CodaHerdrTheme.textPrimary)

            Spacer()

            if store.isRefreshing {
                ProgressView()
                    .controlSize(.small)
                    .tint(CodaHerdrTheme.textSecondary)
            }

            if let route = store.activeRouteModeForSelected {
                Text(route.rawValue)
                    .font(.system(size: 11, weight: .medium, design: .monospaced))
                    .foregroundStyle(route == .lanDirect || route == .cnIPv6P2P ? CodaHerdrTheme.greenDiff : CodaHerdrTheme.cyanWorking)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 4)
                    .background(
                        Capsule()
                            .fill(CodaHerdrTheme.elevatedBG)
                            .overlay(Capsule().strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
                    )
            }

            Button {
                store.settingsInitialFocusPairing = false
                store.showSettingsSheet = true
            } label: {
                Image(systemName: "gearshape")
                    .font(.system(size: 15, weight: .medium))
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
        .padding(.horizontal, 18)
        .padding(.top, 10)
    }

    private var searchBar: some View {
        HStack(spacing: 8) {
            Image(systemName: "magnifyingglass")
                .font(.system(size: 14, weight: .medium))
                .foregroundStyle(CodaHerdrTheme.textSecondary)

            TextField(
                tr("Jump to a terminal...", "跳转到终端...", locale: store.locale),
                text: $store.searchQuery
            )
            .font(.system(size: 14))
            .foregroundStyle(CodaHerdrTheme.textPrimary)
            .focused($searchFocused)
            .codaNoAutocapAndSpelling()

            if !store.searchQuery.isEmpty {
                Button {
                    store.searchQuery = ""
                } label: {
                    Image(systemName: "xmark.circle.fill")
                        .font(.system(size: 14))
                        .foregroundStyle(CodaHerdrTheme.textDim)
                }
                .buttonStyle(.plain)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 9)
        .background(
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .fill(CodaHerdrTheme.elevatedBG)
                .overlay(
                    RoundedRectangle(cornerRadius: 12, style: .continuous)
                        .strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1)
                )
        )
    }

    private var signedOutCard: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 10) {
                Image(systemName: "point.3.connected.trianglepath.dotted")
                    .font(.system(size: 20, weight: .semibold))
                    .foregroundStyle(CodaHerdrTheme.cyanWorking)
                Text(tr("Connect your agent mesh", "连接你的 Agent 网络", locale: store.locale))
                    .font(.system(size: 17, weight: .semibold))
                    .foregroundStyle(CodaHerdrTheme.textPrimary)
            }

            Text(tr(
                "Sign in and pair your Mac or Linux hosts to monitor Claude Code, Codex, Gemini, and shells over zero-knowledge E2EE.",
                "登录并配对你的 Mac 或 Linux 工作站，通过零知识端到端加密随时控制终端会话。",
                locale: store.locale
            ))
            .font(.system(size: 14))
            .foregroundStyle(CodaHerdrTheme.textSecondary)

            Button {
                store.settingsInitialFocusPairing = false
                store.showSettingsSheet = true
            } label: {
                HStack {
                    Image(systemName: "person.badge.key.fill")
                    Text(tr("Sign in & Pair Machine", "登录并配对主机", locale: store.locale))
                }
                .font(.system(size: 14, weight: .semibold))
                .foregroundStyle(.black)
                .frame(maxWidth: .infinity)
                .padding(.vertical, 11)
                .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(CodaHerdrTheme.textPrimary))
            }
            .buttonStyle(.plain)
        }
        .padding(18)
        .background(
            RoundedRectangle(cornerRadius: 16, style: .continuous)
                .fill(CodaHerdrTheme.cardBG)
                .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
        )
    }

    private var emptyInboxCard: some View {
        VStack(alignment: .leading, spacing: 12) {
            if store.remoteMachines.isEmpty {
                Text(tr("No machines paired yet", "尚未配对任何主机", locale: store.locale))
                    .font(.system(size: 16, weight: .semibold))
                    .foregroundStyle(CodaHerdrTheme.textPrimary)
                Text(tr(
                    "Generate a 6-digit pairing command to enroll a Mac or Linux host into your workspace mesh.",
                    "生成 6 位配对指令，将你的 Mac 或 Linux 主机加入加密工作网。",
                    locale: store.locale
                ))
                .font(.system(size: 13))
                .foregroundStyle(CodaHerdrTheme.textSecondary)

                Button {
                    store.settingsInitialFocusPairing = true
                    store.showSettingsSheet = true
                } label: {
                    Label(tr("Add Machine", "添加主机", locale: store.locale), systemImage: "plus")
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundStyle(.black)
                        .padding(.horizontal, 14)
                        .padding(.vertical, 8)
                        .background(Capsule().fill(CodaHerdrTheme.textPrimary))
                }
                .buttonStyle(.plain)
            } else {
                Text(tr("Inbox is clear", "暂无待处理终端", locale: store.locale))
                    .font(.system(size: 16, weight: .semibold))
                    .foregroundStyle(CodaHerdrTheme.textPrimary)
                Text(tr(
                    "Paired hosts are online. Open Spaces to start an agent or shell session.",
                    "主机已在线，前往 Spaces 启动新 Agent 或 Shell 终端。",
                    locale: store.locale
                ))
                .font(.system(size: 13))
                .foregroundStyle(CodaHerdrTheme.textSecondary)

                Button {
                    store.selectedTab = .spaces
                } label: {
                    Label(tr("Open Spaces", "打开 Spaces", locale: store.locale), systemImage: "square.grid.2x2")
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundStyle(CodaHerdrTheme.textPrimary)
                        .padding(.horizontal, 14)
                        .padding(.vertical, 8)
                        .background(
                            Capsule()
                                .fill(CodaHerdrTheme.elevatedBG)
                                .overlay(Capsule().strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
                        )
                }
                .buttonStyle(.plain)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(18)
        .background(
            RoundedRectangle(cornerRadius: 16, style: .continuous)
                .fill(CodaHerdrTheme.cardBG)
                .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
        )
    }
}

// MARK: - Inbox Row Component

public struct CodaInboxRowView: View {
    public let session: CodaAgentSession
    public let locale: CodaUILocale
    public let isFirst: Bool
    public let isLast: Bool

    public init(session: CodaAgentSession, locale: CodaUILocale, isFirst: Bool, isLast: Bool) {
        self.session = session
        self.locale = locale
        self.isFirst = isFirst
        self.isLast = isLast
    }

    private var subtitleLine: String {
        var parts: [String] = []
        if session.helperCount > 0 {
            let hLabel = session.helperCount == 1
                ? tr("1 helper", "1 个子终端", locale: locale)
                : tr("\(session.helperCount) helpers", "\(session.helperCount) 个子终端", locale: locale)
            parts.append(hLabel)
        }
        if session.workingHelperCount > 0 {
            parts.append(tr("\(session.workingHelperCount) working", "\(session.workingHelperCount) 运行中", locale: locale))
        }
        parts.append(session.branch)
        parts.append(session.hostName)
        return parts.joined(separator: " · ")
    }

    public var body: some View {
        let corners = UnevenRoundedRectangle(
            topLeadingRadius: isFirst ? 14 : 0,
            bottomLeadingRadius: isLast ? 14 : 0,
            bottomTrailingRadius: isLast ? 14 : 0,
            topTrailingRadius: isFirst ? 14 : 0,
            style: .continuous
        )

        VStack(spacing: 0) {
            HStack(spacing: 12) {
                ZStack(alignment: .topTrailing) {
                    CodaWorkspaceAvatar(name: session.workspaceName, size: 36)
                    if session.isPinned {
                        Image(systemName: "pin.fill")
                            .font(.system(size: 8))
                            .foregroundStyle(CodaHerdrTheme.amberNeedsYou)
                            .padding(3)
                            .background(Circle().fill(CodaHerdrTheme.canvasBG))
                            .offset(x: 4, y: -4)
                    }
                }

                VStack(alignment: .leading, spacing: 3) {
                    HStack(spacing: 5) {
                        Text(session.workspaceName)
                            .font(.system(size: 14, weight: .semibold))
                            .foregroundStyle(CodaHerdrTheme.textPrimary)
                            .lineLimit(1)

                        Text(session.taskTitle)
                            .font(.system(size: 14, weight: .regular))
                            .foregroundStyle(CodaHerdrTheme.textPrimary.opacity(0.88))
                            .lineLimit(1)
                            .truncationMode(.tail)
                    }

                    Text(subtitleLine)
                        .font(.system(size: 12))
                        .foregroundStyle(CodaHerdrTheme.textSecondary)
                        .lineLimit(1)
                        .truncationMode(.tail)

                    if session.state == .needsYou, let msg = session.statusMessage, !msg.isEmpty {
                        Text(msg)
                            .font(.system(size: 11))
                            .foregroundStyle(CodaHerdrTheme.amberNeedsYou.opacity(0.92))
                            .lineLimit(1)
                            .truncationMode(.tail)
                    }
                }

                Spacer(minLength: 6)

                CodaSessionStatusBadge(session: session, locale: locale)
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 12)

            if !isLast {
                Divider()
                    .background(CodaHerdrTheme.borderHairline)
                    .padding(.leading, 62)
            }
        }
        .background(CodaHerdrTheme.cardBG, in: corners)
        .overlay(corners.strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 0.75))
    }
}

// MARK: - iPhone Spaces Tab (Machines -> Repos/Worktrees -> Live Pane Minimap Grid)

public struct CodaMobileSpacesTabView: View {
    @ObservedObject public var store: CodaWorkspaceStore

    public init(store: CodaWorkspaceStore) {
        self.store = store
    }

    public var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text(tr("Spaces", "空间", locale: store.locale))
                    .font(.system(size: 30, weight: .bold))
                    .foregroundStyle(CodaHerdrTheme.textPrimary)
                Spacer()
                Button {
                    store.settingsInitialFocusPairing = true
                    store.showSettingsSheet = true
                } label: {
                    Image(systemName: "plus")
                        .font(.system(size: 15, weight: .semibold))
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
            .padding(.horizontal, 18)
            .padding(.top, 10)
            .padding(.bottom, 10)

            ScrollView {
                LazyVStack(alignment: .leading, spacing: 18) {
                    if !store.isSignedIn || store.spacesByMachine.isEmpty {
                        VStack(alignment: .leading, spacing: 10) {
                            Text(tr("No connected workstations", "暂无已连接主机", locale: store.locale))
                                .font(.system(size: 16, weight: .semibold))
                                .foregroundStyle(CodaHerdrTheme.textPrimary)
                            Text(tr(
                                "Add a Mac or Linux host to browse repositories, branches, and live terminal panes.",
                                "添加一台 Mac 或 Linux 主机以浏览代码仓库与实时终端窗格。",
                                locale: store.locale
                            ))
                            .font(.system(size: 13))
                            .foregroundStyle(CodaHerdrTheme.textSecondary)
                        }
                        .padding(18)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .background(
                            RoundedRectangle(cornerRadius: 16, style: .continuous)
                                .fill(CodaHerdrTheme.cardBG)
                                .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
                        )
                    } else {
                        ForEach(store.spacesByMachine) { group in
                            machineSection(group)
                        }
                    }
                }
                .padding(.horizontal, 16)
                .padding(.bottom, 96)
            }
            .refreshable {
                await store.refreshMachinesAndSessions()
            }
        }
        .background(CodaHerdrTheme.canvasBG)
    }

    @ViewBuilder
    private func machineSection(_ group: CodaMachineSpacesGroup) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                Circle()
                    .fill(group.device.isOnline ? CodaHerdrTheme.greenDiff : CodaHerdrTheme.textDim)
                    .frame(width: 8, height: 8)
                Text(group.device.name)
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundStyle(CodaHerdrTheme.textPrimary)
                Text(group.device.platform)
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(CodaHerdrTheme.textSecondary)

                Spacer()

                if let route = group.routeMode {
                    Text(route.rawValue)
                        .font(.system(size: 10, weight: .medium, design: .monospaced))
                        .foregroundStyle(CodaHerdrTheme.cyanWorking)
                }

                if group.device.isOnline {
                    launchMenu(device: group.device, cwd: group.repos.first?.rootPath)
                }
            }
            .padding(.horizontal, 4)

            if !group.repos.isEmpty {
                ForEach(group.repos) { repo in
                    VStack(alignment: .leading, spacing: 10) {
                        HStack(spacing: 10) {
                            CodaWorkspaceAvatar(name: repo.repoName, size: 28)
                            VStack(alignment: .leading, spacing: 2) {
                                Text(repo.repoName)
                                    .font(.system(size: 14, weight: .semibold))
                                    .foregroundStyle(CodaHerdrTheme.textPrimary)
                                if let b = repo.branches.first {
                                    HStack(spacing: 6) {
                                        Text(b.branch)
                                            .font(.system(size: 11, design: .monospaced))
                                            .foregroundStyle(CodaHerdrTheme.textSecondary)
                                        if b.diffAdded > 0 || b.diffDeleted > 0 {
                                            Text("+\(b.diffAdded)")
                                                .font(.system(size: 11, design: .monospaced))
                                                .foregroundStyle(CodaHerdrTheme.greenDiff)
                                            Text("-\(b.diffDeleted)")
                                                .font(.system(size: 11, design: .monospaced))
                                                .foregroundStyle(CodaHerdrTheme.redDiff)
                                        }
                                    }
                                }
                            }
                            Spacer()
                            if group.device.isOnline {
                                launchMenu(device: group.device, cwd: repo.rootPath)
                            }
                        }

                        let matching = group.sessions.filter { $0.cwd.hasPrefix(repo.rootPath) || $0.workspaceName == repo.repoName }
                        if !matching.isEmpty {
                            paneMinimapGrid(sessions: matching)
                        }
                    }
                    .padding(14)
                    .background(
                        RoundedRectangle(cornerRadius: 14, style: .continuous)
                            .fill(CodaHerdrTheme.cardBG)
                            .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
                    )
                }
            }

            let repoRoots = Set(group.repos.map(\.rootPath))
            let orphanSessions = group.sessions.filter { s in
                !repoRoots.contains(where: { s.cwd.hasPrefix($0) })
            }
            if !orphanSessions.isEmpty {
                paneMinimapGrid(sessions: orphanSessions)
                    .padding(12)
                    .background(
                        RoundedRectangle(cornerRadius: 14, style: .continuous)
                            .fill(CodaHerdrTheme.cardBG)
                            .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
                    )
            }
        }
    }

    private func paneMinimapGrid(sessions: [CodaAgentSession]) -> some View {
        LazyVGrid(columns: [GridItem(.flexible(), spacing: 10), GridItem(.flexible(), spacing: 10)], spacing: 10) {
            ForEach(sessions) { session in
                Button {
                    store.selectSession(session)
                } label: {
                    VStack(alignment: .leading, spacing: 6) {
                        HStack {
                            Text(session.agentName)
                                .font(.system(size: 11, weight: .semibold, design: .monospaced))
                                .foregroundStyle(CodaHerdrTheme.textPrimary)
                                .lineLimit(1)
                            Spacer()
                            CodaSessionStatusBadge(session: session, locale: store.locale)
                        }

                        Text(session.screenPreview.isEmpty ? session.taskTitle : String(session.screenPreview.suffix(120)))
                            .font(.system(size: 9, design: .monospaced))
                            .foregroundStyle(CodaHerdrTheme.textSecondary)
                            .lineLimit(3)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .frame(height: 36, alignment: .topLeading)
                    }
                    .padding(10)
                    .background(
                        RoundedRectangle(cornerRadius: 10, style: .continuous)
                            .fill(CodaHerdrTheme.canvasBG)
                            .overlay(
                                RoundedRectangle(cornerRadius: 10, style: .continuous)
                                    .strokeBorder(
                                        session.state == .needsYou ? CodaHerdrTheme.amberNeedsYou.opacity(0.6) : CodaHerdrTheme.borderHairline,
                                        lineWidth: 1
                                    )
                            )
                    )
                }
                .buttonStyle(.plain)
            }
        }
    }

    private func launchMenu(device: CodaRemoteDevice, cwd: String?) -> some View {
        Menu {
            Button("Claude Code") { store.launchAgent(agent: "claude", on: device, cwd: cwd) }
            Button("Codex") { store.launchAgent(agent: "codex", on: device, cwd: cwd) }
            Button("Gemini CLI") { store.launchAgent(agent: "gemini", on: device, cwd: cwd) }
            Divider()
            Button("Shell (zsh)") { store.launchAgent(agent: "zsh", on: device, cwd: cwd) }
        } label: {
            Image(systemName: "plus")
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(CodaHerdrTheme.textPrimary)
                .frame(width: 26, height: 26)
                .background(
                    Circle()
                        .fill(CodaHerdrTheme.elevatedBG)
                        .overlay(Circle().strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
                )
        }
    }
}

// MARK: - Bottom Floating Frosted Tab Dock (`Inbox | Spaces`)

public struct CodaFloatingBottomTabPill: View {
    @ObservedObject public var store: CodaWorkspaceStore

    public init(store: CodaWorkspaceStore) {
        self.store = store
    }

    public var body: some View {
        HStack(spacing: 4) {
            tabButton(
                tab: .inbox,
                title: tr("Inbox", "收件箱", locale: store.locale),
                icon: "tray.fill",
                badge: store.attentionBadgeCount
            )
            tabButton(
                tab: .spaces,
                title: tr("Spaces", "空间", locale: store.locale),
                icon: "square.grid.2x2.fill",
                badge: 0
            )
        }
        .padding(5)
        .background(
            Capsule()
                .fill(CodaHerdrTheme.elevatedBG.opacity(0.96))
                .overlay(Capsule().strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
                .shadow(color: .black.opacity(0.55), radius: 18, x: 0, y: 8)
        )
    }

    private func tabButton(tab: CodaMobileTab, title: String, icon: String, badge: Int) -> some View {
        let active = store.selectedTab == tab
        return Button {
            withAnimation(.snappy(duration: 0.2)) {
                store.selectedTab = tab
            }
        } label: {
            HStack(spacing: 6) {
                Image(systemName: icon)
                    .font(.system(size: 13, weight: .semibold))
                Text(title)
                    .font(.system(size: 13, weight: .semibold))
                if badge > 0 {
                    Text("\(badge)")
                        .font(.system(size: 10, weight: .bold))
                        .foregroundStyle(.white)
                        .padding(.horizontal, 5)
                        .padding(.vertical, 1.5)
                        .background(Capsule().fill(CodaHerdrTheme.redDiff))
                }
            }
            .foregroundStyle(active ? CodaHerdrTheme.textPrimary : CodaHerdrTheme.textSecondary)
            .padding(.horizontal, 16)
            .padding(.vertical, 9)
            .background(
                Capsule()
                    .fill(active ? CodaHerdrTheme.pillActiveBG : .clear)
            )
        }
        .buttonStyle(.plain)
    }
}
