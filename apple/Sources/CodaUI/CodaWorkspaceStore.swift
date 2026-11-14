import Combine
import CodaGhosttyKit
import CodaKit
import CoreGraphics
import Foundation

public enum CodaMobileTab: String, CaseIterable, Sendable {
    case inbox
    case spaces
}

public enum CodaDuoSplitMode: String, CaseIterable, Sendable, Identifiable {
    case inboxAndTerminal
    case studioDiffSplit
    case dualTerminalSplit
    case expandedCanvas

    public var id: String { rawValue }

    public func title(locale: CodaUILocale) -> String {
        switch self {
        case .inboxAndTerminal:
            return locale == .zh ? "分栏" : "Split"
        case .studioDiffSplit:
            return locale == .zh ? "差异" : "Diff"
        case .dualTerminalSplit:
            return locale == .zh ? "双窗" : "Dual"
        case .expandedCanvas:
            return locale == .zh ? "全宽" : "Canvas"
        }
    }
}

public struct CodaInboxSection: Identifiable, Sendable, Equatable {
    public let id: String
    public let title: String
    public let accentKind: HerdrLifecycleState
    public let sessions: [CodaAgentSession]
}

public struct CodaMachineSpacesGroup: Identifiable, Sendable, Equatable {
    public let id: UUID
    public let device: CodaRemoteDevice
    public let routeMode: SmartPathRouteMode?
    public let repos: [CodaRepoWorkspaceSummary]
    public let sessions: [CodaAgentSession]
}

public struct CodaToastBanner: Identifiable, Equatable, Sendable {
    public let id: UUID = UUID()
    public let sessionId: String
    public let workspaceName: String
    public let title: String
    public let state: HerdrLifecycleState
}

@MainActor
public final class CodaWorkspaceStore: ObservableObject {
    @Published public var settings: CodaPersistedSettings
    @Published public var localIdentity: CodaDeviceIdentity

    @Published public var selectedTab: CodaMobileTab = .inbox
    @Published public var selectedSessionID: String? = nil
    @Published public var showSidebar: Bool = true
    @Published public var showSettingsSheet: Bool = false
    @Published public var settingsInitialFocusPairing: Bool = false
    @Published public var showComposeSheet: Bool = false
    @Published public var showPaneMinimapSheet: Bool = false
    @Published public var showGitDiffSheet: Bool = false
    @Published public var searchQuery: String = ""
    @Published public var expandedWorkspaces: Set<String> = []

    @Published public var sessions: [CodaAgentSession] = []
    @Published public var remoteMachines: [CodaRemoteDevice] = []
    @Published public var workspacesByDevice: [UUID: [CodaRepoWorkspaceSummary]] = [:]
    @Published public var machineRouteModes: [UUID: SmartPathRouteMode] = [:]
    @Published public var terminalLinesBySession: [String: [GhosttyStyledLine]] = [:]
    @Published public var rawScreenTextBySession: [String: String] = [:]
    @Published public var gitDiffResult: CodaGitDiffResult? = nil
    @Published public var gitDiffLoading: Bool = false

    @Published public var ctrlActive: Bool = false
    @Published public var altActive: Bool = false
    @Published public var dpadVisible: Bool = false
    @Published public var composeText: String = ""
    @Published public var uploadStatusBanner: String? = nil
    @Published public var isSendingAction: Bool = false

    @Published public var pendingDeviceAuth: CodaDeviceAuthStart? = nil
    @Published public var isPollingDeviceAuth: Bool = false
    @Published public var pairingCode: String? = nil
    @Published public var pairingCommand: String? = nil
    @Published public var authError: String? = nil
    @Published public var isRefreshing: Bool = false
    @Published public var activeToast: CodaToastBanner? = nil
    @Published public var terminalColumns: Int = 56
    @Published public var terminalRows: Int = 28
    @Published public var terminalFontSize: CGFloat = 12.5
    @Published public var formFactor: CodaFormFactor = .iphonePortrait
    @Published public var foldPosture: CodaFoldPosture = .flat
    @Published public var edgeSafeAreas: CodaEdgeSafeAreas = .zero
    @Published public var duoSplitMode: CodaDuoSplitMode = .inboxAndTerminal
    @Published public var secondarySessionID: String? = nil
    @Published public var showTactileTwoRowKeypad: Bool = false

    private var networkClient: CodaNetworkClient?
    private var networkClientURL: URL?
    private var pollLoopTask: Task<Void, Never>?
    private var deviceAuthTask: Task<Void, Never>?
    private var readCompletedIDs: Set<String> = []
    private var previousStates: [String: HerdrLifecycleState] = [:]
    private var lastToastAt: Date = .distantPast
    private var lastViewportSize: CGSize = .zero
    private var lastIsHorizontalRegular: Bool? = nil
    private var lastIsVerticalRegular: Bool? = nil

    public init() {
        let loadedSettings = CodaPersistedSettings.load()
        self.settings = loadedSettings

        #if os(iOS)
        let defaultName = "iOS-Client"
        #else
        let defaultName = "Mac-Client"
        #endif
        self.localIdentity = CodaDeviceIdentity.loadOrCreatePersisted(defaultName: defaultName)

        if let url = URL(string: loadedSettings.serverBaseURLString), !loadedSettings.sessionToken.isEmpty {
            let client = CodaNetworkClient(serverBaseURL: url)
            self.networkClient = client
            self.networkClientURL = url
            Task { [weak self] in
                await client.setToken(loadedSettings.sessionToken)
                await self?.refreshMachinesAndSessions()
            }
        }
    }

    public var isSignedIn: Bool {
        !settings.sessionToken.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    public var locale: CodaUILocale {
        settings.locale
    }

    public func setLocale(_ next: CodaUILocale) {
        settings.locale = next
        settings.save()
    }

    public func updateServerURL(_ raw: String) {
        settings.serverBaseURLString = raw
        settings.save()
        if let url = URL(string: normalizedServerURLString()) {
            let client = CodaNetworkClient(serverBaseURL: url)
            self.networkClient = client
            self.networkClientURL = url
            let tok = settings.sessionToken
            if !tok.isEmpty {
                Task { await client.setToken(tok) }
            }
        }
    }

    private func normalizedServerURLString() -> String {
        let trimmed = settings.serverBaseURLString.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.isEmpty { return "https://codahub.dev" }
        return trimmed.hasSuffix("/") ? String(trimmed.dropLast()) : trimmed
    }

    private func ensureClient() -> CodaNetworkClient? {
        guard let url = URL(string: normalizedServerURLString()) else {
            authError = tr("Invalid server URL.", "服务器地址格式无效。", locale: locale)
            return nil
        }
        if let existing = networkClient, networkClientURL == url {
            return existing
        }
        let client = CodaNetworkClient(serverBaseURL: url)
        let tok = settings.sessionToken
        if !tok.isEmpty {
            Task { await client.setToken(tok) }
        }
        self.networkClient = client
        self.networkClientURL = url
        return client
    }

    // MARK: - Computed Collections (Inbox, Attention Count, Siblings, Spaces)

    public var selectedSession: CodaAgentSession? {
        guard let id = selectedSessionID else { return nil }
        return sessions.first(where: { $0.id == id })
    }

    public var secondarySession: CodaAgentSession? {
        if let secId = secondarySessionID, let match = sessions.first(where: { $0.id == secId }) {
            return match
        }
        return filteredSessions.first(where: { $0.id != selectedSessionID }) ?? selectedSession
    }

    public var attentionBadgeCount: Int {
        sessions.filter { s in
            s.state == .needsYou || s.state == .error || (s.state == .done && s.unreadCompletion)
        }.count
    }

    public var filteredSessions: [CodaAgentSession] {
        let q = searchQuery.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        let base: [CodaAgentSession]
        if q.isEmpty {
            base = sessions
        } else {
            base = sessions.filter { s in
                s.taskTitle.lowercased().contains(q)
                    || s.workspaceName.lowercased().contains(q)
                    || s.hostName.lowercased().contains(q)
                    || s.branch.lowercased().contains(q)
                    || s.agentName.lowercased().contains(q)
                    || (s.statusMessage?.lowercased().contains(q) ?? false)
            }
        }
        return base.sorted(by: CodaAgentSession.inboxOrder)
    }

    public var groupedInboxSections: [CodaInboxSection] {
        let sorted = filteredSessions
        let needsYou = sorted.filter { $0.state == .needsYou || $0.state == .error }
        let done = sorted.filter { $0.state == .done }
        let working = sorted.filter { $0.state == .working }
        let idle = sorted.filter { $0.state == .idle }

        var sections: [CodaInboxSection] = []
        if !needsYou.isEmpty {
            sections.append(CodaInboxSection(
                id: "needs_you",
                title: tr("Needs you", "待确认", locale: locale),
                accentKind: .needsYou,
                sessions: needsYou
            ))
        }
        if !done.isEmpty {
            sections.append(CodaInboxSection(
                id: "done",
                title: tr("Done", "已完成", locale: locale),
                accentKind: .done,
                sessions: done
            ))
        }
        if !working.isEmpty {
            sections.append(CodaInboxSection(
                id: "working",
                title: tr("Working", "运行中", locale: locale),
                accentKind: .working,
                sessions: working
            ))
        }
        if !idle.isEmpty {
            sections.append(CodaInboxSection(
                id: "idle",
                title: tr("Idle", "空闲", locale: locale),
                accentKind: .idle,
                sessions: idle
            ))
        }
        return sections
    }

    public var siblingSessionsForSelected: [CodaAgentSession] {
        guard let current = selectedSession else { return [] }
        return sessions.filter { $0.deviceId == current.deviceId && $0.workspaceName == current.workspaceName }
    }

    public var activeRouteModeForSelected: SmartPathRouteMode? {
        guard let devId = selectedSession?.deviceId else {
            return remoteMachines.first(where: { $0.isOnline }).flatMap { machineRouteModes[$0.id] }
        }
        return machineRouteModes[devId]
    }

    public var spacesByMachine: [CodaMachineSpacesGroup] {
        let q = searchQuery.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        return remoteMachines.map { dev in
            let devSessions = sessions.filter { $0.deviceId == dev.id }
                .filter { s in
                    q.isEmpty
                        || s.workspaceName.lowercased().contains(q)
                        || s.taskTitle.lowercased().contains(q)
                        || s.branch.lowercased().contains(q)
                        || dev.name.lowercased().contains(q)
                }
            let rawRepos = workspacesByDevice[dev.id] ?? []
            let repos = rawRepos.filter { repo in
                q.isEmpty
                    || repo.repoName.lowercased().contains(q)
                    || dev.name.lowercased().contains(q)
                    || repo.branches.contains(where: { $0.branch.lowercased().contains(q) })
                    || devSessions.contains(where: { $0.workspaceName == repo.repoName })
            }
            return CodaMachineSpacesGroup(
                id: dev.id,
                device: dev,
                routeMode: machineRouteModes[dev.id],
                repos: repos,
                sessions: devSessions
            )
        }
    }

    // MARK: - Polling Lifecycle

    public func startLiveSync() {
        pollLoopTask?.cancel()
        pollLoopTask = Task { [weak self] in
            var tick = 0
            while !Task.isCancelled {
                guard let self else { return }
                if self.isSignedIn {
                    if tick % 3 == 0 {
                        await self.refreshMachinesAndSessions()
                    } else if let selected = self.selectedSession {
                        await self.refreshTerminalScreen(for: selected)
                    }
                }
                tick += 1
                try? await Task.sleep(nanoseconds: 1_400_000_000)
            }
        }
    }

    public func stopLiveSync() {
        pollLoopTask?.cancel()
        pollLoopTask = nil
    }

    // MARK: - Authentication & Machine Pairing (RFC 8628, Password, 6-Digit Host Pair)

    public func beginBrowserDeviceAuth() async {
        authError = nil
        pendingDeviceAuth = nil
        deviceAuthTask?.cancel()
        guard let client = ensureClient() else { return }

        do {
            #if os(iOS)
            let platform = formFactor.platformIdentifier(forMacOS: false)
            #else
            let platform = "macos"
            #endif
            let started = try await client.startDeviceAuth(
                deviceName: localIdentity.name,
                role: "client",
                platform: platform,
                identity: localIdentity
            )
            pendingDeviceAuth = started
            isPollingDeviceAuth = true

            deviceAuthTask = Task { [weak self] in
                guard let self else { return }
                let maxAttempts = max(30, started.expiresInSecs / max(1, started.intervalSecs))
                for _ in 0..<maxAttempts {
                    if Task.isCancelled { return }
                    try? await Task.sleep(nanoseconds: UInt64(max(1, started.intervalSecs)) * 1_000_000_000)
                    guard !Task.isCancelled else { return }
                    do {
                        let res = try await client.pollDeviceAuth(deviceCode: started.deviceCode)
                        if res.state == "approved", let tok = res.sessionToken {
                            self.settings.sessionToken = tok
                            self.settings.ownerHandle = res.ownerHandle ?? ""
                            self.settings.save()
                            if let devId = res.device?.id {
                                self.localIdentity.deviceId = devId
                                CodaDeviceIdentity.savePersisted(self.localIdentity)
                            }
                            self.pendingDeviceAuth = nil
                            self.isPollingDeviceAuth = false
                            await self.ensureClientDeviceRegistered(using: client)
                            await self.refreshMachinesAndSessions()
                            return
                        } else if res.state == "expired" {
                            self.isPollingDeviceAuth = false
                            self.pendingDeviceAuth = nil
                            self.authError = tr("Authorization code expired. Start again.", "授权码已过期，请重试。", locale: self.locale)
                            return
                        }
                    } catch {
                        // Transient network error while waiting for browser approval; keep polling until expiry
                    }
                }
            }
        } catch {
            authError = error.localizedDescription
            isPollingDeviceAuth = false
        }
    }

    public func cancelBrowserDeviceAuth() {
        deviceAuthTask?.cancel()
        deviceAuthTask = nil
        pendingDeviceAuth = nil
        isPollingDeviceAuth = false
    }

    public func signInWithPassword(emailOrHandle: String, password: String) async {
        authError = nil
        guard let client = ensureClient() else { return }
        do {
            let tok = try await client.login(
                emailOrHandle: emailOrHandle.trimmingCharacters(in: .whitespacesAndNewlines),
                password: password,
                clientLabel: localIdentity.name
            )
            settings.sessionToken = tok
            if let me = try? await client.fetchAuthMe() {
                settings.ownerHandle = me.handle
            } else {
                settings.ownerHandle = emailOrHandle
            }
            settings.save()
            await ensureClientDeviceRegistered(using: client)
            await refreshMachinesAndSessions()
        } catch {
            authError = tr("Sign in failed. Check credentials or server URL.", "登录失败，请检查账号密码或服务器地址。", locale: locale)
        }
    }

    public func registerAccount(email: String, handle: String, password: String) async {
        authError = nil
        guard let client = ensureClient() else { return }
        do {
            let cleanEmail = email.trimmingCharacters(in: .whitespacesAndNewlines)
            let cleanHandle = handle.trimmingCharacters(in: .whitespacesAndNewlines)
            let tok = try await client.register(
                email: cleanEmail,
                handle: cleanHandle,
                displayName: cleanHandle.isEmpty ? cleanEmail : cleanHandle,
                password: password
            )
            settings.sessionToken = tok
            settings.ownerHandle = cleanHandle
            settings.save()
            await ensureClientDeviceRegistered(using: client)
            await refreshMachinesAndSessions()
        } catch {
            authError = tr("Registration failed.", "注册失败，请检查输入信息。", locale: locale)
        }
    }

    private func ensureClientDeviceRegistered(using client: CodaNetworkClient) async {
        if localIdentity.deviceId != nil { return }
        #if os(iOS)
        let platform = formFactor.platformIdentifier(forMacOS: false)
        #else
        let platform = "macos"
        #endif
        if let dev = try? await client.registerClientDevice(identity: localIdentity, platform: platform) {
            localIdentity.deviceId = dev.id
            CodaDeviceIdentity.savePersisted(localIdentity)
        }
    }

    public func generateMachinePairingCommand(suggestedName: String?) async {
        authError = nil
        guard let client = ensureClient(), isSignedIn else { return }
        do {
            let name = suggestedName?.trimmingCharacters(in: .whitespacesAndNewlines)
            let res = try await client.createPairingCode(suggestedName: (name?.isEmpty == false) ? name : nil)
            pairingCode = res.code
            pairingCommand = res.oneLiner
        } catch {
            authError = tr("Could not create pairing token.", "生成配对码失败。", locale: locale)
        }
    }

    public func signOut() {
        stopLiveSync()
        deviceAuthTask?.cancel()
        pendingDeviceAuth = nil
        isPollingDeviceAuth = false
        pairingCode = nil
        pairingCommand = nil
        settings.sessionToken = ""
        settings.ownerHandle = ""
        settings.save()
        sessions = []
        remoteMachines = []
        workspacesByDevice = [:]
        machineRouteModes = [:]
        selectedSessionID = nil
        if let client = networkClient {
            Task { await client.disconnectTransport() }
        }
    }

    // MARK: - Real Remote Synchronization

    public func refreshMachinesAndSessions() async {
        guard isSignedIn, let client = ensureClient() else { return }
        if isRefreshing { return }
        isRefreshing = true
        defer { isRefreshing = false }

        do {
            await ensureClientDeviceRegistered(using: client)
            let allDevices = try await client.listDevices()
            let hosts = allDevices.filter { $0.role == "daemon" || $0.role == "hybrid" }
            self.remoteMachines = hosts

            let pinnedSet = Set(settings.pinnedSessionIDs)
            var discoveredSessions: [CodaAgentSession] = []
            var nextWorkspaces: [UUID: [CodaRepoWorkspaceSummary]] = [:]

            for host in hosts where host.isOnline {
                do {
                    let route = try await client.connectBestAvailable(
                        sessionToken: settings.sessionToken,
                        targetDevice: host,
                        clientIdentity: localIdentity
                    )
                    machineRouteModes[host.id] = route

                    let wsRes = try? await client.listWorkspaces(
                        on: host,
                        clientIdentity: localIdentity,
                        clientDeviceId: localIdentity.deviceId ?? host.id
                    )
                    let repos = wsRes?.repos ?? []
                    nextWorkspaces[host.id] = repos
                    if let vc = wsRes?.voiceConfig, settings.voiceConfig.apiKey.isEmpty, !vc.apiKey.isEmpty {
                        settings.voiceConfig = vc
                        settings.save()
                    }

                    let terminals = (try? await client.listTerminals(
                        on: host,
                        clientIdentity: localIdentity,
                        clientDeviceId: localIdentity.deviceId ?? host.id,
                        includeFinished: true
                    )) ?? []

                    let built = correlateSessions(
                        host: host,
                        repos: repos,
                        terminals: terminals,
                        pinnedSet: pinnedSet
                    )
                    discoveredSessions.append(contentsOf: built)
                } catch {
                    // Keep iterating other paired hosts if one host tunnel is temporarily resetting
                }
            }

            if !discoveredSessions.isEmpty || !hosts.isEmpty {
                emitTransitionToastsIfNeeded(newSessions: discoveredSessions)
                self.sessions = discoveredSessions.sorted(by: CodaAgentSession.inboxOrder)
                for (k, v) in nextWorkspaces {
                    self.workspacesByDevice[k] = v
                }
                for repo in nextWorkspaces.values.flatMap({ $0 }) {
                    if expandedWorkspaces.isEmpty {
                        expandedWorkspaces.insert(repo.id)
                    }
                }
            }

            if let selected = selectedSession {
                await refreshTerminalScreen(for: selected)
            }
        } catch {
            // Do not wipe existing cached sessions on transient offline blips
        }
    }

    private func correlateSessions(
        host: CodaRemoteDevice,
        repos: [CodaRepoWorkspaceSummary],
        terminals: [CodaHerdrTerminalSummary],
        pinnedSet: Set<String>
    ) -> [CodaAgentSession] {
        // Count terminals sharing the same CWD / workspace for helper count display
        var cwdTotals: [String: Int] = [:]
        var cwdWorking: [String: Int] = [:]
        for t in terminals {
            let st = HerdrLifecycleState.from(rawState: t.status?.state, kind: t.status?.kind)
            cwdTotals[t.cwd, default: 0] += 1
            if st == .working { cwdWorking[t.cwd, default: 0] += 1 }
        }

        return terminals.map { term in
            let sid = "\(host.id.uuidString.lowercased())/\(term.address)"
            let matchedRepo = repos.first(where: { repo in
                term.cwd.hasPrefix(repo.rootPath)
                    || repo.branches.contains(where: { term.cwd.hasPrefix($0.worktreePath) })
            })
            let matchedBranch = matchedRepo?.branches.first(where: { term.cwd.hasPrefix($0.worktreePath) })
                ?? matchedRepo?.branches.first

            let workspace = matchedRepo?.repoName
                ?? URL(fileURLWithPath: term.cwd).lastPathComponent.nilIfEmpty
                ?? host.name

            let agentName = term.status?.app
                ?? term.argv.first.map { URL(fileURLWithPath: $0).lastPathComponent }
                ?? "agent"

            let state = HerdrLifecycleState.from(rawState: term.status?.state, kind: term.status?.kind)
            let prevState = previousStates[sid]
            let existing = sessions.first(where: { $0.id == sid })

            var unread = existing?.unreadCompletion ?? false
            if state == .done && !readCompletedIDs.contains(sid) {
                if prevState != .done || selectedSessionID != sid {
                    unread = true
                }
            }
            if state != .done {
                unread = false
                readCompletedIDs.remove(sid)
            }
            if selectedSessionID == sid && state == .done {
                unread = false
                readCompletedIDs.insert(sid)
            }

            let rawTitle = term.status?.title?.nilIfEmpty
                ?? term.name?.nilIfEmpty
                ?? term.status?.msg?.nilIfEmpty
                ?? "\(agentName) · \(term.address)"

            let totalInCwd = max(0, (cwdTotals[term.cwd] ?? 1) - 1)
            let workingInCwd = max(0, (cwdWorking[term.cwd] ?? 0) - (state == .working ? 1 : 0))

            return CodaAgentSession(
                id: sid,
                terminalAddress: term.address,
                deviceId: host.id,
                workspaceName: workspace,
                cwd: term.cwd,
                branch: matchedBranch?.branch ?? "main",
                hostName: host.name,
                agentName: agentName,
                taskTitle: rawTitle,
                statusMessage: term.status?.msg,
                statusKind: term.status?.kind,
                state: state,
                helperCount: totalInCwd,
                workingHelperCount: workingInCwd,
                unreadCompletion: unread,
                isPinned: pinnedSet.contains(sid),
                updatedAt: (existing?.state == state ? existing?.updatedAt : Date()) ?? Date(),
                cols: Int(term.cols),
                rows: Int(term.rows),
                alive: term.alive,
                diffAdded: Int(matchedBranch?.diffAdded ?? 0),
                diffDeleted: Int(matchedBranch?.diffDeleted ?? 0),
                screenPreview: existing?.screenPreview ?? rawScreenTextBySession[sid] ?? ""
            )
        }
    }

    private func emitTransitionToastsIfNeeded(newSessions: [CodaAgentSession]) {
        for s in newSessions {
            let prev = previousStates[s.id]
            previousStates[s.id] = s.state

            guard prev != nil, prev != s.state, selectedSessionID != s.id else { continue }
            guard s.state == .needsYou || s.state == .done else { continue }
            if Date().timeIntervalSince(lastToastAt) < 2.0 { continue }

            lastToastAt = Date()
            let banner = CodaToastBanner(
                sessionId: s.id,
                workspaceName: s.workspaceName,
                title: s.taskTitle,
                state: s.state
            )
            activeToast = banner
            Task { [weak self] in
                try? await Task.sleep(nanoseconds: 4_000_000_000)
                if self?.activeToast?.id == banner.id {
                    self?.activeToast = nil
                }
            }
        }
    }

    public func refreshTerminalScreen(for session: CodaAgentSession) async {
        guard let client = networkClient,
              let devId = session.deviceId,
              let dev = remoteMachines.first(where: { $0.id == devId }),
              let clientDevId = localIdentity.deviceId else { return }

        do {
            let res = try await client.readTerminalScreen(
                on: dev,
                clientIdentity: localIdentity,
                clientDeviceId: clientDevId,
                terminal: session.terminalAddress,
                historyLines: 180
            )
            rawScreenTextBySession[session.id] = res.text
            let rendered = GhosttyReflowEngine.renderStyledLines(
                rawOutput: res.text,
                targetColumns: terminalColumns
            )
            terminalLinesBySession[session.id] = rendered.lines

            if let idx = sessions.firstIndex(where: { $0.id == session.id }) {
                sessions[idx].screenPreview = String(res.text.suffix(420))
                if let st = rendered.latestStatus {
                    sessions[idx].state = st.state
                    if let msg = st.message { sessions[idx].statusMessage = msg }
                    if let title = st.title { sessions[idx].taskTitle = title }
                } else if let wireStatus = res.status {
                    sessions[idx].state = HerdrLifecycleState.from(rawState: wireStatus.state, kind: wireStatus.kind)
                    if let msg = wireStatus.msg { sessions[idx].statusMessage = msg }
                }
            }
        } catch {
            // Keep previously cached screen lines if poll hit temporary timeout
        }
    }

    // MARK: - Interactive Controls (Selection, Swipe, Quick Bar, Compose, Launch, Diff)

    public func selectSession(_ session: CodaAgentSession) {
        selectedSessionID = session.id
        if let idx = sessions.firstIndex(where: { $0.id == session.id }) {
            if sessions[idx].unreadCompletion {
                sessions[idx].unreadCompletion = false
                readCompletedIDs.insert(session.id)
            }
        }
        Task { await refreshTerminalScreen(for: session) }
    }

    public func selectSecondarySession(_ session: CodaAgentSession) {
        secondarySessionID = session.id
        if let idx = sessions.firstIndex(where: { $0.id == session.id }) {
            if sessions[idx].unreadCompletion {
                sessions[idx].unreadCompletion = false
                readCompletedIDs.insert(session.id)
            }
        }
        Task { await refreshTerminalScreen(for: session) }
    }

    public func setDuoSplitMode(_ mode: CodaDuoSplitMode) {
        duoSplitMode = mode
        if mode == .dualTerminalSplit && (secondarySessionID == nil || secondarySessionID == selectedSessionID) {
            secondarySessionID = filteredSessions.first(where: { $0.id != selectedSessionID })?.id ?? selectedSessionID
        }
        if mode == .studioDiffSplit && gitDiffResult == nil {
            loadGitDiffForSelected(presentModal: false)
        }
        if lastViewportSize.width > 0 && lastViewportSize.height > 0 {
            updateViewportGeometry(
                size: lastViewportSize,
                safeAreas: edgeSafeAreas,
                isHorizontalRegular: lastIsHorizontalRegular,
                isVerticalRegular: lastIsVerticalRegular
            )
        }
    }

    public func setFoldPosture(_ next: CodaFoldPosture) {
        foldPosture = next
        if lastViewportSize.width > 0 && lastViewportSize.height > 0 {
            updateViewportGeometry(
                size: lastViewportSize,
                safeAreas: edgeSafeAreas,
                isHorizontalRegular: lastIsHorizontalRegular,
                isVerticalRegular: lastIsVerticalRegular
            )
        }
    }

    public func updateViewportGeometry(
        size: CGSize,
        safeAreas: CodaEdgeSafeAreas = .zero,
        isHorizontalRegular: Bool? = nil,
        isVerticalRegular: Bool? = nil
    ) {
        guard size.width > 0 && size.height > 0 else { return }
        lastViewportSize = size
        lastIsHorizontalRegular = isHorizontalRegular
        lastIsVerticalRegular = isVerticalRegular
        edgeSafeAreas = safeAreas

        let nextFactor = CodaFormFactor.detect(
            width: size.width,
            height: size.height,
            isHorizontalRegular: isHorizontalRegular,
            isVerticalRegular: isVerticalRegular
        )
        formFactor = nextFactor

        let usableW = max(240, size.width - safeAreas.leading - safeAreas.trailing)
        let usableH = max(180, size.height - safeAreas.top - safeAreas.bottom)

        let paneW: CGFloat
        let paneH: CGFloat
        switch nextFactor {
        case .iphonePortrait:
            paneW = usableW
            paneH = max(180, usableH - 118)
        case .iphoneDuoCoverPortrait:
            paneW = usableW
            paneH = max(180, usableH - 96)
        case .iphoneLandscape, .iphoneDuoCoverLandscape:
            paneW = max(280, usableW - CodaEdgeSafeAreas.duoSideRailWidth)
            paneH = max(160, usableH - 64)
        case .iphoneDuoInnerLandscape:
            if duoSplitMode == .expandedCanvas {
                paneW = max(360, usableW - CodaEdgeSafeAreas.duoSideRailWidth)
            } else {
                paneW = max(260, floor(size.width * 0.5) - 12 - safeAreas.trailing)
            }
            paneH = max(200, usableH - 76)
        case .iphoneDuoInnerPortrait:
            if foldPosture == .tabletopLaptop {
                paneW = usableW
                paneH = max(160, floor(size.height * 0.5) - 12 - safeAreas.top - 42)
            } else {
                paneW = showSidebar ? max(300, usableW - 236) : usableW
                paneH = max(220, usableH - 96)
            }
        case .ipadOrDesktop:
            paneW = max(360, usableW - (showSidebar ? 288 : 0))
            paneH = max(220, usableH - 110)
        }

        let charWidth = max(6.5, terminalFontSize * 0.61)
        let lineHeight = max(14.0, terminalFontSize * 1.35)
        let nextCols = max(32, Int((paneW - 28) / charWidth))
        let nextRows = max(12, Int(paneH / lineHeight))

        if !nextFactor.isCompact {
            if selectedSessionID == nil, let first = filteredSessions.first {
                selectSession(first)
            }
            if secondarySessionID == nil || secondarySessionID == selectedSessionID {
                secondarySessionID = filteredSessions.first(where: { $0.id != selectedSessionID })?.id
            }
        }
        if nextFactor == .iphoneDuoInnerLandscape && duoSplitMode == .studioDiffSplit && gitDiffResult == nil {
            loadGitDiffForSelected(presentModal: false)
        }

        let dimsChanged = (nextCols != terminalColumns || nextRows != terminalRows)
        if dimsChanged {
            terminalColumns = nextCols
            terminalRows = nextRows
            for (sid, raw) in rawScreenTextBySession {
                terminalLinesBySession[sid] = GhosttyReflowEngine.renderStyledLines(
                    rawOutput: raw,
                    targetColumns: nextCols
                ).lines
            }
            if let session = selectedSession,
               let client = networkClient,
               let devId = session.deviceId,
               let dev = remoteMachines.first(where: { $0.id == devId }),
               let clientDevId = localIdentity.deviceId {
                Task {
                    try? await client.resizeTerminalPty(
                        on: dev,
                        clientIdentity: localIdentity,
                        clientDeviceId: clientDevId,
                        terminal: session.terminalAddress,
                        cols: nextCols,
                        rows: nextRows
                    )
                }
            }
        }
    }

    public func closeTerminalToInbox() {
        selectedSessionID = nil
        dpadVisible = false
        ctrlActive = false
        altActive = false
    }

    public func switchSibling(delta: Int) {
        let siblings = siblingSessionsForSelected
        guard siblings.count > 1,
              let cur = selectedSessionID,
              let idx = siblings.firstIndex(where: { $0.id == cur }) else { return }
        let nextIdx = (idx + delta + siblings.count) % siblings.count
        selectSession(siblings[nextIdx])
    }

    public func togglePin(_ session: CodaAgentSession) {
        var pinned = Set(settings.pinnedSessionIDs)
        if pinned.contains(session.id) {
            pinned.remove(session.id)
        } else {
            pinned.insert(session.id)
        }
        settings.pinnedSessionIDs = Array(pinned)
        settings.save()

        if let idx = sessions.firstIndex(where: { $0.id == session.id }) {
            sessions[idx].isPinned = pinned.contains(session.id)
        }
        sessions.sort(by: CodaAgentSession.inboxOrder)
    }

    public func markSessionRead(_ session: CodaAgentSession) {
        readCompletedIDs.insert(session.id)
        if let idx = sessions.firstIndex(where: { $0.id == session.id }) {
            sessions[idx].unreadCompletion = false
        }
        sessions.sort(by: CodaAgentSession.inboxOrder)
    }

    public func interruptSession(_ session: CodaAgentSession) {
        guard let client = networkClient,
              let devId = session.deviceId,
              let dev = remoteMachines.first(where: { $0.id == devId }),
              let clientDevId = localIdentity.deviceId else { return }
        Task {
            try? await client.interruptTerminal(
                on: dev,
                clientIdentity: localIdentity,
                clientDeviceId: clientDevId,
                terminal: session.terminalAddress
            )
            try? await Task.sleep(nanoseconds: 250_000_000)
            await refreshTerminalScreen(for: session)
        }
    }

    public func sendQuickKey(_ key: HerdrQuickKey, to targetSession: CodaAgentSession? = nil) {
        guard let session = targetSession ?? selectedSession,
              let client = networkClient,
              let devId = session.deviceId,
              let dev = remoteMachines.first(where: { $0.id == devId }),
              let clientDevId = localIdentity.deviceId else { return }

        var keys = key.herdrKeyNames
        if ctrlActive && keys.count == 1, !keys[0].lowercased().hasPrefix("ctrl+") {
            keys = ["Ctrl+\(keys[0])"]
        }
        if altActive && keys.count == 1, !keys[0].lowercased().hasPrefix("alt+") {
            keys = ["Alt+\(keys[0])"]
        }
        ctrlActive = false
        altActive = false

        Task {
            try? await client.sendTerminalKeys(
                on: dev,
                clientIdentity: localIdentity,
                clientDeviceId: clientDevId,
                terminal: session.terminalAddress,
                keys: keys
            )
            try? await Task.sleep(nanoseconds: 180_000_000)
            await refreshTerminalScreen(for: session)
        }
    }

    public func sendDirectText(_ text: String, submit: Bool = false, to targetSession: CodaAgentSession? = nil) {
        guard !text.isEmpty,
              let session = targetSession ?? selectedSession,
              let client = networkClient,
              let devId = session.deviceId,
              let dev = remoteMachines.first(where: { $0.id == devId }),
              let clientDevId = localIdentity.deviceId else { return }

        let useCtrl = ctrlActive
        let useAlt = altActive
        ctrlActive = false
        altActive = false

        Task {
            if (useCtrl || useAlt) && text.count == 1, let ch = text.first {
                var combo = ""
                if useCtrl { combo += "Ctrl+" }
                if useAlt { combo += "Alt+" }
                combo += String(ch)
                try? await client.sendTerminalKeys(
                    on: dev,
                    clientIdentity: localIdentity,
                    clientDeviceId: clientDevId,
                    terminal: session.terminalAddress,
                    keys: [combo]
                )
            } else {
                try? await client.sendTerminalText(
                    on: dev,
                    clientIdentity: localIdentity,
                    clientDeviceId: clientDevId,
                    terminal: session.terminalAddress,
                    text: text,
                    submit: submit
                )
            }
            try? await Task.sleep(nanoseconds: 220_000_000)
            await refreshTerminalScreen(for: session)
        }
    }

    public func submitComposePrompt() {
        let trimmed = composeText.trimmingCharacters(in: .newlines)
        guard !trimmed.isEmpty, let session = selectedSession,
              let client = networkClient,
              let devId = session.deviceId,
              let dev = remoteMachines.first(where: { $0.id == devId }),
              let clientDevId = localIdentity.deviceId else { return }

        isSendingAction = true
        let toSend = trimmed
        composeText = ""
        showComposeSheet = false

        Task {
            defer { self.isSendingAction = false }
            try? await client.sendTerminalText(
                on: dev,
                clientIdentity: localIdentity,
                clientDeviceId: clientDevId,
                terminal: session.terminalAddress,
                text: toSend,
                submit: true
            )
            try? await Task.sleep(nanoseconds: 250_000_000)
            await refreshTerminalScreen(for: session)
        }
    }

    public func uploadAttachment(fileURL: URL) {
        guard let session = selectedSession,
              let client = networkClient,
              let devId = session.deviceId,
              let dev = remoteMachines.first(where: { $0.id == devId }),
              let clientDevId = localIdentity.deviceId else { return }

        Task {
            let granted = fileURL.startAccessingSecurityScopedResource()
            defer {
                if granted { fileURL.stopAccessingSecurityScopedResource() }
            }
            guard let data = try? Data(contentsOf: fileURL) else {
                uploadStatusBanner = tr("Could not read selected file.", "读取文件失败。", locale: locale)
                return
            }
            uploadStatusBanner = tr("Uploading \(fileURL.lastPathComponent)...", "正在上传 \(fileURL.lastPathComponent)...", locale: locale)
            do {
                let remotePath = try await client.uploadTerminalFile(
                    on: dev,
                    clientIdentity: localIdentity,
                    clientDeviceId: clientDevId,
                    terminal: session.terminalAddress,
                    filename: fileURL.lastPathComponent,
                    data: data
                )
                let inserted = remotePath ?? fileURL.lastPathComponent
                if composeText.isEmpty {
                    composeText = inserted + " "
                } else {
                    composeText += " \(inserted) "
                }
                uploadStatusBanner = tr("Attached \(inserted)", "已附加 \(inserted)", locale: locale)
            } catch {
                uploadStatusBanner = tr("Upload failed: \(error.localizedDescription)", "上传失败：\(error.localizedDescription)", locale: locale)
            }
        }
    }

    public func launchAgent(agent: String, on device: CodaRemoteDevice, cwd: String?, prompt: String? = nil) {
        guard let client = networkClient,
              let clientDevId = localIdentity.deviceId else { return }
        Task {
            _ = try? await client.launchAgent(
                on: device,
                clientIdentity: localIdentity,
                clientDeviceId: clientDevId,
                agent: agent,
                name: nil,
                cwd: cwd,
                initialPrompt: prompt,
                cols: max(48, terminalColumns),
                rows: 32
            )
            try? await Task.sleep(nanoseconds: 350_000_000)
            await refreshMachinesAndSessions()
        }
    }

    public func loadGitDiffForSelected(presentModal: Bool = true) {
        guard let session = selectedSession,
              let client = networkClient,
              let devId = session.deviceId,
              let dev = remoteMachines.first(where: { $0.id == devId }),
              let clientDevId = localIdentity.deviceId else { return }
        gitDiffLoading = true
        if presentModal {
            showGitDiffSheet = true
        }
        Task {
            defer { self.gitDiffLoading = false }
            self.gitDiffResult = try? await client.fetchGitDiff(
                on: dev,
                clientIdentity: localIdentity,
                clientDeviceId: clientDevId,
                cwd: session.cwd
            )
        }
    }
}

private extension String {
    var nilIfEmpty: String? {
        let t = trimmingCharacters(in: .whitespacesAndNewlines)
        return t.isEmpty ? nil : t
    }
}
