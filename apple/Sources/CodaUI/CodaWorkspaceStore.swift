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
    @Published public var settingsInitialAuthMode: Int? = nil
    @Published public var showComposeSheet: Bool = false {
        didSet { if !showComposeSheet { reconcileActiveSheetSessionOnDismissIfNeeded() } }
    }
    @Published public var showPaneMinimapSheet: Bool = false {
        didSet { if !showPaneMinimapSheet { reconcileActiveSheetSessionOnDismissIfNeeded() } }
    }
    @Published public var showGitDiffSheet: Bool = false {
        didSet { if !showGitDiffSheet { reconcileActiveSheetSessionOnDismissIfNeeded() } }
    }
    @Published public var showRemoteFilesSheet: Bool = false {
        didSet { if !showRemoteFilesSheet { reconcileActiveSheetSessionOnDismissIfNeeded() } }
    }
    @Published public var inspectorSheetMode: CodaInspectorSheetMode = .files
    @Published public private(set) var inspectorSessionId: String? = nil
    @Published public var remoteFileRootPath: String = ""
    @Published public var remoteFileSubPath: String = ""
    @Published public var remoteFileEntries: [CodaRemoteFileEntry] = []
    @Published public var remoteFileSelectedRel: String? = nil
    @Published public var remoteFilePreviewText: String = ""
    @Published public var remoteFileTruncated: Bool = false
    @Published public var remoteFileLoading: Bool = false
    @Published public var remoteFileError: String? = nil
    @Published public var portPreviewResult: CodaPortPreviewResult? = nil
    @Published public var portPreviewLoading: Bool = false
    @Published public var portPreviewError: String? = nil
    @Published public var searchQuery: String = ""
    @Published public var expandedWorkspaces: Set<String> = []

    @Published public var sessions: [CodaAgentSession] = []
    @Published public var remoteMachines: [CodaRemoteDevice] = []
    @Published public var workspacesByDevice: [UUID: [CodaRepoWorkspaceSummary]] = [:]
    @Published public var hostHealthByDevice: [UUID: CodaHostHealthSnapshot] = [:]
    @Published public private(set) var daemonVoiceConfigsByDevice: [UUID: CodaVoiceModelConfig] = [:]
    @Published public var machineRouteModes: [UUID: SmartPathRouteMode] = [:]
    @Published public var terminalLinesBySession: [String: [GhosttyStyledLine]] = [:]
    @Published public var rawScreenTextBySession: [String: String] = [:]
    @Published public var terminalRenderRevisionBySession: [String: UInt64] = [:]
    @Published public var activeSheetSessionID: String? = nil
    @Published public var activeSheetIsSecondary: Bool = false
    @Published public var pendingOpenSettingsAfterCompose: Bool = false
    @Published public var gitDiffResult: CodaGitDiffResult? = nil
    @Published public var gitDiffLoading: Bool = false
    @Published public var gitDiffError: String? = nil
    @Published public var gitDiffSelectedFile: String? = nil
    @Published public var notificationSettings: CodaNotificationWebhookSettings = CodaNotificationWebhookSettings()
    @Published public var isSavingNotificationSettings: Bool = false
    @Published public var isTestingNotificationSettings: Bool = false
    @Published public var notificationStatusMessage: String? = nil
    @Published public var notificationErrorMessage: String? = nil
    @Published public var isPruningStaleDevices: Bool = false
    @Published public var otaUpgradingDeviceIDs: Set<UUID> = []
    @Published public var showBootstrapWorkspaceSheet: Bool = false
    @Published public var bootstrapTargetDeviceID: UUID? = nil
    @Published public var bootstrapDefaultParentDir: String = "~/workspace"
    @Published public var isBootstrappingWorkspace: Bool = false
    @Published public var bootstrapErrorMessage: String? = nil
    @Published public var bootstrapStatusMessage: String? = nil

    @Published public var ctrlActive: Bool = false
    @Published public var altActive: Bool = false
    @Published public var dpadVisible: Bool = false
    @Published public var composeText: String = ""
    @Published public var composeTextBySession: [String: String] = [:]
    @Published public private(set) var uploadingAttachmentCount: Int = 0
    @Published public var uploadStatusBanner: String? = nil
    @Published public var uploadBannerIsError: Bool = false
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
    @Published public private(set) var voiceDictationSessionID: String? = nil
    @Published public private(set) var voiceRecordingSeconds: Int = 0
    @Published public private(set) var voiceAudioLevel: Float = 0
    @Published public private(set) var voicePartialTranscript: String = ""
    @Published public var voiceError: String? = nil
    @Published public var accountStatusMessage: String? = nil
    @Published public var currentUser: CodaAuthUserProfile? = nil
    @Published public var promptSnippets: [CodaPromptSnippet] = CodaPromptSnippet.defaultStarterSnippets
    @Published public var isSavingSnippets: Bool = false
    @Published public var adminInvites: [CodaInviteCodeItem] = []
    @Published public var adminUsers: [CodaAdminUserSummary] = []
    @Published public var isLoadingAdminData: Bool = false
    @Published public var isResettingPassword: Bool = false
    @Published public var adminStatusMessage: String? = nil
    @Published public var adminErrorMessage: String? = nil

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
    private var voiceMeterTask: Task<Void, Never>?
    private var pendingVoiceTranscriptHandler: ((String) -> Void)? = nil
    private var isStartingVoice: Bool = false
    private var workspaceEpoch: UInt64 = 0
    private var voiceGeneration: UInt64 = 0
    private var gitDiffGeneration: UInt64 = 0
    private var remoteFileRequestGeneration: UInt64 = 0
    private var portPreviewGeneration: UInt64 = 0
    private var snippetsMutationGeneration: UInt64 = 0
    private var lastAccountMetaSyncAt: Date? = nil
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
    private var lastHardwareScreenSize: CGSize? = nil
    private var lastIsPadOrMacHardware: Bool = false
    private var lastIsHorizontalRegular: Bool? = nil
    private var lastIsVerticalRegular: Bool? = nil

    public var isUploadingAttachment: Bool {
        uploadingAttachmentCount > 0
    }

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
        self.formFactor = .ipadOrDesktop
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

        if !loadedSettings.sessionToken.isEmpty {
            self.promptSnippets = Self.loadCachedSnippets(
                serverBaseURL: loadedSettings.serverBaseURLString,
                ownerHandle: loadedSettings.ownerHandle
            )
            self.currentUser = Self.loadCachedUserProfile(
                serverBaseURL: loadedSettings.serverBaseURLString,
                ownerHandle: loadedSettings.ownerHandle
            )
        }

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
        remoteFileRequestGeneration &+= 1
        portPreviewGeneration &+= 1
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
        self.hostHealthByDevice.removeAll()
        self.daemonVoiceConfigsByDevice.removeAll()
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
        self.inspectorSessionId = nil
        self.remoteFileEntries = []
        self.remoteFileSubPath = ""
        self.remoteFileSelectedRel = nil
        self.remoteFilePreviewText = ""
        self.remoteFileTruncated = false
        self.remoteFileLoading = false
        self.remoteFileError = nil
        self.portPreviewResult = nil
        self.portPreviewLoading = false
        self.portPreviewError = nil
        self.gitDiffResult = nil
        self.gitDiffLoading = false
        self.gitDiffError = nil
        self.gitDiffSelectedFile = nil
        self.notificationStatusMessage = nil
        self.notificationErrorMessage = nil
        self.bootstrapErrorMessage = nil
        self.bootstrapStatusMessage = nil
        self.showBootstrapWorkspaceSheet = false
        self.otaUpgradingDeviceIDs.removeAll()
        self.showComposeSheet = false
        self.showPaneMinimapSheet = false
        self.showGitDiffSheet = false
        self.showRemoteFilesSheet = false
        self.composeText = ""
        self.composeTextBySession.removeAll()
        self.uploadingAttachmentCount = 0
        self.activeToast = nil
        self.uploadStatusBanner = nil
        self.uploadBannerIsError = false
        self.voiceConfigSyncMessage = nil
        self.lastAccountMetaSyncAt = nil
        self.snippetsMutationGeneration &+= 1
        self.adminStatusMessage = nil
        self.adminErrorMessage = nil
        if originChanged {
            self.currentUser = nil
            self.adminInvites = []
            self.adminUsers = []
            self.promptSnippets = CodaPromptSnippet.defaultStarterSnippets
        }
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

    public var hasActiveSessionSheet: Bool {
        showComposeSheet || showPaneMinimapSheet || showGitDiffSheet || showRemoteFilesSheet
    }

    public func reconcileActiveSheetSessionOnDismissIfNeeded() {
        let reconciled = CodaSheetSessionStateHelper.reconciledActiveSheetSessionID(
            current: activeSheetSessionID,
            selectedSessionID: selectedSessionID,
            showComposeSheet: showComposeSheet,
            showPaneMinimapSheet: showPaneMinimapSheet,
            showGitDiffSheet: showGitDiffSheet,
            showRemoteFilesSheet: showRemoteFilesSheet
        )
        guard reconciled == nil else { return }
        if activeSheetSessionID != nil {
            activeSheetSessionID = nil
        }
        if activeSheetIsSecondary {
            activeSheetIsSecondary = false
        }
        let targetDraft = selectedSessionID.flatMap { composeTextBySession[$0] } ?? ""
        if composeText != targetDraft {
            composeText = targetDraft
        }
    }

    private func preparedClientOrThrow() async throws -> CodaNetworkClient {
        if let client = await preparedClient() {
            return client
        }
        throw CodaNetworkError.remoteOperationFailed(
            authError ?? tr(
                "Control plane client is not ready.",
                "控制面网络客户端尚未就绪。",
                locale: locale
            )
        )
    }

    private func presentActionErrorToast(workspaceName: String, sessionId: String = "", message: String) {
        let banner = CodaToastBanner(
            sessionId: sessionId,
            workspaceName: workspaceName,
            title: message,
            state: .error
        )
        activeToast = banner
        Task { [weak self] in
            try? await Task.sleep(nanoseconds: 5_000_000_000)
            if self?.activeToast?.id == banner.id {
                self?.activeToast = nil
            }
        }
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
        if let prevSid = activeSheetSessionID ?? selectedSessionID {
            composeTextBySession[prevSid] = composeText
        }
        activeSheetSessionID = session.id
        activeSheetIsSecondary = isSecondary
        composeText = composeTextBySession[session.id] ?? ""
        showComposeSheet = true
    }

    public func composeDraft(for sessionID: String) -> String {
        if let saved = composeTextBySession[sessionID] {
            return saved
        }
        if (activeSheetSessionID ?? selectedSessionID) == sessionID {
            return composeText
        }
        return ""
    }

    public func setComposeDraft(_ text: String, for sessionID: String) {
        composeTextBySession[sessionID] = text
        if (activeSheetSessionID ?? selectedSessionID) == sessionID {
            composeText = text
        }
    }

    public func appendComposeDraft(_ segment: String, for sessionID: String, padTrailingSpace: Bool = false) {
        guard !segment.isEmpty else { return }
        var cur = composeDraft(for: sessionID)
        if cur.isEmpty || cur.hasSuffix(" ") || cur.hasSuffix("\n") {
            cur += segment + (padTrailingSpace ? " " : "")
        } else {
            cur += " " + segment + (padTrailingSpace ? " " : "")
        }
        setComposeDraft(cur, for: sessionID)
    }

    public func isSessionInAltScreen(_ sessionID: String) -> Bool {
        surfaceEngines[sessionID]?.inAltScreen == true
    }

    public func matchedRepoRoot(for session: CodaAgentSession) -> String? {
        if inspectorSessionId == session.id, let root = gitDiffResult?.repoRoot?.nilIfEmpty {
            return root
        }
        if let devId = session.deviceId, let repos = workspacesByDevice[devId] {
            if let matched = repos.first(where: { repo in
                codaPathWithinRoot(session.cwd, root: repo.rootPath)
                    || repo.branches.contains(where: { codaPathWithinRoot(session.cwd, root: $0.worktreePath) })
            }) {
                if let branchWt = matched.branches.first(where: { codaPathWithinRoot(session.cwd, root: $0.worktreePath) })?.worktreePath.nilIfEmpty {
                    return branchWt
                }
                return matched.rootPath.nilIfEmpty
            }
        }
        return nil
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
                uploadBannerIsError = false
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
                        self.uploadBannerIsError = false
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
                        self.uploadBannerIsError = true
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

        if event.op == "peer_offline" {
            let epoch = self.workspaceEpoch
            let prefixWithSlash = "\(hostPrefix)/"
            attachedSessionIDs = attachedSessionIDs.filter { !$0.hasPrefix(prefixWithSlash) }
            Task { [weak self] in
                guard let self, self.workspaceEpoch == epoch else { return }
                await self.refreshMachinesAndSessions()
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
            let isDisplayed = (sid == selectedSessionID)
                || (duoSplitMode == .dualTerminalSplit && sid == secondarySession?.id)
            guard attachedSessionIDs.contains(sid) || isDisplayed else {
                return
            }
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
            if let profile = try? await client.fetchCurrentUserProfile() {
                currentUser = profile
                settings.ownerHandle = profile.handle
                Self.saveCachedUserProfile(
                    profile,
                    serverBaseURL: settings.serverBaseURLString,
                    ownerHandle: profile.handle
                )
            } else {
                settings.ownerHandle = emailOrHandle
            }
            promptSnippets = Self.loadCachedSnippets(
                serverBaseURL: settings.serverBaseURLString,
                ownerHandle: settings.ownerHandle
            )
            lastAccountMetaSyncAt = nil
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
        if let rawInvite = inviteCode,
           let origin = extractServerOriginFromAuthLink(rawInvite),
           origin.lowercased() != normalizedServerURLString().lowercased() {
            updateServerURL(origin)
        }
        guard let client = await preparedClient() else { return }
        do {
            let cleanEmail = email.trimmingCharacters(in: .whitespacesAndNewlines)
            let cleanHandle = handle.trimmingCharacters(in: .whitespacesAndNewlines)
            let cleanInvite = inviteCode.map(extractInviteCodeFromInput)
            let tok = try await client.register(
                email: cleanEmail,
                handle: cleanHandle,
                displayName: cleanHandle.isEmpty ? cleanEmail : cleanHandle,
                password: password,
                inviteCode: (cleanInvite?.isEmpty == false) ? cleanInvite : nil
            )
            settings.sessionToken = tok
            if let profile = try? await client.fetchCurrentUserProfile() {
                currentUser = profile
                settings.ownerHandle = profile.handle
                Self.saveCachedUserProfile(
                    profile,
                    serverBaseURL: settings.serverBaseURLString,
                    ownerHandle: profile.handle
                )
            } else {
                settings.ownerHandle = cleanHandle
            }
            promptSnippets = Self.loadCachedSnippets(
                serverBaseURL: settings.serverBaseURLString,
                ownerHandle: settings.ownerHandle
            )
            lastAccountMetaSyncAt = nil
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
            otaUpgradingDeviceIDs.remove(device.id)
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
        self.formFactor = .ipadOrDesktop
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
        guard isSignedIn,
              device.isOnline,
              !otaUpgradingDeviceIDs.contains(device.id),
              let client = await preparedClient() else { return }
        let epoch = self.workspaceEpoch
        otaUpgradingDeviceIDs.insert(device.id)
        defer {
            self.otaUpgradingDeviceIDs.remove(device.id)
        }
        if localIdentity.deviceId == nil {
            await ensureClientDeviceRegistered(using: client)
            guard self.workspaceEpoch == epoch else { return }
        }
        accountStatusMessage = tr(
            "Upgrading coda-daemon on \(device.name) (zero-downtime OTA)…",
            "正在为 \(device.name) 无感升级 coda-daemon…",
            locale: locale
        )
        authError = nil
        do {
            _ = try? await client.connectBestAvailable(
                sessionToken: settings.sessionToken,
                targetDevice: device,
                clientIdentity: localIdentity
            )
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
        remoteFileRequestGeneration &+= 1
        portPreviewGeneration &+= 1
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
        daemonVoiceConfigsByDevice = [:]
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
        inspectorSessionId = nil
        gitDiffResult = nil
        gitDiffLoading = false
        gitDiffError = nil
        gitDiffSelectedFile = nil
        notificationSettings = CodaNotificationWebhookSettings()
        notificationStatusMessage = nil
        notificationErrorMessage = nil
        bootstrapErrorMessage = nil
        bootstrapStatusMessage = nil
        showBootstrapWorkspaceSheet = false
        otaUpgradingDeviceIDs.removeAll()
        showComposeSheet = false
        showPaneMinimapSheet = false
        showGitDiffSheet = false
        showRemoteFilesSheet = false
        remoteFileEntries = []
        remoteFileSubPath = ""
        remoteFileSelectedRel = nil
        remoteFilePreviewText = ""
        remoteFileTruncated = false
        remoteFileLoading = false
        remoteFileError = nil
        portPreviewResult = nil
        portPreviewLoading = false
        portPreviewError = nil
        composeText = ""
        composeTextBySession = [:]
        uploadingAttachmentCount = 0
        activeToast = nil
        voiceConfigSyncMessage = nil
        uploadStatusBanner = nil
        uploadBannerIsError = false
        currentUser = nil
        adminInvites = []
        adminUsers = []
        adminStatusMessage = nil
        adminErrorMessage = nil
        lastAccountMetaSyncAt = nil
        snippetsMutationGeneration &+= 1
        promptSnippets = CodaPromptSnippet.defaultStarterSnippets
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
            let needAccountMeta = forceWorkspaces
                || currentUser == nil
                || (lastAccountMetaSyncAt.map { Date().timeIntervalSince($0) >= 45 } ?? true)
            if needAccountMeta {
                await syncAccountProfileAndSnippets(using: client, epoch: epoch)
                guard self.workspaceEpoch == epoch, isSignedIn else { return }
            }
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
                        if let vc = wsRes?.voiceConfig {
                            daemonVoiceConfigsByDevice[host.id] = vc
                            if settings.voiceConfig.apiKey.isEmpty {
                                if vc.hasLocalPlaintextApiKey {
                                    settings.voiceConfig = vc
                                    settings.save()
                                } else if vc.hasMaskedDaemonKey {
                                    // Preserve daemon's non-secret provider/baseURL/model metadata without
                                    // writing "••••••••" into local plaintext key storage.
                                    settings.voiceConfig.provider = vc.provider
                                    settings.voiceConfig.baseURL = vc.baseURL
                                    settings.voiceConfig.model = vc.model
                                    settings.save()
                                }
                            }
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
            let activeSessionIDs = Set(self.sessions.map(\.id))
            // Prune exited sessions from VT engines, screen buffers, and draft caches so memory stays bounded.
            self.attachedSessionIDs = self.attachedSessionIDs.intersection(activeSessionIDs)
            for key in Array(self.surfaceEngines.keys) where !activeSessionIDs.contains(key) {
                self.surfaceEngines.removeValue(forKey: key)
            }
            for key in Array(self.terminalLinesBySession.keys) where !activeSessionIDs.contains(key) {
                self.terminalLinesBySession.removeValue(forKey: key)
            }
            for key in Array(self.rawScreenTextBySession.keys) where !activeSessionIDs.contains(key) {
                self.rawScreenTextBySession.removeValue(forKey: key)
            }
            for key in Array(self.terminalRenderRevisionBySession.keys) where !activeSessionIDs.contains(key) {
                self.terminalRenderRevisionBySession.removeValue(forKey: key)
            }
            for key in Array(self.previousStates.keys) where !activeSessionIDs.contains(key) {
                self.previousStates.removeValue(forKey: key)
            }
            self.readCompletedIDs = self.readCompletedIDs.intersection(activeSessionIDs)
            for key in Array(self.composeTextBySession.keys) where !activeSessionIDs.contains(key) {
                self.composeTextBySession.removeValue(forKey: key)
            }

            let validHostIDs = Set(hosts.map(\.id))
            self.workspacesByDevice = self.workspacesByDevice.filter { validHostIDs.contains($0.key) }
            self.hostHealthByDevice = self.hostHealthByDevice.filter { validHostIDs.contains($0.key) }
            self.daemonVoiceConfigsByDevice = self.daemonVoiceConfigsByDevice.filter { validHostIDs.contains($0.key) }
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
            if let sheetSid = self.activeSheetSessionID, !self.sessions.contains(where: { $0.id == sheetSid }) {
                self.activeSheetSessionID = nil
                self.activeSheetIsSecondary = false
                self.showComposeSheet = false
                self.showPaneMinimapSheet = false
                self.showGitDiffSheet = false
                self.showRemoteFilesSheet = false
            } else if !self.hasActiveSessionSheet && self.activeSheetSessionID != nil {
                self.activeSheetSessionID = nil
                self.activeSheetIsSecondary = false
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
                    let priorEngine = surfaceEngines[session.id]
                    var engine = GhosttyReflowEngine(
                        columns: terminalColumns,
                        rows: terminalRows,
                        maxScrollback: 1500,
                        treatLFAsCRLF: true
                    )
                    engine.feed(Data(res.text.utf8))
                    // Immediately disable LF->CRLF translation after feeding static snapshot text
                    // so subsequent live PTY streams preserve cursor column on bare LF (`\n`),
                    // and preserve prior private modes (`?2004h`/`?2004l`, mouse reporting) if
                    // the plain-text snapshot itself didn't explicitly emit them.
                    engine.treatLFAsCRLF = false
                    if let priorEngine {
                        engine.restoreModeFlagsIfUnset(from: priorEngine)
                    }
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

    private func detachIfNoLongerDisplayed(_ candidate: CodaAgentSession?) {
        guard let closing = candidate else { return }
        let stillDisplayed = (closing.id == selectedSessionID)
            || (duoSplitMode == .dualTerminalSplit && closing.id == secondarySession?.id)
        guard !stillDisplayed else { return }
        guard attachedSessionIDs.remove(closing.id) != nil else { return }
        guard let client = networkClient,
              let devId = closing.deviceId,
              let dev = remoteMachines.first(where: { $0.id == devId }),
              let clientDevId = localIdentity.deviceId else { return }
        let ident = localIdentity
        Task {
            try? await client.detachTerminalPty(
                on: dev,
                clientIdentity: ident,
                clientDeviceId: clientDevId,
                terminal: closing.terminalAddress
            )
        }
    }

    private func resetInspectorStateIfSessionChanged(to session: CodaAgentSession) {
        guard inspectorSessionId != session.id else { return }
        inspectorSessionId = session.id
        gitDiffGeneration &+= 1
        remoteFileRequestGeneration &+= 1
        portPreviewGeneration &+= 1
        let root = session.cwd.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? "." : session.cwd
        remoteFileRootPath = root
        remoteFileSubPath = ""
        remoteFileEntries = []
        remoteFileSelectedRel = nil
        remoteFilePreviewText = ""
        remoteFileTruncated = false
        remoteFileLoading = false
        remoteFileError = nil
        gitDiffResult = nil
        gitDiffLoading = false
        gitDiffError = nil
        gitDiffSelectedFile = nil
        portPreviewResult = nil
        portPreviewLoading = false
        portPreviewError = nil
    }

    public func selectSession(_ session: CodaAgentSession) {
        let previousSelected = selectedSession
        let changedSession = (selectedSessionID != session.id)
        if changedSession {
            if let prevId = selectedSessionID, !showComposeSheet {
                composeTextBySession[prevId] = composeText
            }
            cancelVoiceDictation()
            ctrlActive = false
            altActive = false
            resetInspectorStateIfSessionChanged(to: session)
        }
        pendingSelectSessionID = nil
        selectedSessionID = session.id
        let keepSecondarySheet = activeSheetIsSecondary && duoSplitMode == .dualTerminalSplit
        let nextSheetSid = CodaSheetSessionStateHelper.reconciledActiveSheetSessionID(
            current: activeSheetSessionID,
            selectedSessionID: session.id,
            showComposeSheet: showComposeSheet,
            showPaneMinimapSheet: showPaneMinimapSheet,
            showGitDiffSheet: showGitDiffSheet,
            showRemoteFilesSheet: showRemoteFilesSheet,
            didSwitchSelectedSession: changedSession,
            activeSheetIsSecondary: keepSecondarySheet
        )
        if nextSheetSid == nil {
            activeSheetSessionID = nil
            activeSheetIsSecondary = false
        } else if nextSheetSid != activeSheetSessionID {
            if showComposeSheet, let prevSheetSid = activeSheetSessionID {
                composeTextBySession[prevSheetSid] = composeText
            }
            activeSheetSessionID = nextSheetSid
            if !keepSecondarySheet {
                activeSheetIsSecondary = false
            }
        }
        if !showComposeSheet || activeSheetSessionID == session.id {
            composeText = composeTextBySession[session.id] ?? ""
        }
        if changedSession {
            detachIfNoLongerDisplayed(previousSelected)
        }
        if let idx = sessions.firstIndex(where: { $0.id == session.id }) {
            if sessions[idx].unreadCompletion {
                sessions[idx].unreadCompletion = false
                readCompletedIDs.insert(session.id)
            }
        }
        if changedSession {
            let needsInlineDiff = (formFactor == .iphoneDuoInnerLandscape && duoSplitMode == .studioDiffSplit)
                || showGitDiffSheet
                || (showRemoteFilesSheet && inspectorSheetMode == .changes)
            if needsInlineDiff {
                loadGitDiff(for: session, presentModal: false)
            }
            if showRemoteFilesSheet && inspectorSheetMode == .files {
                browseRemoteSubPath("", for: session)
            }
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
        let previousSecondary = secondarySession
        let changedSecondary = (secondarySessionID != session.id)
        if changedSecondary, let prevSecId = previousSecondary?.id, voiceDictationSessionID == prevSecId {
            cancelVoiceDictation()
        }
        secondarySessionID = session.id
        if !hasActiveSessionSheet {
            activeSheetSessionID = nil
            activeSheetIsSecondary = false
        } else if activeSheetIsSecondary && changedSecondary {
            if showComposeSheet, let prevSheetSid = activeSheetSessionID, prevSheetSid != session.id {
                composeTextBySession[prevSheetSid] = composeText
                composeText = composeTextBySession[session.id] ?? ""
            }
            activeSheetSessionID = session.id
        }
        if changedSecondary {
            detachIfNoLongerDisplayed(previousSecondary)
        }
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
        let prevSecondary = secondarySession
        let wasDual = (duoSplitMode == .dualTerminalSplit)
        duoSplitMode = mode
        if wasDual && mode != .dualTerminalSplit {
            detachIfNoLongerDisplayed(prevSecondary)
        }
        if mode == .dualTerminalSplit && (secondarySessionID == nil || secondarySessionID == selectedSessionID) {
            secondarySessionID = filteredSessions.first(where: { $0.id != selectedSessionID })?.id ?? selectedSessionID
        }
        if mode == .studioDiffSplit, let selected = selectedSession {
            if inspectorSessionId != selected.id || gitDiffResult == nil {
                loadGitDiff(for: selected, presentModal: false)
            }
        }
        if lastViewportSize.width > 0 && lastViewportSize.height > 0 {
            updateViewportGeometry(
                size: lastViewportSize,
                safeAreas: edgeSafeAreas,
                isHorizontalRegular: lastIsHorizontalRegular,
                isVerticalRegular: lastIsVerticalRegular,
                hardwareScreenSize: lastHardwareScreenSize,
                isPadOrMacHardware: lastIsPadOrMacHardware
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
                isVerticalRegular: lastIsVerticalRegular,
                hardwareScreenSize: lastHardwareScreenSize,
                isPadOrMacHardware: lastIsPadOrMacHardware
            )
        }
    }

    public func updateViewportGeometry(
        size: CGSize,
        safeAreas: CodaEdgeSafeAreas = .zero,
        isHorizontalRegular: Bool? = nil,
        isVerticalRegular: Bool? = nil,
        hardwareScreenSize: CGSize? = nil,
        isPadOrMacHardware: Bool = false
    ) {
        guard size.width > 0 && size.height > 0 else { return }
        lastViewportSize = size
        lastIsHorizontalRegular = isHorizontalRegular
        lastIsVerticalRegular = isVerticalRegular
        lastHardwareScreenSize = hardwareScreenSize
        lastIsPadOrMacHardware = isPadOrMacHardware
        edgeSafeAreas = safeAreas

        let duoRailHint: Bool? = (safeAreas.trailing >= 64 && safeAreas.isHorizontallyAsymmetric) ? true : nil
        let nextFactor = CodaFormFactor.detect(
            width: size.width,
            height: size.height,
            isHorizontalRegular: isHorizontalRegular,
            isVerticalRegular: isVerticalRegular,
            hardwareScreenSize: hardwareScreenSize,
            isPadOrMacHardware: isPadOrMacHardware,
            hasAsymmetricDuoRail: duoRailHint
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
            isVerticalRegular: lastIsVerticalRegular,
            hardwareScreenSize: lastHardwareScreenSize,
            isPadOrMacHardware: lastIsPadOrMacHardware
        )
    }

    public func closeTerminalToInbox() {
        cancelVoiceDictation()
        let closing = selectedSession
        if let prevId = (activeSheetSessionID ?? closing?.id) {
            composeTextBySession[prevId] = composeText
        }
        screenRefreshTask?.cancel()
        screenRefreshTask = nil
        showComposeSheet = false
        showPaneMinimapSheet = false
        showGitDiffSheet = false
        showRemoteFilesSheet = false
        activeSheetSessionID = nil
        activeSheetIsSecondary = false
        selectedSessionID = nil
        dpadVisible = false
        ctrlActive = false
        altActive = false
        detachIfNoLongerDisplayed(closing)
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
        if voiceDictationSessionID == nil || voiceDictationSessionID == sid {
            cancelVoiceDictation()
        }
        attachedSessionIDs.remove(sid)
        surfaceEngines.removeValue(forKey: sid)
        terminalLinesBySession.removeValue(forKey: sid)
        rawScreenTextBySession.removeValue(forKey: sid)
        terminalRenderRevisionBySession.removeValue(forKey: sid)
        composeTextBySession.removeValue(forKey: sid)
        sessions.removeAll { $0.id == sid }
        if selectedSessionID == sid {
            selectedSessionID = nil
            ctrlActive = false
            altActive = false
        }
        if secondarySessionID == sid {
            secondarySessionID = nil
        }
        if activeSheetSessionID == sid {
            activeSheetSessionID = nil
            activeSheetIsSecondary = false
            showComposeSheet = false
            showPaneMinimapSheet = false
            showRemoteFilesSheet = false
            showGitDiffSheet = false
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

        if submit && session.state == .needsYou && (text == "y" || text == "Y" || text == "1" || text == "2" || text == "3") {
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

    public func sendTerminalScroll(up: Bool, lines: Int = 3, to targetSession: CodaAgentSession? = nil) {
        guard let session = targetSession ?? selectedSession,
              let client = networkClient,
              let devId = session.deviceId,
              let dev = remoteMachines.first(where: { $0.id == devId }),
              let clientDevId = localIdentity.deviceId else { return }
        let engine = surfaceEngines[session.id]
        guard engine?.inAltScreen == true else { return }
        let count = max(1, min(10, lines))
        var bytes = Data()
        if let eng = engine, eng.mouseTrackingMode != 0 {
            let wheel = GhosttyReflowEngine.encodeMouseWheel(
                up: up,
                column: max(1, terminalColumns / 2),
                row: max(1, terminalRows / 2),
                sgrEncoding: eng.sgrMouseEncoding
            )
            for _ in 0..<count {
                bytes.append(wheel)
            }
        } else {
            let arrow = GhosttyReflowEngine.encodeKey(up ? .up : .down)
            for _ in 0..<count {
                bytes.append(arrow)
            }
        }
        Task {
            try? await client.sendPtyInput(
                on: dev,
                clientIdentity: localIdentity,
                clientDeviceId: clientDevId,
                terminal: session.terminalAddress,
                data: bytes
            )
        }
    }

    private func effectiveBracketedPaste(for session: CodaAgentSession) -> Bool {
        let eng = surfaceEngines[session.id]
        return CodaTerminalTranscriptHelper.resolveEffectiveBracketedPaste(
            explicitSeen: eng?.bracketedPasteExplicit ?? false,
            engineEnabled: eng?.bracketedPasteEnabled ?? false,
            agentName: session.agentName
        )
    }

    public func sendBracketedPastePrompt(_ rawPrompt: String, to targetSession: CodaAgentSession? = nil) {
        let trimmed = rawPrompt.trimmingCharacters(in: .newlines)
        guard !trimmed.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
              let session = targetSession ?? activeSheetSession ?? selectedSession,
              let client = networkClient,
              let devId = session.deviceId,
              let dev = remoteMachines.first(where: { $0.id == devId }),
              let clientDevId = localIdentity.deviceId else { return }

        CodaHaptics.mediumTap()
        ctrlActive = false
        altActive = false
        optimisticallyClearNeedsYou(for: session)
        let useBracketed = effectiveBracketedPaste(for: session)
        let bytes = CodaTerminalTranscriptHelper.encodeBracketedPaste(
            trimmed,
            appendCarriageReturn: true,
            bracketedModeEnabled: useBracketed
        )
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
                try? await Task.sleep(nanoseconds: 250_000_000)
                await refreshTerminalScreen(for: session, forceSnapshot: true)
            }
        }
    }

    public func submitComposePrompt(for targetSession: CodaAgentSession? = nil) {
        guard let session = targetSession ?? activeSheetSession ?? selectedSession else { return }
        let draft = composeDraft(for: session.id)
        let trimmed = draft.trimmingCharacters(in: .newlines)
        guard !trimmed.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
              let client = networkClient,
              let devId = session.deviceId,
              let dev = remoteMachines.first(where: { $0.id == devId }),
              let clientDevId = localIdentity.deviceId else { return }

        ctrlActive = false
        altActive = false
        optimisticallyClearNeedsYou(for: session)
        isSendingAction = true
        let toSend = trimmed
        let useBracketed = effectiveBracketedPaste(for: session)
        let isAttached = attachedSessionIDs.contains(session.id)
        setComposeDraft("", for: session.id)
        composeText = ""
        showComposeSheet = false

        Task {
            defer { self.isSendingAction = false }
            let bytes = CodaTerminalTranscriptHelper.encodeBracketedPaste(
                toSend,
                appendCarriageReturn: true,
                bracketedModeEnabled: useBracketed
            )
            try? await client.sendPtyInput(
                on: dev,
                clientIdentity: localIdentity,
                clientDeviceId: clientDevId,
                terminal: session.terminalAddress,
                data: bytes
            )
            if !isAttached {
                try? await Task.sleep(nanoseconds: 250_000_000)
                await refreshTerminalScreen(for: session, forceSnapshot: true)
            }
        }
    }

    public func uploadAttachment(
        fileURL: URL,
        for targetSession: CodaAgentSession? = nil,
        onUploadedToken: ((String) -> Void)? = nil
    ) {
        guard let session = targetSession ?? activeSheetSession ?? selectedSession else { return }
        let maxUploadBytes = 4 * 1024 * 1024
        Task {
            let granted = fileURL.startAccessingSecurityScopedResource()
            defer {
                if granted { fileURL.stopAccessingSecurityScopedResource() }
            }
            if let fileSize = (try? fileURL.resourceValues(forKeys: [.fileSizeKey]).fileSize),
               fileSize > maxUploadBytes {
                uploadBannerIsError = true
                uploadStatusBanner = tr(
                    "File exceeds 4 MiB upload limit.",
                    "文件超过 4 MiB 上传限制。",
                    locale: locale
                )
                return
            }
            guard let data = try? Data(contentsOf: fileURL) else {
                uploadBannerIsError = true
                uploadStatusBanner = tr("Could not read selected file.", "读取文件失败。", locale: locale)
                return
            }
            uploadAttachmentData(
                data: data,
                filename: fileURL.lastPathComponent,
                for: session,
                onUploadedToken: onUploadedToken
            )
        }
    }

    public func uploadAttachmentData(
        data: Data,
        filename: String,
        for targetSession: CodaAgentSession? = nil,
        onUploadedToken: ((String) -> Void)? = nil
    ) {
        guard let session = targetSession ?? activeSheetSession ?? selectedSession else { return }
        guard let devId = session.deviceId,
              let dev = remoteMachines.first(where: { $0.id == devId }) else {
            let msg = tr(
                "Upload failed: target host is offline or unavailable.",
                "上传失败：目标主机离线或不可用。",
                locale: locale
            )
            uploadBannerIsError = true
            uploadStatusBanner = msg
            presentActionErrorToast(workspaceName: session.workspaceName, sessionId: session.id, message: msg)
            return
        }

        // Keep raw upload under 4 MiB so double-Base64 inside encrypted RelayWireFrame stays below 8 MiB.
        let maxUploadBytes = 4 * 1024 * 1024
        guard data.count <= maxUploadBytes else {
            uploadBannerIsError = true
            uploadStatusBanner = tr(
                "File exceeds 4 MiB upload limit.",
                "文件超过 4 MiB 上传限制。",
                locale: locale
            )
            return
        }

        let targetSid = session.id
        let epoch = workspaceEpoch
        uploadingAttachmentCount += 1
        uploadBannerIsError = false
        uploadStatusBanner = tr("Uploading \(filename)...", "正在上传 \(filename)...", locale: locale)
        Task { [weak self] in
            guard let self else { return }
            defer {
                if self.workspaceEpoch == epoch {
                    self.uploadingAttachmentCount = max(0, self.uploadingAttachmentCount - 1)
                }
            }
            do {
                let client = try await self.preparedClientOrThrow()
                if self.localIdentity.deviceId == nil {
                    await self.ensureClientDeviceRegistered(using: client)
                }
                guard self.workspaceEpoch == epoch else { return }
                let clientDevId = self.localIdentity.deviceId ?? devId
                let remotePath = try await client.uploadTerminalFile(
                    on: dev,
                    clientIdentity: self.localIdentity,
                    clientDeviceId: clientDevId,
                    terminal: session.terminalAddress,
                    filename: filename,
                    data: data
                )
                guard self.workspaceEpoch == epoch else { return }
                let inserted = codaShellQuotePath(remotePath)
                if let onUploadedToken {
                    onUploadedToken(inserted)
                } else {
                    self.appendComposeDraft(inserted, for: targetSid, padTrailingSpace: true)
                }
                self.uploadBannerIsError = false
                self.uploadStatusBanner = tr("Attached \(inserted)", "已附加 \(inserted)", locale: self.locale)
            } catch {
                guard self.workspaceEpoch == epoch else { return }
                let errMsg = tr(
                    "Upload failed: \(error.localizedDescription)",
                    "上传失败：\(error.localizedDescription)",
                    locale: self.locale
                )
                self.uploadBannerIsError = true
                self.uploadStatusBanner = errMsg
                self.authError = errMsg
                self.presentActionErrorToast(
                    workspaceName: session.workspaceName,
                    sessionId: session.id,
                    message: errMsg
                )
            }
        }
    }

    public func toggleVoiceDictation(
        for targetSessionID: String? = nil,
        onTranscribed: ((String) -> Void)? = nil
    ) async {
        voiceError = nil
        if isTranscribingVoice || isStartingVoice { return }

        if isRecordingVoice {
            voiceMeterTask?.cancel()
            voiceMeterTask = nil
            voiceGeneration &+= 1
            let gen = voiceGeneration
            let epoch = workspaceEpoch
            let originSid = voiceDictationSessionID ?? targetSessionID ?? activeSheetSessionID ?? selectedSessionID
            let customHandler = pendingVoiceTranscriptHandler ?? onTranscribed
            pendingVoiceTranscriptHandler = nil
            isRecordingVoice = false
            isTranscribingVoice = true
            voicePartialTranscript = ""

            let daemonFallback: (@Sendable (Data, String) async throws -> String)? = buildDaemonWhisperFallback(forSessionID: originSid)

            let task = Task { @MainActor [weak self] in
                guard let self else { return }
                defer {
                    if self.voiceGeneration == gen {
                        self.isTranscribingVoice = false
                        self.voiceDictationSessionID = nil
                        self.voiceRecordingSeconds = 0
                        self.voiceAudioLevel = 0
                        self.voicePartialTranscript = ""
                        self.voiceTask = nil
                    }
                }
                do {
                    let transcript = try await self.voiceRecorder.stopAndTranscribe(
                        config: self.settings.voiceConfig,
                        locale: self.locale,
                        onPartialTranscript: { [weak self] partial in
                            Task { @MainActor in
                                guard let self, self.voiceGeneration == gen, self.workspaceEpoch == epoch else { return }
                                self.voicePartialTranscript = partial
                            }
                        },
                        daemonWhisperFallback: daemonFallback
                    )
                    guard !Task.isCancelled, self.voiceGeneration == gen, self.workspaceEpoch == epoch else { return }
                    if !transcript.isEmpty {
                        if let customHandler {
                            // Only invoke view-scoped handler if the originating session is still active!
                            let originStillVisible = originSid == nil
                                || originSid == self.selectedSessionID
                                || (self.duoSplitMode == .dualTerminalSplit && originSid == self.secondarySession?.id)
                                || originSid == self.activeSheetSessionID
                            if originStillVisible {
                                customHandler(transcript)
                            } else if let originSid {
                                self.appendComposeDraft(transcript, for: originSid)
                            }
                        } else if let originSid {
                            self.appendComposeDraft(transcript, for: originSid)
                        } else if self.composeText.isEmpty || self.composeText.hasSuffix(" ") || self.composeText.hasSuffix("\n") {
                            self.composeText += transcript
                        } else {
                            self.composeText += " \(transcript)"
                        }
                    }
                } catch is CancellationError {
                    return
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
        let originSid = targetSessionID ?? activeSheetSessionID ?? selectedSessionID
        voiceDictationSessionID = originSid
        voiceRecordingSeconds = 0
        voiceAudioLevel = 0
        voicePartialTranscript = ""
        pendingVoiceTranscriptHandler = onTranscribed
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
                if self.settings.voiceConfig.hasLocalPlaintextApiKey {
                    _ = try CodaVoiceRecorder.validatedTranscriptionURL(from: self.settings.voiceConfig.baseURL)
                }
                try await self.voiceRecorder.startRecording()
                if Task.isCancelled || self.voiceGeneration != gen || self.workspaceEpoch != epoch {
                    if self.voiceGeneration == gen {
                        self.voiceRecorder.cancelRecording()
                        self.pendingVoiceTranscriptHandler = nil
                        self.voiceDictationSessionID = nil
                        self.isRecordingVoice = false
                    }
                    return
                }
                self.isRecordingVoice = true
                let startedAt = Date()
                self.voiceMeterTask?.cancel()
                self.voiceMeterTask = Task { @MainActor [weak self] in
                    while !Task.isCancelled {
                        try? await Task.sleep(nanoseconds: 200_000_000)
                        guard let self, self.voiceGeneration == gen, self.isRecordingVoice else { return }
                        self.voiceRecordingSeconds = max(0, Int(Date().timeIntervalSince(startedAt)))
                        self.voiceAudioLevel = self.voiceRecorder.currentNormalizedAudioLevel()
                    }
                }
            } catch {
                guard self.voiceGeneration == gen else { return }
                self.voiceRecorder.cancelRecording()
                self.pendingVoiceTranscriptHandler = nil
                self.voiceDictationSessionID = nil
                self.isRecordingVoice = false
                guard !Task.isCancelled, self.workspaceEpoch == epoch else { return }
                self.voiceError = error.localizedDescription
            }
        }
        voiceTask = task
        await task.value
    }

    private func buildDaemonWhisperFallback(
        forSessionID sessionID: String?
    ) -> (@Sendable (Data, String) async throws -> String)? {
        guard let client = networkClient, let clientDevId = localIdentity.deviceId else { return nil }
        let targetSession = sessionID.flatMap { sid in sessions.first(where: { $0.id == sid }) } ?? selectedSession
        let candidateDev: CodaRemoteDevice?
        if let devId = targetSession?.deviceId, let dev = remoteMachines.first(where: { $0.id == devId && $0.isOnline }) {
            candidateDev = dev
        } else {
            candidateDev = remoteMachines.first(where: { $0.isOnline })
        }
        guard let dev = candidateDev else { return nil }
        let daemonHasKey = settings.voiceConfig.hasMaskedDaemonKey
            || (daemonVoiceConfigsByDevice[dev.id]?.apiKey.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty == false)
        guard daemonHasKey else { return nil }
        let ident = localIdentity
        return { audioData, language in
            let res = try await client.transcribeAudio(
                on: dev,
                clientIdentity: ident,
                clientDeviceId: clientDevId,
                audioData: audioData,
                mimeType: "audio/m4a",
                filename: "dictation.m4a",
                language: language
            )
            if res.ok {
                let clean = res.text.trimmingCharacters(in: .whitespacesAndNewlines)
                if !clean.isEmpty {
                    return clean
                }
            }
            throw CodaVoiceRecorderError.transcriptionFailed(
                res.error ?? "Remote host Whisper transcription returned no speech."
            )
        }
    }

    public func cancelVoiceDictation() {
        voiceGeneration &+= 1
        voiceMeterTask?.cancel()
        voiceMeterTask = nil
        voiceTask?.cancel()
        voiceTask = nil
        pendingVoiceTranscriptHandler = nil
        voiceDictationSessionID = nil
        voiceRecordingSeconds = 0
        voiceAudioLevel = 0
        voicePartialTranscript = ""
        voiceRecorder.cancelRecording()
        isStartingVoice = false
        isRecordingVoice = false
        isTranscribingVoice = false
    }

    public func saveVoiceConfig() async {
        if settings.voiceConfig.hasMaskedDaemonKey {
            settings.voiceConfig.apiKey = ""
        }
        let usesNativeFallback = CodaVoiceRecorder.usesNativeSpeechFallback(for: settings.voiceConfig)
        if !usesNativeFallback || !settings.voiceConfig.baseURL.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            do {
                _ = try CodaVoiceRecorder.validatedTranscriptionURL(from: settings.voiceConfig.baseURL)
            } catch {
                if !usesNativeFallback {
                    voiceConfigSyncMessage = error.localizedDescription
                    return
                }
            }
        }
        settings.save()
        voiceConfigSyncMessage = usesNativeFallback
            ? tr("Saved (Apple Speech zero-config fallback active).", "已保存（已启用 Apple 原生零配置语音识别）。", locale: locale)
            : tr("Saved to secure storage.", "已保存至安全存储。", locale: locale)
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
                daemonVoiceConfigsByDevice[host.id] = synced
                var merged = synced
                if merged.hasMaskedDaemonKey || merged.apiKey.isEmpty {
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
        }
    }

    // MARK: - Offline Alert Webhooks, Stale Web Device Pruning & Workspace Bootstrapper

    public func loadNotificationWebhookSettings() async {
        guard isSignedIn, let client = await preparedClient() else { return }
        let epoch = workspaceEpoch
        do {
            let fetched = try await client.fetchNotificationSettings()
            guard self.workspaceEpoch == epoch else { return }
            self.notificationSettings = fetched
        } catch {
            if self.workspaceEpoch == epoch {
                self.notificationErrorMessage = error.localizedDescription
            }
        }
    }

    public func saveNotificationWebhookSettings() async {
        guard isSignedIn, let client = await preparedClient() else { return }
        let epoch = workspaceEpoch
        notificationErrorMessage = nil
        notificationStatusMessage = nil
        isSavingNotificationSettings = true
        defer {
            if self.workspaceEpoch == epoch {
                self.isSavingNotificationSettings = false
            }
        }
        do {
            let updated = try await client.updateNotificationSettings(notificationSettings)
            guard self.workspaceEpoch == epoch else { return }
            self.notificationSettings = updated
            self.notificationStatusMessage = tr(
                "Saved offline webhook alert settings.",
                "已保存离线 Webhook 告警推送配置。",
                locale: locale
            )
        } catch {
            if self.workspaceEpoch == epoch {
                self.notificationErrorMessage = error.localizedDescription
            }
        }
    }

    public func sendTestNotificationWebhook() async {
        guard isSignedIn, let client = await preparedClient() else { return }
        let epoch = workspaceEpoch
        notificationErrorMessage = nil
        notificationStatusMessage = nil
        isTestingNotificationSettings = true
        defer {
            if self.workspaceEpoch == epoch {
                self.isTestingNotificationSettings = false
            }
        }
        do {
            let updated = try await client.updateNotificationSettings(notificationSettings)
            guard self.workspaceEpoch == epoch else { return }
            self.notificationSettings = updated
            let res = try await client.testNotificationSettings()
            guard self.workspaceEpoch == epoch else { return }
            if !res.errors.isEmpty {
                self.notificationErrorMessage = res.errors.joined(separator: " · ")
            }
            if !res.deliveredChannels.isEmpty {
                let list = res.deliveredChannels.joined(separator: ", ")
                self.notificationStatusMessage = tr(
                    "Test push delivered to: \(list)",
                    "测试推送已送达：\(list)",
                    locale: locale
                )
            } else if res.errors.isEmpty {
                self.notificationStatusMessage = tr(
                    "No webhook channels configured yet.",
                    "尚未配置任何推送通道 URL。",
                    locale: locale
                )
            }
        } catch {
            if self.workspaceEpoch == epoch {
                self.notificationErrorMessage = error.localizedDescription
            }
        }
    }

    public func pruneStaleWebDevices(maxIdleDays: Int = 0) async {
        guard isSignedIn, let client = await preparedClient() else { return }
        let epoch = workspaceEpoch
        authError = nil
        accountStatusMessage = nil
        isPruningStaleDevices = true
        defer {
            if self.workspaceEpoch == epoch {
                self.isPruningStaleDevices = false
            }
        }
        do {
            let res = try await client.pruneStaleDevices(
                maxIdleDays: maxIdleDays,
                excludeDeviceId: localIdentity.deviceId
            )
            guard self.workspaceEpoch == epoch else { return }
            accountStatusMessage = tr(
                "Pruned \(res.prunedCount) offline web device(s).",
                "已清理 \(res.prunedCount) 台离线网页端设备。",
                locale: locale
            )
            await refreshMachinesAndSessions()
        } catch {
            if self.workspaceEpoch == epoch {
                authError = error.localizedDescription
            }
        }
    }

    public func openBootstrapWorkspaceSheet(
        for device: CodaRemoteDevice? = nil,
        defaultParentDir: String? = nil
    ) {
        let targetDev = device ?? remoteMachines.first(where: { $0.isOnline }) ?? remoteMachines.first
        bootstrapTargetDeviceID = targetDev?.id
        if let dir = defaultParentDir?.trimmingCharacters(in: .whitespacesAndNewlines), !dir.isEmpty {
            bootstrapDefaultParentDir = dir
        } else if let devId = targetDev?.id,
                  let firstRepo = workspacesByDevice[devId]?.first?.rootPath,
                  !firstRepo.isEmpty {
            let parent = URL(fileURLWithPath: firstRepo).deletingLastPathComponent().path
            bootstrapDefaultParentDir = parent.isEmpty ? "~/workspace" : parent
        } else {
            bootstrapDefaultParentDir = "~/workspace"
        }
        bootstrapErrorMessage = nil
        bootstrapStatusMessage = nil
        showBootstrapWorkspaceSheet = true
    }

    public func bootstrapWorkspaceAndLaunch(
        on device: CodaRemoteDevice,
        parentDir: String,
        folderName: String,
        gitCloneUrl: String?,
        agent: String?,
        initialPrompt: String? = nil
    ) async -> Bool {
        guard isSignedIn else {
            let msg = tr("Please sign in before launching a workspace.", "请先登录后再创建或启动工作区。", locale: locale)
            bootstrapErrorMessage = msg
            authError = msg
            return false
        }
        let epoch = workspaceEpoch
        bootstrapErrorMessage = nil
        bootstrapStatusMessage = nil
        authError = nil
        isBootstrappingWorkspace = true
        defer {
            if self.workspaceEpoch == epoch {
                self.isBootstrappingWorkspace = false
            }
        }

        do {
            let client = try await preparedClientOrThrow()
            if localIdentity.deviceId == nil {
                await ensureClientDeviceRegistered(using: client)
                guard self.workspaceEpoch == epoch else { return false }
            }
            let clientDevId = localIdentity.deviceId ?? device.id
            _ = try? await client.connectBestAvailable(
                sessionToken: settings.sessionToken,
                targetDevice: device,
                clientIdentity: localIdentity
            )
            let res = try await client.bootstrapRemoteWorkspace(
                device: device,
                clientIdentity: localIdentity,
                clientDeviceId: clientDevId,
                parentDir: parentDir,
                folderName: folderName,
                gitCloneUrl: gitCloneUrl
            )
            guard self.workspaceEpoch == epoch else { return false }
            guard res.ok else {
                let msg = res.detail.isEmpty
                    ? tr("Workspace bootstrap failed.", "创建或克隆工作区失败。", locale: locale)
                    : res.detail
                bootstrapErrorMessage = msg
                authError = msg
                return false
            }

            bootstrapStatusMessage = res.workspacePath
            let cleanAgent = (agent ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            if !cleanAgent.isEmpty && cleanAgent != "none" {
                let cleanPrompt = initialPrompt?.trimmingCharacters(in: .whitespacesAndNewlines)
                do {
                    let launched = try await client.launchAgent(
                        on: device,
                        clientIdentity: localIdentity,
                        clientDeviceId: clientDevId,
                        agent: cleanAgent,
                        name: nil,
                        cwd: res.workspacePath,
                        initialPrompt: (cleanPrompt?.isEmpty == false) ? cleanPrompt : nil,
                        cols: max(48, terminalColumns),
                        rows: 32
                    )
                    pendingSelectSessionID = "\(device.id.uuidString.lowercased())/\(launched.address)"
                } catch {
                    if self.workspaceEpoch == epoch {
                        let msg = tr(
                            "Workspace created at \(res.workspacePath), but launching \(cleanAgent) failed: \(error.localizedDescription)",
                            "已在 \(res.workspacePath) 创建工作区，但启动 \(cleanAgent) 失败：\(error.localizedDescription)",
                            locale: locale
                        )
                        bootstrapErrorMessage = msg
                        authError = msg
                        presentActionErrorToast(workspaceName: device.name, message: msg)
                        await refreshMachinesAndSessions(forceWorkspaces: true)
                    }
                    return false
                }
            }
            await refreshMachinesAndSessions(forceWorkspaces: true)
            return true
        } catch {
            if self.workspaceEpoch == epoch {
                bootstrapErrorMessage = error.localizedDescription
                authError = error.localizedDescription
            }
            return false
        }
    }

    public func launchAgent(agent: String, on device: CodaRemoteDevice, cwd: String?, prompt: String? = nil) {
        let epoch = self.workspaceEpoch
        authError = nil
        Task { [weak self] in
            guard let self else { return }
            guard self.isSignedIn else {
                let msg = tr("Please sign in before launching an agent.", "请先登录后再启动 Agent。", locale: self.locale)
                self.authError = msg
                self.presentActionErrorToast(workspaceName: device.name, message: msg)
                return
            }
            do {
                let client = try await self.preparedClientOrThrow()
                if self.localIdentity.deviceId == nil {
                    await self.ensureClientDeviceRegistered(using: client)
                }
                guard self.workspaceEpoch == epoch else { return }
                let clientDevId = self.localIdentity.deviceId ?? device.id
                _ = try? await client.connectBestAvailable(
                    sessionToken: self.settings.sessionToken,
                    targetDevice: device,
                    clientIdentity: self.localIdentity
                )
                let launched = try await client.launchAgent(
                    on: device,
                    clientIdentity: self.localIdentity,
                    clientDeviceId: clientDevId,
                    agent: agent,
                    name: nil,
                    cwd: cwd,
                    initialPrompt: prompt,
                    cols: max(48, self.terminalColumns),
                    rows: 32
                )
                guard self.workspaceEpoch == epoch else { return }
                self.pendingSelectSessionID = "\(device.id.uuidString.lowercased())/\(launched.address)"
                try? await Task.sleep(nanoseconds: 350_000_000)
                if self.workspaceEpoch == epoch {
                    await self.refreshMachinesAndSessions()
                }
            } catch {
                guard self.workspaceEpoch == epoch else { return }
                let msg = tr(
                    "Failed to launch \(agent) on \(device.name): \(error.localizedDescription)",
                    "在 \(device.name) 上启动 \(agent) 失败：\(error.localizedDescription)",
                    locale: self.locale
                )
                self.authError = msg
                self.uploadBannerIsError = true
                self.uploadStatusBanner = msg
                self.presentActionErrorToast(workspaceName: device.name, message: msg)
            }
        }
    }

    public func loadGitDiff(for targetSession: CodaAgentSession?, presentModal: Bool = true) {
        guard let session = targetSession ?? selectedSession,
              let client = networkClient,
              let devId = session.deviceId,
              let dev = remoteMachines.first(where: { $0.id == devId }),
              let clientDevId = localIdentity.deviceId else { return }
        resetInspectorStateIfSessionChanged(to: session)
        gitDiffGeneration &+= 1
        let diffGen = gitDiffGeneration
        let epoch = workspaceEpoch
        gitDiffResult = nil
        gitDiffError = nil
        gitDiffSelectedFile = nil
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
            do {
                let fetched = try await client.fetchRemoteGitDiff(
                    device: dev,
                    clientIdentity: self.localIdentity,
                    clientDeviceId: clientDevId,
                    cwd: session.cwd
                )
                if self.gitDiffGeneration == diffGen && self.workspaceEpoch == epoch {
                    self.gitDiffResult = fetched
                    if let rpcErr = fetched.error?.nilIfEmpty {
                        self.gitDiffError = rpcErr
                    }
                }
            } catch {
                if self.gitDiffGeneration == diffGen && self.workspaceEpoch == epoch {
                    self.gitDiffError = error.localizedDescription
                }
            }
        }
    }

    public func loadGitDiffForSelected(presentModal: Bool = true) {
        loadGitDiff(for: selectedSession, presentModal: presentModal)
    }

    public func sessionMarkdownTranscript(for session: CodaAgentSession) -> String {
        let styled = terminalLinesBySession[session.id] ?? []
        let plainLines: [String]
        if !styled.isEmpty {
            plainLines = styled.map { $0.spans.map(\.text).joined() }
        } else if let raw = rawScreenTextBySession[session.id], !raw.isEmpty {
            plainLines = raw.components(separatedBy: "\n")
        } else {
            plainLines = session.screenPreview.components(separatedBy: "\n")
        }
        return CodaTerminalTranscriptHelper.buildMarkdown(
            title: session.taskTitle,
            cwd: session.cwd,
            hostName: session.hostName,
            lines: plainLines
        )
    }

    public func openRemoteInspector(
        for targetSession: CodaAgentSession?,
        mode: CodaInspectorSheetMode = .files
    ) {
        guard let session = targetSession ?? selectedSession else { return }
        resetInspectorStateIfSessionChanged(to: session)
        activeSheetSessionID = session.id
        inspectorSheetMode = mode
        showRemoteFilesSheet = true
        let root = session.cwd.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? "." : session.cwd
        if remoteFileRootPath.isEmpty {
            remoteFileRootPath = root
        }
        remoteFileSelectedRel = nil
        remoteFilePreviewText = ""
        remoteFileTruncated = false
        if mode == .files {
            browseRemoteSubPath(remoteFileSubPath, for: session)
        } else if mode == .changes {
            loadGitDiff(for: session, presentModal: false)
        }
    }

    public func browseRemoteSubPath(_ subPath: String, for session: CodaAgentSession) {
        guard let client = networkClient,
              let devId = session.deviceId,
              let dev = remoteMachines.first(where: { $0.id == devId }),
              let clientDevId = localIdentity.deviceId else { return }
        resetInspectorStateIfSessionChanged(to: session)
        remoteFileRequestGeneration &+= 1
        let reqGen = remoteFileRequestGeneration
        let epoch = workspaceEpoch
        let fallbackCwd = session.cwd.trimmingCharacters(in: .whitespacesAndNewlines)
        let root = remoteFileRootPath.isEmpty ? (fallbackCwd.isEmpty ? "." : fallbackCwd) : remoteFileRootPath
        remoteFileRootPath = root
        remoteFileSubPath = subPath
        remoteFileSelectedRel = nil
        remoteFilePreviewText = ""
        remoteFileTruncated = false
        remoteFileError = nil
        remoteFileLoading = true
        Task { [weak self] in
            guard let self else { return }
            defer {
                if self.remoteFileRequestGeneration == reqGen && self.workspaceEpoch == epoch {
                    self.remoteFileLoading = false
                }
            }
            do {
                let res = try await client.listRemoteDirectory(
                    on: dev,
                    clientIdentity: self.localIdentity,
                    clientDeviceId: clientDevId,
                    rootPath: root,
                    subPath: subPath
                )
                guard self.remoteFileRequestGeneration == reqGen && self.workspaceEpoch == epoch else { return }
                self.remoteFileRootPath = res.rootPath
                self.remoteFileSubPath = res.subPath
                self.remoteFileEntries = res.entries
                self.remoteFileError = res.error
            } catch {
                if self.remoteFileRequestGeneration == reqGen && self.workspaceEpoch == epoch {
                    self.remoteFileError = error.localizedDescription
                }
            }
        }
    }

    public func previewRemoteFile(
        _ relPath: String,
        for session: CodaAgentSession,
        baseRootOverride: String? = nil
    ) {
        guard let client = networkClient,
              let devId = session.deviceId,
              let dev = remoteMachines.first(where: { $0.id == devId }),
              let clientDevId = localIdentity.deviceId else { return }
        resetInspectorStateIfSessionChanged(to: session)
        remoteFileRequestGeneration &+= 1
        let reqGen = remoteFileRequestGeneration
        let epoch = workspaceEpoch
        let fallbackCwd = session.cwd.trimmingCharacters(in: .whitespacesAndNewlines)
        let defaultRoot = remoteFileRootPath.isEmpty ? (fallbackCwd.isEmpty ? "." : fallbackCwd) : remoteFileRootPath
        if remoteFileRootPath.isEmpty {
            remoteFileRootPath = defaultRoot
        }
        let effectiveRoot = baseRootOverride?.nilIfEmpty ?? defaultRoot
        let fullPath = CodaTerminalTranscriptHelper.joinRootAndRel(rootPath: effectiveRoot, relPath: relPath)
        remoteFileSelectedRel = relPath
        remoteFilePreviewText = ""
        remoteFileTruncated = false
        remoteFileError = nil
        remoteFileLoading = true
        Task { [weak self] in
            guard let self else { return }
            defer {
                if self.remoteFileRequestGeneration == reqGen && self.workspaceEpoch == epoch {
                    self.remoteFileLoading = false
                }
            }
            do {
                let res = try await client.readRemoteFilePreview(
                    on: dev,
                    clientIdentity: self.localIdentity,
                    clientDeviceId: clientDevId,
                    path: fullPath
                )
                guard self.remoteFileRequestGeneration == reqGen && self.workspaceEpoch == epoch else { return }
                if let err = res.error {
                    self.remoteFileError = err
                } else {
                    self.remoteFilePreviewText = res.content
                    self.remoteFileTruncated = res.truncated
                }
            } catch {
                if self.remoteFileRequestGeneration == reqGen && self.workspaceEpoch == epoch {
                    self.remoteFileError = error.localizedDescription
                }
            }
        }
    }

    public func loadRemotePortPreview(port: Int, path: String, for session: CodaAgentSession) {
        guard let client = networkClient,
              let devId = session.deviceId,
              let dev = remoteMachines.first(where: { $0.id == devId }),
              let clientDevId = localIdentity.deviceId else { return }
        resetInspectorStateIfSessionChanged(to: session)
        portPreviewGeneration &+= 1
        let previewGen = portPreviewGeneration
        let epoch = workspaceEpoch
        portPreviewError = nil
        portPreviewLoading = true
        Task { [weak self] in
            guard let self else { return }
            defer {
                if self.portPreviewGeneration == previewGen && self.workspaceEpoch == epoch {
                    self.portPreviewLoading = false
                }
            }
            do {
                let res = try await client.fetchLocalPortPreview(
                    on: dev,
                    clientIdentity: self.localIdentity,
                    clientDeviceId: clientDevId,
                    port: port,
                    path: path
                )
                guard self.portPreviewGeneration == previewGen && self.workspaceEpoch == epoch else { return }
                self.portPreviewResult = res
            } catch {
                if self.portPreviewGeneration == previewGen && self.workspaceEpoch == epoch {
                    self.portPreviewError = error.localizedDescription
                }
            }
        }
    }

    // MARK: - v1.1.0 Synced Prompt Snippets, Password Reset Token & Admin Friend Circle

    private static func snippetsCacheKey(serverBaseURL: String, ownerHandle: String) -> String {
        let regKey = CodaPersistedSettings.registrationKey(serverBaseURL: serverBaseURL, ownerHandle: ownerHandle)
        return "dev.codahub.client.prompt_snippets.v1.\(regKey)"
    }

    private static func userProfileCacheKey(serverBaseURL: String, ownerHandle: String) -> String {
        let regKey = CodaPersistedSettings.registrationKey(serverBaseURL: serverBaseURL, ownerHandle: ownerHandle)
        return "dev.codahub.client.user_profile.v1.\(regKey)"
    }

    private static func loadCachedSnippets(serverBaseURL: String, ownerHandle: String) -> [CodaPromptSnippet] {
        let key = snippetsCacheKey(serverBaseURL: serverBaseURL, ownerHandle: ownerHandle)
        guard let data = UserDefaults.standard.data(forKey: key),
              let decoded = try? JSONDecoder().decode([CodaPromptSnippet].self, from: data) else {
            return CodaPromptSnippet.defaultStarterSnippets
        }
        return CodaPromptSnippet.sanitizeList(decoded)
    }

    private static func saveCachedSnippets(
        _ snippets: [CodaPromptSnippet],
        serverBaseURL: String,
        ownerHandle: String
    ) {
        let key = snippetsCacheKey(serverBaseURL: serverBaseURL, ownerHandle: ownerHandle)
        let clean = CodaPromptSnippet.sanitizeList(snippets)
        if let data = try? JSONEncoder().encode(clean) {
            UserDefaults.standard.set(data, forKey: key)
        }
    }

    private static func loadCachedUserProfile(serverBaseURL: String, ownerHandle: String) -> CodaAuthUserProfile? {
        let key = userProfileCacheKey(serverBaseURL: serverBaseURL, ownerHandle: ownerHandle)
        guard let data = UserDefaults.standard.data(forKey: key) else { return nil }
        return try? JSONDecoder().decode(CodaAuthUserProfile.self, from: data)
    }

    private static func saveCachedUserProfile(
        _ profile: CodaAuthUserProfile,
        serverBaseURL: String,
        ownerHandle: String
    ) {
        let key = userProfileCacheKey(serverBaseURL: serverBaseURL, ownerHandle: ownerHandle)
        if let data = try? JSONEncoder().encode(profile) {
            UserDefaults.standard.set(data, forKey: key)
        }
    }

    private func syncAccountProfileAndSnippets(using client: CodaNetworkClient, epoch: UInt64) async {
        let snipGenBefore = snippetsMutationGeneration
        if let profile = try? await client.fetchCurrentUserProfile() {
            guard self.workspaceEpoch == epoch, isSignedIn else { return }
            self.currentUser = profile
            if settings.ownerHandle != profile.handle {
                settings.ownerHandle = profile.handle
                settings.save()
            }
            Self.saveCachedUserProfile(
                profile,
                serverBaseURL: settings.serverBaseURLString,
                ownerHandle: profile.handle
            )
        }
        if let fetchedSnippets = try? await client.fetchPromptSnippets() {
            guard self.workspaceEpoch == epoch, isSignedIn else { return }
            if self.snippetsMutationGeneration == snipGenBefore {
                self.promptSnippets = fetchedSnippets
                Self.saveCachedSnippets(
                    fetchedSnippets,
                    serverBaseURL: settings.serverBaseURLString,
                    ownerHandle: settings.ownerHandle
                )
            }
        }
        lastAccountMetaSyncAt = Date()
        if currentUser?.isAdmin == true {
            await loadAdminDashboardData()
        }
    }

    public func loadPromptSnippets() async {
        guard isSignedIn, let client = await preparedClient() else { return }
        let epoch = workspaceEpoch
        let genBefore = snippetsMutationGeneration
        if let fetched = try? await client.fetchPromptSnippets() {
            guard self.workspaceEpoch == epoch, isSignedIn, self.snippetsMutationGeneration == genBefore else { return }
            self.promptSnippets = fetched
            Self.saveCachedSnippets(
                fetched,
                serverBaseURL: settings.serverBaseURLString,
                ownerHandle: settings.ownerHandle
            )
        }
    }

    public func insertSnippetPrompt(_ prompt: String, for sessionID: String) {
        let clean = prompt.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !clean.isEmpty else { return }
        var cur = composeDraft(for: sessionID)
        if cur.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            cur = clean
        } else if cur.hasSuffix("\n") {
            cur += clean
        } else {
            cur += "\n" + clean
        }
        setComposeDraft(cur, for: sessionID)
        CodaHaptics.lightTap()
    }

    @discardableResult
    public func saveDraftAsSnippet(_ rawDraft: String, customTitle: String? = nil) async -> Bool {
        let cleanPrompt = rawDraft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !cleanPrompt.isEmpty else {
            uploadStatusBanner = tr(
                "Type a prompt draft first before saving as a snippet.",
                "请先输入指令内容再保存为快捷片段。",
                locale: locale
            )
            return false
        }
        let resolvedTitle = customTitle?.nilIfEmpty ?? CodaPromptSnippet.derivedTitle(fromPrompt: cleanPrompt)
        guard let candidate = CodaPromptSnippet(title: resolvedTitle, prompt: cleanPrompt).sanitized() else {
            return false
        }

        var updated = promptSnippets
        if let dupIdx = updated.firstIndex(where: {
            $0.prompt.trimmingCharacters(in: .whitespacesAndNewlines) == candidate.prompt
        }) {
            updated[dupIdx].title = candidate.title
        } else {
            if updated.count >= CodaPromptSnippet.maxSnippetsCount {
                uploadStatusBanner = tr(
                    "Maximum of 24 snippets reached — delete one first.",
                    "最多可保存 24 条快捷片段，请先删除旧片段。",
                    locale: locale
                )
                return false
            }
            updated.append(candidate)
        }

        let cleanList = CodaPromptSnippet.sanitizeList(updated)
        snippetsMutationGeneration &+= 1
        let mutGen = snippetsMutationGeneration
        let epoch = workspaceEpoch
        promptSnippets = cleanList
        if isSignedIn {
            Self.saveCachedSnippets(
                cleanList,
                serverBaseURL: settings.serverBaseURLString,
                ownerHandle: settings.ownerHandle
            )
        }
        CodaHaptics.success()
        uploadBannerIsError = false
        uploadStatusBanner = tr(
            "Saved snippet “\(candidate.title)”.",
            "已保存快捷片段「\(candidate.title)」。",
            locale: locale
        )

        guard isSignedIn, let client = await preparedClient() else { return true }
        isSavingSnippets = true
        defer {
            if self.workspaceEpoch == epoch {
                self.isSavingSnippets = false
            }
        }
        do {
            let remoteSaved = try await client.savePromptSnippets(cleanList)
            if self.workspaceEpoch == epoch && self.snippetsMutationGeneration == mutGen {
                self.promptSnippets = remoteSaved
                Self.saveCachedSnippets(
                    remoteSaved,
                    serverBaseURL: settings.serverBaseURLString,
                    ownerHandle: settings.ownerHandle
                )
            }
            return true
        } catch {
            if self.workspaceEpoch == epoch {
                self.uploadStatusBanner = error.localizedDescription
            }
            return false
        }
    }

    public func deletePromptSnippet(id: String) async {
        let nextList = promptSnippets.filter { $0.id != id }
        guard nextList.count != promptSnippets.count else { return }
        snippetsMutationGeneration &+= 1
        let mutGen = snippetsMutationGeneration
        let epoch = workspaceEpoch
        promptSnippets = nextList
        if isSignedIn {
            Self.saveCachedSnippets(
                nextList,
                serverBaseURL: settings.serverBaseURLString,
                ownerHandle: settings.ownerHandle
            )
        }
        CodaHaptics.lightTap()

        guard isSignedIn, let client = await preparedClient() else { return }
        isSavingSnippets = true
        defer {
            if self.workspaceEpoch == epoch {
                self.isSavingSnippets = false
            }
        }
        do {
            let remoteSaved = try await client.savePromptSnippets(nextList)
            if self.workspaceEpoch == epoch && self.snippetsMutationGeneration == mutGen {
                self.promptSnippets = remoteSaved
                Self.saveCachedSnippets(
                    remoteSaved,
                    serverBaseURL: settings.serverBaseURLString,
                    ownerHandle: settings.ownerHandle
                )
            }
        } catch {
            if self.workspaceEpoch == epoch {
                self.uploadStatusBanner = error.localizedDescription
            }
        }
    }

    public func resetPasswordWithToken(
        rawTokenOrURL: String,
        newPassword: String
    ) async -> CodaPasswordResetConfirmResult? {
        authError = nil
        accountStatusMessage = nil
        if let origin = extractServerOriginFromAuthLink(rawTokenOrURL),
           origin.lowercased() != normalizedServerURLString().lowercased() {
            updateServerURL(origin)
        }
        guard extractResetTokenFromInput(rawTokenOrURL) != nil else {
            authError = tr(
                "Please paste a valid reset link or rst_... token.",
                "请粘贴有效的密码重置链接或 rst_... 令牌。",
                locale: locale
            )
            return nil
        }
        guard newPassword.utf8.count >= 8 else {
            authError = tr(
                "New password must be at least 8 characters.",
                "新密码长度至少为 8 位字符。",
                locale: locale
            )
            return nil
        }
        guard let client = await preparedClient() else { return nil }
        let epoch = workspaceEpoch
        isResettingPassword = true
        defer {
            if self.workspaceEpoch == epoch {
                self.isResettingPassword = false
            }
        }
        do {
            let res = try await client.resetPasswordWithToken(token: rawTokenOrURL, newPassword: newPassword)
            guard self.workspaceEpoch == epoch else { return nil }
            let handleDisplay = res.handle.isEmpty ? "" : " (@\(res.handle))"
            accountStatusMessage = tr(
                "Password reset successful\(handleDisplay)! Sign in with your new password.",
                "密码重置成功\(handleDisplay)！请使用新密码登录。",
                locale: locale
            )
            CodaHaptics.success()
            return res
        } catch {
            if self.workspaceEpoch == epoch {
                authError = error.localizedDescription
            }
            return nil
        }
    }

    public func loadAdminDashboardData() async {
        guard isSignedIn, currentUser?.isAdmin == true, let client = await preparedClient() else { return }
        let epoch = workspaceEpoch
        isLoadingAdminData = true
        defer {
            if self.workspaceEpoch == epoch {
                self.isLoadingAdminData = false
            }
        }
        async let invitesResult = client.fetchAdminInvites()
        async let usersResult = client.fetchAdminUsers()
        if let invites = try? await invitesResult, self.workspaceEpoch == epoch {
            self.adminInvites = invites
        }
        if let users = try? await usersResult, self.workspaceEpoch == epoch {
            self.adminUsers = users
        }
    }

    @discardableResult
    public func createAdminInvite(
        note: String,
        maxUses: Int = 5,
        validDays: Int = 30
    ) async -> CodaInviteCodeItem? {
        guard isSignedIn, currentUser?.isAdmin == true, let client = await preparedClient() else { return nil }
        let epoch = workspaceEpoch
        adminErrorMessage = nil
        adminStatusMessage = nil
        do {
            let created = try await client.createAdminInvite(note: note, maxUses: maxUses, validDays: validDays)
            guard self.workspaceEpoch == epoch else { return nil }
            adminInvites.insert(created, at: 0)
            if let url = buildInviteRegistrationURL(serverURL: settings.serverBaseURLString, code: created.code) {
                copyTextToClipboard(url)
                adminStatusMessage = tr(
                    "Created \(created.code) & copied invite link.",
                    "已创建 \(created.code) 并复制注册邀请链接。",
                    locale: locale
                )
            } else {
                adminStatusMessage = tr(
                    "Created invite code \(created.code).",
                    "已创建邀请码 \(created.code)。",
                    locale: locale
                )
            }
            CodaHaptics.success()
            return created
        } catch {
            if self.workspaceEpoch == epoch {
                adminErrorMessage = error.localizedDescription
            }
            return nil
        }
    }

    public func deleteAdminInvite(code: String) async {
        guard isSignedIn, currentUser?.isAdmin == true, let client = await preparedClient() else { return }
        let epoch = workspaceEpoch
        adminErrorMessage = nil
        do {
            try await client.deleteAdminInvite(code: code)
            guard self.workspaceEpoch == epoch else { return }
            adminInvites.removeAll { $0.code == code }
            adminStatusMessage = tr("Revoked invite \(code).", "已撤销邀请码 \(code)。", locale: locale)
        } catch {
            if self.workspaceEpoch == epoch {
                adminErrorMessage = error.localizedDescription
            }
        }
    }

    public func generateAdminPasswordResetLink(
        for member: CodaAdminUserSummary
    ) async -> CodaPasswordResetLinkResult? {
        guard isSignedIn, currentUser?.isAdmin == true, let client = await preparedClient() else { return nil }
        let epoch = workspaceEpoch
        adminErrorMessage = nil
        adminStatusMessage = nil
        do {
            let res = try await client.createAdminPasswordResetLink(userId: member.id)
            guard self.workspaceEpoch == epoch else { return nil }
            copyTextToClipboard(res.resetUrl)
            CodaHaptics.success()
            adminStatusMessage = tr(
                "Copied 24h reset link for @\(member.handle) — paste on WeChat.",
                "已复制 @\(member.handle) 的 24 小时密码重置链接，可直接发送至微信。",
                locale: locale
            )
            return res
        } catch {
            if self.workspaceEpoch == epoch {
                adminErrorMessage = error.localizedDescription
            }
            return nil
        }
    }
}

private extension String {
    var nilIfEmpty: String? {
        let t = trimmingCharacters(in: .whitespacesAndNewlines)
        return t.isEmpty ? nil : t
    }
}
