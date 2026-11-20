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
    @Published public var hostHealthByDevice: [UUID: CodaHostHealthSnapshot] = [:]
    @Published public var machineRouteModes: [UUID: SmartPathRouteMode] = [:]
    @Published public var terminalLinesBySession: [String: [GhosttyStyledLine]] = [:]
    @Published public var rawScreenTextBySession: [String: String] = [:]
    @Published public var terminalRenderRevisionBySession: [String: UInt64] = [:]
    @Published public var activeSheetSessionID: String? = nil
    @Published public var activeSheetIsSecondary: Bool = false
    @Published public var pendingOpenSettingsAfterCompose: Bool = false
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
    @Published public var voiceConfigSyncMessage: String? = nil
    @Published public var isRecordingVoice: Bool = false
    @Published public var isTranscribingVoice: Bool = false
    @Published public var voiceError: String? = nil
    @Published public var accountStatusMessage: String? = nil

    private var networkClient: CodaNetworkClient?
    private var networkClientURL: URL?
    private let voiceRecorder = CodaVoiceRecorder()
    private var pendingRefresh: Bool = false
    private var pendingForceWorkspaces: Bool = false
    private var pollLoopTask: Task<Void, Never>?
    private var accountEventsTask: Task<Void, Never>?
    private var deviceAuthTask: Task<Void, Never>?
    private var screenRefreshTask: Task<Void, Never>?
    private var voiceTask: Task<Void, Never>?
    private var isStartingVoice: Bool = false
    private var workspaceEpoch: UInt64 = 0
    private var voiceGeneration: UInt64 = 0
    private var gitDiffGeneration: UInt64 = 0
    private var pendingSelectSessionID: String? = nil
    private var readCompletedIDs: Set<String> = []
    private var attachedSessionIDs: Set<String> = []
    private var surfaceEngines: [String: GhosttyReflowEngine] = [:]
    private var lastWorkspaceScanAt: [UUID: Date] = [:]
    private var accountEventsConnectedToken: String? = nil
    private var previousStates: [String: HerdrLifecycleState] = [:]
    private var notificationTracker = CodaNotificationTracker(cooldown: 20.0)
    private var lastToastAt: Date = .distantPast
    private var lastViewportSize: CGSize = .zero
    private var lastIsHorizontalRegular: Bool? = nil
    private var lastIsVerticalRegular: Bool? = nil

    public var onlineMachineCount: Int {
        remoteMachines.filter(\.isOnline).count
    }

    public init() {
        let loadedSettings = CodaPersistedSettings.load()
        self.settings = loadedSettings

        #if os(iOS)
        let defaultName = "iOS-Client"
        #else
        let defaultName = "Mac-Client"
        #endif
        var loadedIdentity = CodaDeviceIdentity.loadOrCreatePersisted(defaultName: defaultName)
        let regKey = CodaPersistedSettings.registrationKey(
            serverBaseURL: loadedSettings.serverBaseURLString,
            ownerHandle: loadedSettings.ownerHandle
        )
        if let savedDevStr = loadedSettings.registeredDeviceIDs[regKey],
           let savedDevUUID = UUID(uuidString: savedDevStr) {
            loadedIdentity.deviceId = savedDevUUID
        }
        self.localIdentity = loadedIdentity

        CodaLocalNotifier.shared.registerSessionTapHandler { [weak self] sid in
            self?.selectSessionByID(sid)
        }

        if !loadedSettings.sessionToken.isEmpty {
            Task { [weak self] in
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

    public func setNotificationsEnabled(_ enabled: Bool) {
        settings.notificationsEnabled = enabled
        settings.save()
        if enabled {
            Task { [weak self] in
                let granted = await CodaLocalNotifier.shared.requestAuthorization()
                guard let self else { return }
                if !granted && CodaLocalNotifier.shared.isAvailableInCurrentProcess {
                    self.accountStatusMessage = tr(
                        "Notifications blocked in system settings",
                        "系统设置已拒绝通知权限，请在系统设置中开启",
                        locale: self.locale
                    )
                }
            }
        }
    }

    public func probeDirectLanNow() async {
        guard let client = await preparedClient(),
              let clientDevId = localIdentity.deviceId else { return }
        let online = remoteMachines.filter(\.isOnline)
        var upgradedAny = false
        for dev in online {
            if let route = await client.probeDirectLanNow(
                for: dev,
                clientIdentity: localIdentity,
                clientDeviceId: clientDevId
            ) {
                machineRouteModes[dev.id] = route
                upgradedAny = true
            }
        }
        accountStatusMessage = upgradedAny
            ? tr("Hot-switched seamlessly to LAN Direct", "已无感热切至局域网直连", locale: locale)
            : tr("LAN Direct unreachable — staying on Smart Relay", "当前网络暂无法直连，继续通过智能中继稳定传输", locale: locale)
    }

    private static func validatedServerURL(from raw: String) -> URL? {
        var trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.isEmpty {
            trimmed = "https://codahub.dev"
        }
        while trimmed.hasSuffix("/") && trimmed.count > "https://".count {
            trimmed.removeLast()
        }
        guard let url = URL(string: trimmed),
              let scheme = url.scheme?.lowercased(),
              let host = url.host?.lowercased(),
              !host.isEmpty else {
            return nil
        }
        if scheme == "https" {
            return url
        }
        if scheme == "http" && (host == "localhost" || host == "127.0.0.1" || host == "::1" || host == "[::1]") {
            return url
        }
        return nil
    }

    public func updateServerURL(_ raw: String) {
        guard let validURL = Self.validatedServerURL(from: raw) else {
            authError = tr(
                "Server URL must use https:// (or http://localhost).",
                "服务器地址必须使用 https://（或本机 http://localhost）。",
                locale: locale
            )
            return
        }
        let prevNormalized = normalizedServerURLString()
        let nextNormalized = validURL.absoluteString.hasSuffix("/")
            ? String(validURL.absoluteString.dropLast())
            : validURL.absoluteString
        let originChanged = prevNormalized.lowercased() != nextNormalized.lowercased()

        authError = nil
        accountStatusMessage = nil
        workspaceEpoch &+= 1
        gitDiffGeneration &+= 1
        let oldClient = self.networkClient
        let wasSignedIn = isSignedIn
        if originChanged && wasSignedIn {
            settings.sessionToken = ""
            settings.ownerHandle = ""
        }
        settings.serverBaseURLString = nextNormalized
        settings.save()
        stopLiveSync()
        deviceAuthTask?.cancel()
        deviceAuthTask = nil
        pendingDeviceAuth = nil
        isPollingDeviceAuth = false
        pairingCode = nil
        pairingCommand = nil
        if let oldClient {
            Task {
                await oldClient.setAsyncEventHandler(nil)
                if originChanged && wasSignedIn {
                    await oldClient.logout()
                }
                await oldClient.disconnectTransport()
            }
        }
        self.networkClient = nil
        self.networkClientURL = nil
        self.accountEventsConnectedToken = nil
        self.pendingRefresh = false
        self.pendingForceWorkspaces = false
        self.attachedSessionIDs.removeAll()
        self.surfaceEngines.removeAll()
        self.lastWorkspaceScanAt.removeAll()
        self.sessions.removeAll()
        self.remoteMachines.removeAll()
        self.workspacesByDevice.removeAll()
        self.machineRouteModes.removeAll()
        self.rawScreenTextBySession.removeAll()
        self.terminalLinesBySession.removeAll()
        self.terminalRenderRevisionBySession.removeAll()
        self.previousStates.removeAll()
        self.readCompletedIDs.removeAll()
        self.notificationTracker.reset()
        self.pendingSelectSessionID = nil
        self.activeSheetSessionID = nil
        self.activeSheetIsSecondary = false
        self.secondarySessionID = nil
        self.selectedSessionID = nil
        self.gitDiffResult = nil
        self.gitDiffLoading = false
        self.composeText = ""
        self.activeToast = nil
        self.uploadStatusBanner = nil
        self.voiceConfigSyncMessage = nil
        cancelVoiceDictation()
        syncIdentityDeviceIdForCurrentRegistration()
        _ = ensureClient()
        if isSignedIn {
            startLiveSync()
        }
    }

    private func normalizedServerURLString() -> String {
        let trimmed = settings.serverBaseURLString.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.isEmpty { return "https://codahub.dev" }
        return trimmed.hasSuffix("/") ? String(trimmed.dropLast()) : trimmed
    }

    private func ensureClient() -> CodaNetworkClient? {
        guard let url = Self.validatedServerURL(from: settings.serverBaseURLString) else {
            authError = tr(
                "Server URL must use https:// (or http://localhost).",
                "服务器地址必须使用 https://（或本机 http://localhost）。",
                locale: locale
            )
            return nil
        }
        if let existing = networkClient, networkClientURL == url {
            return existing
        }
        if let stale = networkClient {
            Task {
                await stale.setAsyncEventHandler(nil)
                await stale.disconnectTransport()
            }
        }
        let client = CodaNetworkClient(serverBaseURL: url)
        self.networkClient = client
        self.networkClientURL = url
        self.attachedSessionIDs.removeAll()
        self.accountEventsConnectedToken = nil
        return client
    }

    /// Ensures the `CodaNetworkClient` instance exists AND has its bearer token and
    /// async peer event handler installed before any network call proceeds.
    private func preparedClient() async -> CodaNetworkClient? {
        guard let client = ensureClient() else { return nil }
        let tok = settings.sessionToken
        if !tok.isEmpty {
            await client.setToken(tok)
        }
        let epoch = self.workspaceEpoch
        await client.setAsyncEventHandler { [weak self] event in
            Task { @MainActor in
                guard let self, self.workspaceEpoch == epoch else { return }
                self.handleAsyncPeerEvent(event)
            }
        }
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

    public var activeSheetSession: CodaAgentSession? {
        if let sid = activeSheetSessionID, let match = sessions.first(where: { $0.id == sid }) {
            return match
        }
        return selectedSession
    }

    public func siblingSessions(for session: CodaAgentSession) -> [CodaAgentSession] {
        sessions.filter { $0.deviceId == session.deviceId && $0.workspaceName == session.workspaceName }
    }

    public var siblingSessionsForSelected: [CodaAgentSession] {
        guard let current = selectedSession else { return [] }
        return siblingSessions(for: current)
    }

    public func openComposeSheet(for session: CodaAgentSession, isSecondary: Bool = false) {
        activeSheetSessionID = session.id
        activeSheetIsSecondary = isSecondary
        showComposeSheet = true
    }

    public func openMinimapSheet(for session: CodaAgentSession, isSecondary: Bool = false) {
        activeSheetSessionID = session.id
        activeSheetIsSecondary = isSecondary
        showPaneMinimapSheet = true
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

    // MARK: - Polling & Account Event WebSocket Lifecycle

    public func startLiveSync() {
        pollLoopTask?.cancel()
        if settings.notificationsEnabled {
            Task {
                await CodaLocalNotifier.shared.requestAuthorizationIfNotDetermined()
            }
        }
        let epoch = self.workspaceEpoch
        if isSignedIn {
            let token = settings.sessionToken
            if !token.isEmpty && accountEventsConnectedToken != token, let client = ensureClient() {
                accountEventsTask?.cancel()
                accountEventsConnectedToken = token
                accountEventsTask = Task { [weak self] in
                    await client.setAsyncEventHandler { [weak self] event in
                        Task { @MainActor in
                            guard let self, self.workspaceEpoch == epoch else { return }
                            self.handleAsyncPeerEvent(event)
                        }
                    }
                    await client.connectAccountEventsWebSocket(sessionToken: token) { [weak self] ev in
                        guard ev.role != "client" else { return }
                        if ev.kind == "connected" || ev.kind == "devices_changed" || ev.kind == "peer_presence" {
                            Task { @MainActor in
                                guard let self, self.workspaceEpoch == epoch else { return }
                                await self.refreshMachinesAndSessions()
                            }
                        }
                    }
                }
            }
        }
        pollLoopTask = Task { [weak self] in
            while !Task.isCancelled {
                guard let self, self.workspaceEpoch == epoch else { return }
                if self.isSignedIn {
                    await self.refreshMachinesAndSessions()
                }
                try? await Task.sleep(nanoseconds: 8_000_000_000)
            }
        }
    }

    public func stopLiveSync() {
        pollLoopTask?.cancel()
        pollLoopTask = nil
        accountEventsTask?.cancel()
        accountEventsTask = nil
        screenRefreshTask?.cancel()
        screenRefreshTask = nil
        accountEventsConnectedToken = nil
        attachedSessionIDs.removeAll()
        surfaceEngines.removeAll()
        lastWorkspaceScanAt.removeAll()
        if let client = networkClient {
            Task { await client.disconnectAccountEventsWebSocket() }
        }
    }

    private func handleAsyncPeerEvent(_ event: CodaAsyncEvent) {
        let hostPrefix = event.sourceDeviceId.uuidString.lowercased()
        if event.op == "route_changed" {
            if let obj = try? JSONSerialization.jsonObject(with: event.payloadData) as? [String: Any],
               let rawRoute = obj["route_mode"] as? String,
               let nextRoute = SmartPathRouteMode(rawValue: rawRoute) {
                let isDirect = (obj["is_direct"] as? Bool) ?? (nextRoute == .lanDirect || nextRoute == .cnIPv6P2P)
                machineRouteModes[event.sourceDeviceId] = nextRoute
                let hostName = remoteMachines.first(where: { $0.id == event.sourceDeviceId })?.name ?? "Host"
                let bannerMsg = isDirect
                    ? tr("Hot-switched to LAN Direct · \(hostName)", "已无感热切至局域网直连 · \(hostName)", locale: locale)
                    : tr("Switched seamlessly to Smart Relay · \(hostName)", "已无缝切回智能中继 · \(hostName)", locale: locale)
                if isDirect {
                    CodaHaptics.success()
                }
                uploadStatusBanner = bannerMsg
                Task { [weak self] in
                    try? await Task.sleep(nanoseconds: 4_000_000_000)
                    if self?.uploadStatusBanner == bannerMsg {
                        self?.uploadStatusBanner = nil
                    }
                }
                // Seamlessly re-attach active PTY(s) over the promoted/failover transport without clearing VT buffers.
                // On Relay -> Direct P2P upgrade, explicitly detach the superseded Relay PTY subscription first
                // so the daemon never dual-streams output across both transports.
                if let client = networkClient,
                   let dev = remoteMachines.first(where: { $0.id == event.sourceDeviceId }),
                   let clientDevId = localIdentity.deviceId {
                    let epoch = self.workspaceEpoch
                    let prefixWithSlash = "\(hostPrefix)/"
                    attachedSessionIDs = attachedSessionIDs.filter { !$0.hasPrefix(prefixWithSlash) }
                    var seenIDs = Set<String>()
                    let candidates: [CodaAgentSession?] = duoSplitMode == .dualTerminalSplit
                        ? [selectedSession, secondarySession]
                        : [selectedSession]
                    let activeForHost = candidates
                        .compactMap { $0 }
                        .filter { $0.deviceId == event.sourceDeviceId && seenIDs.insert($0.id).inserted }
                    for sess in activeForHost {
                        let cols = terminalColumns
                        let rows = terminalRows
                        let ident = localIdentity
                        Task { [weak self] in
                            if isDirect {
                                await client.detachTerminalPtyOverRelay(
                                    on: dev,
                                    clientIdentity: ident,
                                    clientDeviceId: clientDevId,
                                    terminal: sess.terminalAddress
                                )
                            }
                            let ok = (try? await client.attachTerminalPty(
                                on: dev,
                                clientIdentity: ident,
                                clientDeviceId: clientDevId,
                                terminal: sess.terminalAddress,
                                cols: cols,
                                rows: rows
                            )) ?? false
                            if ok, let self, self.workspaceEpoch == epoch {
                                self.attachedSessionIDs.insert(sess.id)
                            }
                        }
                    }
                }
            }
            return
        }

        if event.op == "direct_disconnected_need_relay" {
            let prefixWithSlash = "\(hostPrefix)/"
            attachedSessionIDs = attachedSessionIDs.filter { !$0.hasPrefix(prefixWithSlash) }
            if let client = networkClient,
               let dev = remoteMachines.first(where: { $0.id == event.sourceDeviceId }),
               let clientDevId = localIdentity.deviceId {
                let epoch = self.workspaceEpoch
                var seenIDs = Set<String>()
                let candidates: [CodaAgentSession?] = duoSplitMode == .dualTerminalSplit
                    ? [selectedSession, secondarySession]
                    : [selectedSession]
                let activeForHost = candidates
                    .compactMap { $0 }
                    .filter { $0.deviceId == event.sourceDeviceId && seenIDs.insert($0.id).inserted }
                let token = settings.sessionToken
                let ident = localIdentity
                let cols = terminalColumns
                let rows = terminalRows
                Task { [weak self] in
                    guard let self, self.workspaceEpoch == epoch, !token.isEmpty else { return }
                    do {
                        let nextRoute = try await client.connectBestAvailable(
                            sessionToken: token,
                            targetDevice: dev,
                            clientIdentity: ident
                        )
                        guard self.workspaceEpoch == epoch else { return }
                        self.machineRouteModes[dev.id] = nextRoute
                        for sess in activeForHost {
                            let ok = (try? await client.attachTerminalPty(
                                on: dev,
                                clientIdentity: ident,
                                clientDeviceId: clientDevId,
                                terminal: sess.terminalAddress,
                                cols: cols,
                                rows: rows
                            )) ?? false
                            if ok && self.workspaceEpoch == epoch {
                                self.attachedSessionIDs.insert(sess.id)
                            }
                        }
                        let bannerMsg = tr(
                            "Switched seamlessly to Smart Relay · \(dev.name)",
                            "已无缝切回智能中继 · \(dev.name)",
                            locale: self.locale
                        )
                        self.uploadStatusBanner = bannerMsg
                        try? await Task.sleep(nanoseconds: 4_000_000_000)
                        if self.uploadStatusBanner == bannerMsg {
                            self.uploadStatusBanner = nil
                        }
                    } catch {
                        guard self.workspaceEpoch == epoch else { return }
                        let errMsg = tr(
                            "Direct link dropped · Reconnecting \(dev.name)...",
                            "直连已断开 · 正在重连 \(dev.name)...",
                            locale: self.locale
                        )
                        self.uploadStatusBanner = errMsg
                        try? await Task.sleep(nanoseconds: 4_000_000_000)
                        if self.uploadStatusBanner == errMsg {
                            self.uploadStatusBanner = nil
                        }
                    }
                }
            }
            return
        }

        if event.op == "peer_session_reset" {
            let epoch = self.workspaceEpoch
            let prefixWithSlash = "\(hostPrefix)/"
            attachedSessionIDs = attachedSessionIDs.filter { !$0.hasPrefix(prefixWithSlash) }
            for key in Array(surfaceEngines.keys) where key.hasPrefix(prefixWithSlash) {
                surfaceEngines.removeValue(forKey: key)
            }
            Task { [weak self] in
                guard let self, self.workspaceEpoch == epoch else { return }
                await self.refreshMachinesAndSessions()
                guard self.workspaceEpoch == epoch else { return }
                if let sel = self.selectedSession, sel.deviceId == event.sourceDeviceId {
                    await self.refreshTerminalScreen(for: sel, forceSnapshot: true)
                }
                if self.duoSplitMode == .dualTerminalSplit,
                   let sec = self.secondarySession,
                   sec.id != self.selectedSessionID,
                   sec.deviceId == event.sourceDeviceId {
                    await self.refreshTerminalScreen(for: sec, forceSnapshot: true)
                }
            }
            return
        }

        guard let obj = try? JSONSerialization.jsonObject(with: event.payloadData) as? [String: Any] else {
            return
        }
        if event.op == "pty_output_chunk",
           let terminal = obj["terminal"] as? String,
           let b64 = obj["data_b64"] as? String,
           let rawBytes = Data(base64Encoded: b64) {
            let sid = "\(hostPrefix)/\(terminal)"
            var engine = surfaceEngines[sid] ?? GhosttyReflowEngine(
                columns: terminalColumns,
                rows: terminalRows,
                maxScrollback: 1500,
                treatLFAsCRLF: false
            )
            if engine.columns != terminalColumns || engine.rows != terminalRows {
                engine.resizeReflow(columns: terminalColumns, rows: terminalRows)
            }
            let prevOscGen = engine.oscStatusGeneration
            engine.feed(rawBytes)
            surfaceEngines[sid] = engine
            let visiblePlain = engine.plainVisibleText
            rawScreenTextBySession[sid] = visiblePlain
            terminalLinesBySession[sid] = engine.styledLines
            terminalRenderRevisionBySession[sid, default: 0] &+= 1
            if let idx = sessions.firstIndex(where: { $0.id == sid }) {
                let prevState = sessions[idx].state
                sessions[idx].screenPreview = String(visiblePlain.suffix(420))
                // Only apply OSC 7501 status when THIS chunk actually emitted a fresh OSC 7501 frame;
                // never re-apply sticky `latestHerdrStatus` on plain stdout chunks after approval/live-events.
                if engine.oscStatusGeneration != prevOscGen, let st = engine.latestHerdrStatus {
                    sessions[idx].state = st.state
                    if let msg = st.message { sessions[idx].statusMessage = msg }
                    if let title = st.title { sessions[idx].taskTitle = title }
                    if st.state == .done && prevState != .done && selectedSessionID != sid {
                        sessions[idx].unreadCompletion = true
                        readCompletedIDs.remove(sid)
                    } else if st.state != .done {
                        sessions[idx].unreadCompletion = false
                        readCompletedIDs.remove(sid)
                    }
                    emitTransitionToastsIfNeeded(newSessions: [sessions[idx]])
                }
            }
        } else if event.op == "herdr_live_event" {
            let eventKind = (obj["event_kind"] as? String) ?? ""
            if let termDict = obj["terminal"],
               let termData = try? JSONSerialization.data(withJSONObject: termDict),
               let term = try? JSONDecoder().decode(CodaHerdrTerminalSummary.self, from: termData) {
                let sid = "\(hostPrefix)/\(term.address)"
                if let idx = sessions.firstIndex(where: { $0.id == sid }) {
                    let nextState = HerdrLifecycleState.from(rawState: term.status?.state, kind: term.status?.kind)
                    let prevState = sessions[idx].state
                    sessions[idx].alive = term.alive
                    sessions[idx].state = nextState
                    if let msg = term.status?.msg { sessions[idx].statusMessage = msg }
                    if let title = term.status?.title ?? term.name { sessions[idx].taskTitle = title }
                    if nextState == .done && prevState != .done && selectedSessionID != sid {
                        sessions[idx].unreadCompletion = true
                        readCompletedIDs.remove(sid)
                    } else if nextState != .done {
                        sessions[idx].unreadCompletion = false
                        readCompletedIDs.remove(sid)
                    }
                    emitTransitionToastsIfNeeded(newSessions: [sessions[idx]])
                } else {
                    Task { await self.refreshMachinesAndSessions() }
                }
            } else if eventKind.contains("create") || eventKind.contains("destroy") || eventKind.contains("exit") {
                Task { await self.refreshMachinesAndSessions() }
            }
        }
    }

    // MARK: - Authentication & Machine Pairing (RFC 8628, Password, 6-Digit Host Pair)

    public func beginBrowserDeviceAuth() async {
        authError = nil
        pendingDeviceAuth = nil
        deviceAuthTask?.cancel()
        guard let client = await preparedClient() else { return }

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

            let epoch = self.workspaceEpoch
            deviceAuthTask = Task { [weak self] in
                guard let self else { return }
                let maxAttempts = max(30, started.expiresInSecs / max(1, started.intervalSecs))
                for _ in 0..<maxAttempts {
                    if Task.isCancelled || self.workspaceEpoch != epoch { return }
                    try? await Task.sleep(nanoseconds: UInt64(max(1, started.intervalSecs)) * 1_000_000_000)
                    guard !Task.isCancelled, self.workspaceEpoch == epoch else { return }
                    do {
                        let res = try await client.pollDeviceAuth(deviceCode: started.deviceCode)
                        guard !Task.isCancelled, self.workspaceEpoch == epoch else { return }
                        if res.state == "approved", let tok = res.sessionToken {
                            self.settings.sessionToken = tok
                            self.settings.ownerHandle = res.ownerHandle ?? ""
                            if let devId = res.device?.id {
                                self.localIdentity.deviceId = devId
                                CodaDeviceIdentity.savePersisted(self.localIdentity)
                                let key = CodaPersistedSettings.registrationKey(
                                    serverBaseURL: self.settings.serverBaseURLString,
                                    ownerHandle: self.settings.ownerHandle
                                )
                                self.settings.registeredDeviceIDs[key] = devId.uuidString.lowercased()
                            } else {
                                self.syncIdentityDeviceIdForCurrentRegistration()
                            }
                            self.settings.save()
                            self.pendingDeviceAuth = nil
                            self.isPollingDeviceAuth = false
                            await self.ensureClientDeviceRegistered(using: client)
                            self.startLiveSync()
                            await self.refreshMachinesAndSessions(forceWorkspaces: true)
                            return
                        } else if res.state == "expired" || res.state == "denied" {
                            self.isPollingDeviceAuth = false
                            self.pendingDeviceAuth = nil
                            self.authError = res.state == "denied"
                                ? tr("Authorization denied.", "授权请求已被拒绝。", locale: self.locale)
                                : tr("Authorization code expired. Start again.", "授权码已过期，请重试。", locale: self.locale)
                            return
                        }
                    } catch {
                        // Transient network error while waiting for browser approval; keep polling until expiry
                    }
                }
                if !Task.isCancelled && self.workspaceEpoch == epoch {
                    self.isPollingDeviceAuth = false
                    self.pendingDeviceAuth = nil
                    self.authError = tr("Authorization code expired. Start again.", "授权码已过期，请重试。", locale: self.locale)
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
        accountStatusMessage = nil
        guard let client = await preparedClient() else { return }
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
            syncIdentityDeviceIdForCurrentRegistration()
            settings.save()
            await ensureClientDeviceRegistered(using: client)
            startLiveSync()
            await refreshMachinesAndSessions(forceWorkspaces: true)
        } catch {
            authError = error.localizedDescription
        }
    }

    public func registerAccount(email: String, handle: String, password: String, inviteCode: String? = nil) async {
        authError = nil
        accountStatusMessage = nil
        guard let client = await preparedClient() else { return }
        do {
            let cleanEmail = email.trimmingCharacters(in: .whitespacesAndNewlines)
            let cleanHandle = handle.trimmingCharacters(in: .whitespacesAndNewlines)
            let cleanInvite = inviteCode?.trimmingCharacters(in: .whitespacesAndNewlines)
            let tok = try await client.register(
                email: cleanEmail,
                handle: cleanHandle,
                displayName: cleanHandle.isEmpty ? cleanEmail : cleanHandle,
                password: password,
                inviteCode: (cleanInvite?.isEmpty == false) ? cleanInvite : nil
            )
            settings.sessionToken = tok
            settings.ownerHandle = cleanHandle
            syncIdentityDeviceIdForCurrentRegistration()
            settings.save()
            await ensureClientDeviceRegistered(using: client)
            startLiveSync()
            await refreshMachinesAndSessions(forceWorkspaces: true)
        } catch {
            authError = error.localizedDescription
        }
    }

    private func syncIdentityDeviceIdForCurrentRegistration() {
        let regKey = CodaPersistedSettings.registrationKey(
            serverBaseURL: settings.serverBaseURLString,
            ownerHandle: settings.ownerHandle
        )
        if let saved = settings.registeredDeviceIDs[regKey], let uuid = UUID(uuidString: saved) {
            localIdentity.deviceId = uuid
        } else {
            localIdentity.deviceId = nil
        }
        CodaDeviceIdentity.savePersisted(localIdentity)
    }

    private func ensureClientDeviceRegistered(using client: CodaNetworkClient, force: Bool = false) async {
        let regKey = CodaPersistedSettings.registrationKey(
            serverBaseURL: settings.serverBaseURLString,
            ownerHandle: settings.ownerHandle
        )
        if !force {
            if let saved = settings.registeredDeviceIDs[regKey], let uuid = UUID(uuidString: saved) {
                if localIdentity.deviceId != uuid {
                    localIdentity.deviceId = uuid
                    CodaDeviceIdentity.savePersisted(localIdentity)
                }
                return
            }
            if localIdentity.deviceId != nil {
                localIdentity.deviceId = nil
                CodaDeviceIdentity.savePersisted(localIdentity)
            }
        }
        #if os(iOS)
        let platform = formFactor.platformIdentifier(forMacOS: false)
        #else
        let platform = "macos"
        #endif
        if let dev = try? await client.registerClientDevice(identity: localIdentity, platform: platform) {
            localIdentity.deviceId = dev.id
            CodaDeviceIdentity.savePersisted(localIdentity)
            settings.registeredDeviceIDs[regKey] = dev.id.uuidString.lowercased()
            settings.save()
        }
    }

    public func generateMachinePairingCommand(suggestedName: String?) async {
        authError = nil
        accountStatusMessage = nil
        guard isSignedIn, let client = await preparedClient() else { return }
        do {
            let name = suggestedName?.trimmingCharacters(in: .whitespacesAndNewlines)
            let res = try await client.createPairingCode(suggestedName: (name?.isEmpty == false) ? name : nil)
            pairingCode = res.code
            pairingCommand = res.oneLiner
        } catch {
            authError = error.localizedDescription
        }
    }

    public func approveHostDeviceCode(_ userCode: String) async {
        authError = nil
        accountStatusMessage = nil
        let clean = userCode.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !clean.isEmpty, isSignedIn, let client = await preparedClient() else { return }
        do {
            let approved = try await client.approveDeviceAuth(userCode: clean)
            accountStatusMessage = tr(
                "Approved \(approved.name).",
                "已批准设备 \(approved.name)。",
                locale: locale
            )
            await refreshMachinesAndSessions(forceWorkspaces: true)
        } catch {
            authError = error.localizedDescription
        }
    }

    public func renameRemoteMachine(_ device: CodaRemoteDevice, to newName: String) async {
        authError = nil
        accountStatusMessage = nil
        let clean = newName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !clean.isEmpty, isSignedIn, let client = await preparedClient() else { return }
        do {
            try await client.renameDevice(deviceId: device.id, name: clean)
            accountStatusMessage = tr("Renamed host to \(clean).", "主机已重命名为 \(clean)。", locale: locale)
            await refreshMachinesAndSessions()
        } catch {
            authError = error.localizedDescription
        }
    }

    public func deleteRemoteMachine(_ device: CodaRemoteDevice) async {
        authError = nil
        accountStatusMessage = nil
        guard isSignedIn, let client = await preparedClient() else { return }
        do {
            try await client.deleteDevice(deviceId: device.id)
            let prefix = "\(device.id.uuidString.lowercased())/"
            attachedSessionIDs = attachedSessionIDs.filter { !$0.hasPrefix(prefix) }
            for key in Array(surfaceEngines.keys) where key.hasPrefix(prefix) {
                surfaceEngines.removeValue(forKey: key)
            }
            if let sel = selectedSession, sel.deviceId == device.id {
                selectedSessionID = nil
            }
            remoteMachines.removeAll { $0.id == device.id }
            sessions.removeAll { $0.deviceId == device.id }
            accountStatusMessage = tr("Removed host \(device.name).", "已移除主机 \(device.name)。", locale: locale)
            await refreshMachinesAndSessions()
        } catch {
            authError = error.localizedDescription
        }
    }

    public func changePassword(current: String, new: String) async -> Bool {
        authError = nil
        accountStatusMessage = nil
        guard isSignedIn, let client = await preparedClient() else { return false }
        do {
            if let newToken = try await client.changePassword(currentPassword: current, newPassword: new) {
                settings.sessionToken = newToken
                settings.save()
                await client.setToken(newToken)
            }
            accountStatusMessage = tr("Password updated.", "密码已更新。", locale: locale)
            return true
        } catch {
            authError = error.localizedDescription
            return false
        }
    }

    public func deleteAccount(currentPassword: String) async -> Bool {
        authError = nil
        accountStatusMessage = nil
        guard isSignedIn, let client = await preparedClient() else { return false }
        do {
            try await client.deleteAccount(currentPassword: currentPassword)
            signOut()
            return true
        } catch {
            authError = error.localizedDescription
            return false
        }
    }

    public func resetLocalIdentity() async {
        authError = nil
        accountStatusMessage = nil
        #if os(iOS)
        let defaultName = "iOS-Client"
        #else
        let defaultName = "Mac-Client"
        #endif
        let fresh = CodaDeviceIdentity.resetPersisted(defaultName: defaultName)
        self.localIdentity = fresh
        self.settings.registeredDeviceIDs.removeAll()
        self.settings.save()
        self.attachedSessionIDs.removeAll()
        self.surfaceEngines.removeAll()
        if let client = networkClient {
            await client.disconnectTransport()
        }
        if isSignedIn, let client = await preparedClient() {
            await ensureClientDeviceRegistered(using: client, force: true)
            startLiveSync()
            await refreshMachinesAndSessions(forceWorkspaces: true)
        }
        accountStatusMessage = tr(
            "Local cryptographic identity rotated.",
            "本地设备加密密钥已重置并重新注册。",
            locale: locale
        )
    }

    public func upgradeRemoteMachine(_ device: CodaRemoteDevice) async {
        guard isSignedIn, device.isOnline, let client = await preparedClient() else { return }
        let epoch = self.workspaceEpoch
        accountStatusMessage = tr(
            "Upgrading coda-daemon on \(device.name) (zero-downtime OTA)…",
            "正在为 \(device.name) 无感升级 coda-daemon…",
            locale: locale
        )
        authError = nil
        do {
            let res = try await client.upgradeRemoteDaemon(
                on: device,
                clientIdentity: localIdentity,
                clientDeviceId: localIdentity.deviceId ?? device.id
            )
            guard self.workspaceEpoch == epoch else { return }
            if res.ok {
                let targetVer = res.targetVersion ?? "latest"
                accountStatusMessage = "\(device.name): v\(res.previousVersion) -> v\(targetVer) (\(res.message))"
                CodaHaptics.success()
                try? await Task.sleep(nanoseconds: 1_500_000_000)
                if self.workspaceEpoch == epoch {
                    await refreshMachinesAndSessions(forceWorkspaces: true)
                }
            } else {
                authError = "\(device.name) OTA failed: \(res.message)"
            }
        } catch {
            if self.workspaceEpoch == epoch {
                authError = "\(device.name) OTA error: \(error.localizedDescription)"
            }
        }
    }

    public func signOut() {
        workspaceEpoch &+= 1
        gitDiffGeneration &+= 1
        let oldClient = networkClient
        cancelVoiceDictation()
        voiceError = nil
        stopLiveSync()
        deviceAuthTask?.cancel()
        deviceAuthTask = nil
        pendingDeviceAuth = nil
        isPollingDeviceAuth = false
        pairingCode = nil
        pairingCommand = nil
        authError = nil
        accountStatusMessage = nil
        pendingRefresh = false
        pendingForceWorkspaces = false
        settings.sessionToken = ""
        settings.ownerHandle = ""
        settings.save()
        localIdentity.deviceId = nil
        CodaDeviceIdentity.savePersisted(localIdentity)
        sessions = []
        remoteMachines = []
        workspacesByDevice = [:]
        hostHealthByDevice = [:]
        machineRouteModes = [:]
        rawScreenTextBySession = [:]
        terminalLinesBySession = [:]
        terminalRenderRevisionBySession = [:]
        readCompletedIDs.removeAll()
        previousStates.removeAll()
        notificationTracker.reset()
        pendingSelectSessionID = nil
        activeSheetSessionID = nil
        activeSheetIsSecondary = false
        secondarySessionID = nil
        gitDiffResult = nil
        gitDiffLoading = false
        composeText = ""
        activeToast = nil
        voiceConfigSyncMessage = nil
        uploadStatusBanner = nil
        selectedSessionID = nil
        networkClient = nil
        networkClientURL = nil
        if let oldClient {
            Task {
                await oldClient.setAsyncEventHandler(nil)
                await oldClient.logout()
                await oldClient.disconnectTransport()
            }
        }
    }

    // MARK: - Real Remote Synchronization

    public func refreshMachinesAndSessions(forceWorkspaces: Bool = false) async {
        let epoch = self.workspaceEpoch
        guard isSignedIn, let client = await preparedClient() else { return }
        guard self.workspaceEpoch == epoch, isSignedIn else { return }
        if isRefreshing {
            pendingRefresh = true
            if forceWorkspaces { pendingForceWorkspaces = true }
            return
        }
        isRefreshing = true
        defer {
            isRefreshing = false
            if pendingRefresh && self.workspaceEpoch == epoch && self.isSignedIn {
                let nextForce = pendingForceWorkspaces
                pendingRefresh = false
                pendingForceWorkspaces = false
                Task { [weak self] in
                    await self?.refreshMachinesAndSessions(forceWorkspaces: nextForce)
                }
            } else if self.workspaceEpoch != epoch || !self.isSignedIn {
                pendingRefresh = false
                pendingForceWorkspaces = false
            }
        }

        do {
            await ensureClientDeviceRegistered(using: client)
            guard self.workspaceEpoch == epoch, isSignedIn else { return }
            let allDevices = try await client.listDevices()
            guard self.workspaceEpoch == epoch, isSignedIn else { return }
            let matchingLocal = localIdentity.deviceId.flatMap { myId in
                allDevices.first(where: { $0.id == myId })
            }
            let identityKeyMismatch = matchingLocal.map { dev in
                dev.ed25519Pubkey != localIdentity.ed25519PublicB64
                    || dev.x25519Pubkey != localIdentity.x25519PublicB64
            } ?? true
            if identityKeyMismatch {
                await ensureClientDeviceRegistered(using: client, force: true)
                guard self.workspaceEpoch == epoch, isSignedIn else { return }
            }
            let hosts = allDevices.filter { $0.role == "daemon" || $0.role == "hybrid" }
            self.remoteMachines = hosts

            let pinnedSet = Set(settings.pinnedSessionIDs)
            var discoveredSessions: [CodaAgentSession] = []
            var nextWorkspaces: [UUID: [CodaRepoWorkspaceSummary]] = [:]
            var succeededHostIDs = Set<UUID>()

            for host in hosts where host.isOnline {
                guard self.workspaceEpoch == epoch, isSignedIn else { return }
                do {
                    let route = try await client.connectBestAvailable(
                        sessionToken: settings.sessionToken,
                        targetDevice: host,
                        clientIdentity: localIdentity
                    )
                    guard self.workspaceEpoch == epoch, isSignedIn else { return }
                    machineRouteModes[host.id] = route

                    let shouldScanWorkspaces = forceWorkspaces
                        || lastWorkspaceScanAt[host.id].map({ Date().timeIntervalSince($0) >= 30 }) ?? true
                    let repos: [CodaRepoWorkspaceSummary]
                    if shouldScanWorkspaces {
                        let wsRes = try? await client.listWorkspaces(
                            on: host,
                            clientIdentity: localIdentity,
                            clientDeviceId: localIdentity.deviceId ?? host.id
                        )
                        guard self.workspaceEpoch == epoch, isSignedIn else { return }
                        repos = wsRes?.repos ?? (workspacesByDevice[host.id] ?? [])
                        if wsRes != nil {
                            lastWorkspaceScanAt[host.id] = Date()
                        }
                        if let hh = wsRes?.hostHealth {
                            hostHealthByDevice[host.id] = hh
                        }
                        if let vc = wsRes?.voiceConfig, settings.voiceConfig.apiKey.isEmpty, !vc.apiKey.isEmpty {
                            settings.voiceConfig = vc
                            settings.save()
                        }
                    } else {
                        repos = workspacesByDevice[host.id] ?? []
                    }
                    nextWorkspaces[host.id] = repos

                    let terminals = try await client.listTerminals(
                        on: host,
                        clientIdentity: localIdentity,
                        clientDeviceId: localIdentity.deviceId ?? host.id,
                        includeFinished: true
                    )
                    guard self.workspaceEpoch == epoch, isSignedIn else { return }

                    let built = correlateSessions(
                        host: host,
                        repos: repos,
                        terminals: terminals,
                        pinnedSet: pinnedSet
                    )
                    succeededHostIDs.insert(host.id)
                    discoveredSessions.append(contentsOf: built)
                } catch {
                    // Keep iterating other paired hosts if one host tunnel is temporarily resetting;
                    // previously cached sessions for this still-online host will be preserved below.
                }
            }

            guard self.workspaceEpoch == epoch, isSignedIn else { return }

            // Preserve cached sessions for online hosts whose RPC transiently failed so multi-host
            // glitches never wipe terminals or eject the user from an active session.
            let onlineFailedHostIDs = Set(hosts.filter { $0.isOnline && !succeededHostIDs.contains($0.id) }.map(\.id))
            if !onlineFailedHostIDs.isEmpty {
                let preserved = self.sessions.filter { s in
                    guard let devId = s.deviceId else { return false }
                    return onlineFailedHostIDs.contains(devId)
                }
                discoveredSessions.append(contentsOf: preserved)
            }

            emitTransitionToastsIfNeeded(newSessions: discoveredSessions)
            self.sessions = discoveredSessions.sorted(by: CodaAgentSession.inboxOrder)
            let validHostIDs = Set(hosts.map(\.id))
            self.workspacesByDevice = self.workspacesByDevice.filter { validHostIDs.contains($0.key) }
            for (k, v) in nextWorkspaces {
                self.workspacesByDevice[k] = v
            }
            for repo in nextWorkspaces.values.flatMap({ $0 }) {
                if expandedWorkspaces.isEmpty {
                    expandedWorkspaces.insert(repo.id)
                }
            }
            if let selId = self.selectedSessionID, !self.sessions.contains(where: { $0.id == selId }) {
                self.selectedSessionID = nil
            }
            if let secId = self.secondarySessionID, !self.sessions.contains(where: { $0.id == secId }) {
                self.secondarySessionID = nil
            }

            if let pendingSid = pendingSelectSessionID,
               let match = sessions.first(where: { $0.id == pendingSid }) {
                pendingSelectSessionID = nil
                selectSession(match)
            }

            if let selected = selectedSession {
                await refreshTerminalScreen(for: selected)
            }
            if duoSplitMode == .dualTerminalSplit,
               let secondary = secondarySession,
               secondary.id != selectedSessionID {
                await refreshTerminalScreen(for: secondary)
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
                codaPathWithinRoot(term.cwd, root: repo.rootPath)
                    || repo.branches.contains(where: { codaPathWithinRoot(term.cwd, root: $0.worktreePath) })
            })
            let matchedBranch = matchedRepo?.branches.first(where: { codaPathWithinRoot(term.cwd, root: $0.worktreePath) })
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
            // 1. Always advance the edge-triggered notification tracker baseline so toggling
            // notifications off/on never fires phantom alerts; gate only delivery on user preference.
            let alert = notificationTracker.observe(
                sessionId: s.id,
                machineName: s.hostName,
                workspaceName: s.workspaceName,
                taskTitle: s.taskTitle,
                statusMessage: s.statusMessage,
                state: s.state,
                activeSessionId: selectedSessionID,
                locale: locale
            )
            if settings.notificationsEnabled, let alert {
                CodaLocalNotifier.shared.deliver(alert)
            }

            // 2. Lightweight in-app toast banner for background sessions
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

    public func refreshTerminalScreen(for session: CodaAgentSession, forceSnapshot: Bool = false) async {
        let epoch = self.workspaceEpoch
        guard let client = networkClient,
              let devId = session.deviceId,
              let dev = remoteMachines.first(where: { $0.id == devId }),
              let clientDevId = localIdentity.deviceId else { return }

        let needSnapshot = forceSnapshot
            || surfaceEngines[session.id] == nil
            || (terminalLinesBySession[session.id]?.isEmpty ?? true)

        if needSnapshot && !Task.isCancelled {
            do {
                let revBefore = terminalRenderRevisionBySession[session.id] ?? 0
                let res = try await client.readTerminalScreen(
                    on: dev,
                    clientIdentity: localIdentity,
                    clientDeviceId: clientDevId,
                    terminal: session.terminalAddress,
                    historyLines: 180
                )
                guard !Task.isCancelled, self.workspaceEpoch == epoch else { return }
                // If live PTY chunks already arrived while a non-forced snapshot RPC was in flight,
                // keep the fresher live stream instead of clobbering it with an older snapshot.
                let revAfter = terminalRenderRevisionBySession[session.id] ?? 0
                if forceSnapshot || !attachedSessionIDs.contains(session.id) || revAfter == revBefore {
                    var engine = GhosttyReflowEngine(
                        columns: terminalColumns,
                        rows: terminalRows,
                        maxScrollback: 1500,
                        treatLFAsCRLF: true
                    )
                    engine.feed(Data(res.text.utf8))
                    // Immediately disable LF->CRLF translation after feeding static snapshot text
                    // so subsequent live PTY streams preserve cursor column on bare LF (`\n`).
                    engine.treatLFAsCRLF = false
                    surfaceEngines[session.id] = engine
                    let visiblePlain = engine.plainVisibleText
                    rawScreenTextBySession[session.id] = visiblePlain
                    terminalLinesBySession[session.id] = engine.styledLines
                    terminalRenderRevisionBySession[session.id, default: 0] &+= 1

                    if let idx = sessions.firstIndex(where: { $0.id == session.id }) {
                        sessions[idx].screenPreview = String(visiblePlain.suffix(420))
                        if let st = engine.latestHerdrStatus {
                            sessions[idx].state = st.state
                            if let msg = st.message { sessions[idx].statusMessage = msg }
                            if let title = st.title { sessions[idx].taskTitle = title }
                        } else if let wireStatus = res.status {
                            sessions[idx].state = HerdrLifecycleState.from(rawState: wireStatus.state, kind: wireStatus.kind)
                            if let msg = wireStatus.msg { sessions[idx].statusMessage = msg }
                        }
                    }
                }
            } catch {
                // Keep previously cached screen lines if snapshot RPC hit temporary timeout;
                // still proceed below to attach live PTY stream!
            }
        }

        let isActivelyDisplayed = (session.id == self.selectedSessionID)
            || (self.duoSplitMode == .dualTerminalSplit && session.id == self.secondarySession?.id)
        if !Task.isCancelled, self.workspaceEpoch == epoch, isActivelyDisplayed, !attachedSessionIDs.contains(session.id) {
            let attached = (try? await client.attachTerminalPty(
                on: dev,
                clientIdentity: localIdentity,
                clientDeviceId: clientDevId,
                terminal: session.terminalAddress,
                cols: terminalColumns,
                rows: terminalRows
            )) ?? false
            let stillDisplayed = (session.id == self.selectedSessionID)
                || (self.duoSplitMode == .dualTerminalSplit && session.id == self.secondarySession?.id)
            if attached, !Task.isCancelled, self.workspaceEpoch == epoch, stillDisplayed {
                attachedSessionIDs.insert(session.id)
            }
        }
    }

    // MARK: - Interactive Controls (Selection, Swipe, Quick Bar, Compose, Launch, Diff)

    public func selectSession(_ session: CodaAgentSession) {
        let changedSession = (selectedSessionID != session.id)
        pendingSelectSessionID = nil
        selectedSessionID = session.id
        if let idx = sessions.firstIndex(where: { $0.id == session.id }) {
            if sessions[idx].unreadCompletion {
                sessions[idx].unreadCompletion = false
                readCompletedIDs.insert(session.id)
            }
        }
        if changedSession && formFactor == .iphoneDuoInnerLandscape && duoSplitMode == .studioDiffSplit {
            loadGitDiff(for: session, presentModal: false)
        }
        screenRefreshTask?.cancel()
        let needSnapshot = !attachedSessionIDs.contains(session.id) || surfaceEngines[session.id] == nil
        screenRefreshTask = Task { [weak self] in
            await self?.refreshTerminalScreen(for: session, forceSnapshot: needSnapshot)
        }
    }

    /// Jumps straight to `sessionId` (used when tapping a native OSC 7501 notification banner).
    /// Persists `pendingSelectSessionID` across cold-launch or mid-refresh calls so the tap is never dropped.
    public func selectSessionByID(_ sessionId: String) {
        if let match = sessions.first(where: { $0.id == sessionId }) {
            pendingSelectSessionID = nil
            selectSession(match)
            return
        }
        pendingSelectSessionID = sessionId
        Task { [weak self] in
            await self?.refreshMachinesAndSessions()
        }
    }

    public func selectSecondarySession(_ session: CodaAgentSession) {
        secondarySessionID = session.id
        if let idx = sessions.firstIndex(where: { $0.id == session.id }) {
            if sessions[idx].unreadCompletion {
                sessions[idx].unreadCompletion = false
                readCompletedIDs.insert(session.id)
            }
        }
        let needSnapshot = !attachedSessionIDs.contains(session.id) || surfaceEngines[session.id] == nil
        Task { [weak self] in
            await self?.refreshTerminalScreen(for: session, forceSnapshot: needSnapshot)
        }
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
        let duoExtraRail: CGFloat = safeAreas.trailing >= 64 ? 0 : CodaEdgeSafeAreas.duoSideRailWidth

        let paneW: CGFloat
        let paneH: CGFloat
        switch nextFactor {
        case .iphonePortrait:
            paneW = usableW
            paneH = max(180, usableH - 118)
        case .iphoneDuoCoverPortrait:
            paneW = max(260, usableW - duoExtraRail)
            paneH = max(180, usableH - 96)
        case .iphoneLandscape:
            paneW = max(280, usableW - CodaEdgeSafeAreas.duoSideRailWidth)
            paneH = max(160, usableH - 64)
        case .iphoneDuoCoverLandscape:
            paneW = max(280, usableW - duoExtraRail)
            paneH = max(160, usableH - 64)
        case .iphoneDuoInnerLandscape:
            if duoSplitMode == .expandedCanvas {
                paneW = max(360, usableW - duoExtraRail)
            } else if duoSplitMode == .studioDiffSplit {
                paneW = max(260, floor(size.width * 0.5) - 12 - safeAreas.leading)
            } else {
                paneW = max(260, floor(size.width * 0.5) - 12 - max(safeAreas.trailing, CodaEdgeSafeAreas.duoSideRailWidth))
            }
            paneH = max(200, usableH - 96)
        case .iphoneDuoInnerPortrait:
            let contentW = max(300, usableW - duoExtraRail)
            if foldPosture == .tabletopLaptop {
                paneW = contentW
                paneH = max(160, floor(size.height * 0.5) - 12 - safeAreas.top - 42)
            } else {
                paneW = showSidebar ? max(280, contentW - 216) : contentW
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
            for sid in Set(surfaceEngines.keys).union(rawScreenTextBySession.keys) {
                if var engine = surfaceEngines[sid] {
                    engine.resizeReflow(columns: nextCols, rows: nextRows)
                    surfaceEngines[sid] = engine
                    terminalLinesBySession[sid] = engine.styledLines
                    rawScreenTextBySession[sid] = engine.plainVisibleText
                    terminalRenderRevisionBySession[sid, default: 0] &+= 1
                } else if let raw = rawScreenTextBySession[sid] {
                    var fresh = GhosttyReflowEngine(
                        columns: nextCols,
                        rows: nextRows,
                        maxScrollback: 1500,
                        treatLFAsCRLF: true
                    )
                    fresh.feed(Data(raw.utf8))
                    fresh.treatLFAsCRLF = false
                    surfaceEngines[sid] = fresh
                    terminalLinesBySession[sid] = fresh.styledLines
                    terminalRenderRevisionBySession[sid, default: 0] &+= 1
                }
            }
            if let client = networkClient,
               let clientDevId = localIdentity.deviceId {
                var seenIDs = Set<String>()
                var targets: [CodaAgentSession] = []
                if let primary = selectedSession, seenIDs.insert(primary.id).inserted {
                    targets.append(primary)
                }
                if duoSplitMode == .dualTerminalSplit,
                   let secondary = secondarySession,
                   seenIDs.insert(secondary.id).inserted {
                    targets.append(secondary)
                }
                for session in targets {
                    guard let devId = session.deviceId,
                          let dev = remoteMachines.first(where: { $0.id == devId }) else { continue }
                    let ident = localIdentity
                    Task {
                        try? await client.resizeTerminalPty(
                            on: dev,
                            clientIdentity: ident,
                            clientDeviceId: clientDevId,
                            terminal: session.terminalAddress,
                            cols: nextCols,
                            rows: nextRows
                        )
                    }
                }
            }
        }
    }

    public func recomputeTerminalGridForFontChange() {
        guard lastViewportSize.width > 0 && lastViewportSize.height > 0 else { return }
        updateViewportGeometry(
            size: lastViewportSize,
            safeAreas: edgeSafeAreas,
            isHorizontalRegular: lastIsHorizontalRegular,
            isVerticalRegular: lastIsVerticalRegular
        )
    }

    public func closeTerminalToInbox() {
        let closing = selectedSession
        screenRefreshTask?.cancel()
        screenRefreshTask = nil
        selectedSessionID = nil
        dpadVisible = false
        ctrlActive = false
        altActive = false
        if let closing {
            attachedSessionIDs.remove(closing.id)
            if let client = networkClient,
               let devId = closing.deviceId,
               let dev = remoteMachines.first(where: { $0.id == devId }),
               let clientDevId = localIdentity.deviceId {
                Task {
                    try? await client.detachTerminalPty(
                        on: dev,
                        clientIdentity: localIdentity,
                        clientDeviceId: clientDevId,
                        terminal: closing.terminalAddress
                    )
                }
            }
        }
    }

    public func switchSibling(delta: Int, for targetSession: CodaAgentSession? = nil, isSecondary: Bool = false) {
        guard let current = targetSession ?? (isSecondary ? secondarySession : selectedSession) else { return }
        let siblings = siblingSessions(for: current)
        guard siblings.count > 1,
              let idx = siblings.firstIndex(where: { $0.id == current.id }) else { return }
        let nextIdx = (idx + delta + siblings.count) % siblings.count
        if isSecondary {
            selectSecondarySession(siblings[nextIdx])
        } else {
            selectSession(siblings[nextIdx])
        }
    }

    private func optimisticallyClearNeedsYou(for session: CodaAgentSession) {
        guard let idx = sessions.firstIndex(where: { $0.id == session.id }),
              sessions[idx].state == .needsYou else { return }
        sessions[idx].state = .working
        sessions[idx].statusMessage = nil
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
        optimisticallyClearNeedsYou(for: session)
        let isAttached = attachedSessionIDs.contains(session.id)
        Task {
            try? await client.interruptTerminal(
                on: dev,
                clientIdentity: localIdentity,
                clientDeviceId: clientDevId,
                terminal: session.terminalAddress
            )
            if !isAttached {
                try? await Task.sleep(nanoseconds: 250_000_000)
                await refreshTerminalScreen(for: session, forceSnapshot: true)
            }
        }
    }

    public func killSession(_ session: CodaAgentSession) {
        guard let client = networkClient,
              let devId = session.deviceId,
              let dev = remoteMachines.first(where: { $0.id == devId }),
              let clientDevId = localIdentity.deviceId else { return }
        let sid = session.id
        attachedSessionIDs.remove(sid)
        surfaceEngines.removeValue(forKey: sid)
        terminalLinesBySession.removeValue(forKey: sid)
        rawScreenTextBySession.removeValue(forKey: sid)
        sessions.removeAll { $0.id == sid }
        if selectedSessionID == sid {
            selectedSessionID = nil
        }
        if secondarySessionID == sid {
            secondarySessionID = nil
        }
        Task {
            try? await client.killTerminal(
                on: dev,
                clientIdentity: localIdentity,
                clientDeviceId: clientDevId,
                terminal: session.terminalAddress
            )
            await refreshMachinesAndSessions()
        }
    }

    public func sendQuickKey(_ key: HerdrQuickKey, to targetSession: CodaAgentSession? = nil) {
        guard let session = targetSession ?? selectedSession,
              let client = networkClient,
              let devId = session.deviceId,
              let dev = remoteMachines.first(where: { $0.id == devId }),
              let clientDevId = localIdentity.deviceId else { return }

        switch key {
        case .ctrlC, .enter:
            CodaHaptics.mediumTap()
            optimisticallyClearNeedsYou(for: session)
        case .esc:
            CodaHaptics.lightTap()
            optimisticallyClearNeedsYou(for: session)
        default:
            CodaHaptics.lightTap()
        }

        let useAlt = altActive
        ctrlActive = false
        altActive = false

        var bytes = key.encodedBytes
        if useAlt && !bytes.isEmpty && bytes.first != 0x1B {
            var prefixed = Data([0x1B])
            prefixed.append(bytes)
            bytes = prefixed
        }
        let isAttached = attachedSessionIDs.contains(session.id)

        Task {
            try? await client.sendPtyInput(
                on: dev,
                clientIdentity: localIdentity,
                clientDeviceId: clientDevId,
                terminal: session.terminalAddress,
                data: bytes
            )
            if !isAttached {
                try? await Task.sleep(nanoseconds: 180_000_000)
                await refreshTerminalScreen(for: session, forceSnapshot: true)
            }
        }
    }

    public func sendDirectText(_ text: String, submit: Bool = false, to targetSession: CodaAgentSession? = nil) {
        guard !text.isEmpty,
              let session = targetSession ?? selectedSession,
              let client = networkClient,
              let devId = session.deviceId,
              let dev = remoteMachines.first(where: { $0.id == devId }),
              let clientDevId = localIdentity.deviceId else { return }

        if submit && session.state == .needsYou && (text == "1" || text == "2") {
            CodaHaptics.success()
            optimisticallyClearNeedsYou(for: session)
        } else if submit {
            CodaHaptics.mediumTap()
            optimisticallyClearNeedsYou(for: session)
        } else {
            CodaHaptics.lightTap()
        }

        let useCtrl = ctrlActive
        let useAlt = altActive
        ctrlActive = false
        altActive = false

        let payloadBytes: Data
        if useCtrl || useAlt {
            payloadBytes = GhosttyReflowEngine.encodeModifiedText(text, ctrl: useCtrl, alt: useAlt)
        } else {
            var raw = Data(text.utf8)
            if submit {
                raw.append(0x0D)
            }
            payloadBytes = raw
        }
        let isAttached = attachedSessionIDs.contains(session.id)

        Task {
            try? await client.sendPtyInput(
                on: dev,
                clientIdentity: localIdentity,
                clientDeviceId: clientDevId,
                terminal: session.terminalAddress,
                data: payloadBytes
            )
            if !isAttached {
                try? await Task.sleep(nanoseconds: 220_000_000)
                await refreshTerminalScreen(for: session, forceSnapshot: true)
            }
        }
    }

    public func submitComposePrompt(for targetSession: CodaAgentSession? = nil) {
        let trimmed = composeText.trimmingCharacters(in: .newlines)
        guard !trimmed.isEmpty, let session = targetSession ?? activeSheetSession ?? selectedSession,
              let client = networkClient,
              let devId = session.deviceId,
              let dev = remoteMachines.first(where: { $0.id == devId }),
              let clientDevId = localIdentity.deviceId else { return }

        optimisticallyClearNeedsYou(for: session)
        isSendingAction = true
        let toSend = trimmed
        let isAttached = attachedSessionIDs.contains(session.id)
        composeText = ""
        showComposeSheet = false

        Task {
            defer { self.isSendingAction = false }
            if isAttached {
                var bytes = Data()
                if toSend.contains("\n") {
                    // Use bracketed paste for multiline prompts so TUIs (Claude/Codex/Gemini) keep newlines intact.
                    bytes.append(Data("\u{1B}[200~".utf8))
                    bytes.append(Data(toSend.utf8))
                    bytes.append(Data("\u{1B}[201~".utf8))
                    bytes.append(0x0D)
                } else {
                    bytes.append(Data(toSend.utf8))
                    bytes.append(0x0D)
                }
                try? await client.sendPtyInput(
                    on: dev,
                    clientIdentity: localIdentity,
                    clientDeviceId: clientDevId,
                    terminal: session.terminalAddress,
                    data: bytes
                )
            } else {
                try? await client.sendTerminalText(
                    on: dev,
                    clientIdentity: localIdentity,
                    clientDeviceId: clientDevId,
                    terminal: session.terminalAddress,
                    text: toSend,
                    submit: true
                )
                try? await Task.sleep(nanoseconds: 250_000_000)
                await refreshTerminalScreen(for: session, forceSnapshot: true)
            }
        }
    }

    public func uploadAttachment(fileURL: URL, for targetSession: CodaAgentSession? = nil) {
        guard let session = targetSession ?? activeSheetSession ?? selectedSession,
              let client = networkClient,
              let devId = session.deviceId,
              let dev = remoteMachines.first(where: { $0.id == devId }),
              let clientDevId = localIdentity.deviceId else { return }

        // Keep raw upload under 4 MiB so double-Base64 inside encrypted RelayWireFrame stays below 8 MiB.
        let maxUploadBytes = 4 * 1024 * 1024
        Task {
            let granted = fileURL.startAccessingSecurityScopedResource()
            defer {
                if granted { fileURL.stopAccessingSecurityScopedResource() }
            }
            if let fileSize = (try? fileURL.resourceValues(forKeys: [.fileSizeKey]).fileSize),
               fileSize > maxUploadBytes {
                uploadStatusBanner = tr(
                    "File exceeds 4 MiB upload limit.",
                    "文件超过 4 MiB 上传限制。",
                    locale: locale
                )
                return
            }
            guard let data = try? Data(contentsOf: fileURL) else {
                uploadStatusBanner = tr("Could not read selected file.", "读取文件失败。", locale: locale)
                return
            }
            guard data.count <= maxUploadBytes else {
                uploadStatusBanner = tr(
                    "File exceeds 4 MiB upload limit.",
                    "文件超过 4 MiB 上传限制。",
                    locale: locale
                )
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

    public func toggleVoiceDictation() async {
        voiceError = nil
        if isTranscribingVoice || isStartingVoice { return }

        if isRecordingVoice {
            voiceGeneration &+= 1
            let gen = voiceGeneration
            let epoch = workspaceEpoch
            isRecordingVoice = false
            isTranscribingVoice = true
            let task = Task { @MainActor [weak self] in
                guard let self else { return }
                defer {
                    if self.voiceGeneration == gen {
                        self.isTranscribingVoice = false
                        self.voiceTask = nil
                    }
                }
                do {
                    let transcript = try await self.voiceRecorder.stopAndTranscribe(config: self.settings.voiceConfig)
                    guard !Task.isCancelled, self.voiceGeneration == gen, self.workspaceEpoch == epoch else { return }
                    if !transcript.isEmpty {
                        if self.composeText.isEmpty || self.composeText.hasSuffix(" ") || self.composeText.hasSuffix("\n") {
                            self.composeText += transcript
                        } else {
                            self.composeText += " \(transcript)"
                        }
                    }
                } catch {
                    guard !Task.isCancelled, self.voiceGeneration == gen, self.workspaceEpoch == epoch else { return }
                    self.voiceError = error.localizedDescription
                }
            }
            voiceTask = task
            await task.value
            return
        }

        voiceGeneration &+= 1
        let gen = voiceGeneration
        let epoch = workspaceEpoch
        isStartingVoice = true
        let task = Task { @MainActor [weak self] in
            guard let self else { return }
            defer {
                if self.voiceGeneration == gen {
                    self.isStartingVoice = false
                    self.voiceTask = nil
                }
            }
            do {
                _ = try CodaVoiceRecorder.validatedTranscriptionURL(from: self.settings.voiceConfig.baseURL)
                try await self.voiceRecorder.startRecording()
                if Task.isCancelled || self.voiceGeneration != gen || self.workspaceEpoch != epoch {
                    if self.voiceGeneration == gen {
                        self.voiceRecorder.cancelRecording()
                        self.isRecordingVoice = false
                    }
                    return
                }
                self.isRecordingVoice = true
            } catch {
                guard self.voiceGeneration == gen else { return }
                self.voiceRecorder.cancelRecording()
                self.isRecordingVoice = false
                guard !Task.isCancelled, self.workspaceEpoch == epoch else { return }
                self.voiceError = error.localizedDescription
            }
        }
        voiceTask = task
        await task.value
    }

    public func cancelVoiceDictation() {
        voiceGeneration &+= 1
        voiceTask?.cancel()
        voiceTask = nil
        voiceRecorder.cancelRecording()
        isStartingVoice = false
        isRecordingVoice = false
        isTranscribingVoice = false
    }

    public func saveVoiceConfig() async {
        do {
            _ = try CodaVoiceRecorder.validatedTranscriptionURL(from: settings.voiceConfig.baseURL)
        } catch {
            voiceConfigSyncMessage = error.localizedDescription
            return
        }
        settings.save()
        voiceConfigSyncMessage = tr("Saved to secure storage.", "已保存至安全存储。", locale: locale)
        guard isSignedIn, let client = await preparedClient(), let clientDevId = localIdentity.deviceId else { return }
        let onlineHosts = remoteMachines.filter(\.isOnline)
        guard !onlineHosts.isEmpty else { return }
        var syncedCount = 0
        for host in onlineHosts {
            if let synced = try? await client.setVoiceConfig(
                on: host,
                clientIdentity: localIdentity,
                clientDeviceId: clientDevId,
                config: settings.voiceConfig
            ) {
                // Keep locally saved API key if daemon returned redacted empty string.
                var merged = synced
                if merged.apiKey.isEmpty && !settings.voiceConfig.apiKey.isEmpty {
                    merged.apiKey = settings.voiceConfig.apiKey
                }
                settings.voiceConfig = merged
                settings.save()
                syncedCount += 1
            }
        }
        if syncedCount > 0 {
            voiceConfigSyncMessage = tr(
                "Saved & synced to \(syncedCount) host(s).",
                "已保存并同步到 \(syncedCount) 台主机。",
                locale: locale
            )
        } else {
            voiceConfigSyncMessage = tr(
                "Saved locally (host offline).",
                "已在本地安全保存（主机离线）。",
                locale: locale
            )
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

    public func loadGitDiff(for targetSession: CodaAgentSession?, presentModal: Bool = true) {
        guard let session = targetSession ?? selectedSession,
              let client = networkClient,
              let devId = session.deviceId,
              let dev = remoteMachines.first(where: { $0.id == devId }),
              let clientDevId = localIdentity.deviceId else { return }
        gitDiffGeneration &+= 1
        let diffGen = gitDiffGeneration
        let epoch = workspaceEpoch
        gitDiffResult = nil
        gitDiffLoading = true
        if presentModal {
            activeSheetSessionID = session.id
            showGitDiffSheet = true
        }
        Task { [weak self] in
            guard let self else { return }
            defer {
                if self.gitDiffGeneration == diffGen && self.workspaceEpoch == epoch {
                    self.gitDiffLoading = false
                }
            }
            let fetched = try? await client.fetchGitDiff(
                on: dev,
                clientIdentity: self.localIdentity,
                clientDeviceId: clientDevId,
                cwd: session.cwd
            )
            if self.gitDiffGeneration == diffGen && self.workspaceEpoch == epoch {
                self.gitDiffResult = fetched
            }
        }
    }

    public func loadGitDiffForSelected(presentModal: Bool = true) {
        loadGitDiff(for: selectedSession, presentModal: presentModal)
    }
}

private extension String {
    var nilIfEmpty: String? {
        let t = trimmingCharacters(in: .whitespacesAndNewlines)
        return t.isEmpty ? nil : t
    }
}
