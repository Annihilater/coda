import CryptoKit
import Darwin
import Foundation
import Network

private final class OnceResume<T: Sendable>: @unchecked Sendable {
    private let lock = NSLock()
    private var continuation: CheckedContinuation<T, Error>?

    init(_ continuation: CheckedContinuation<T, Error>) {
        self.continuation = continuation
    }

    func succeed(_ value: T) {
        lock.lock()
        let cont = continuation
        continuation = nil
        lock.unlock()
        cont?.resume(returning: value)
    }

    func fail(_ error: Error) {
        lock.lock()
        let cont = continuation
        continuation = nil
        lock.unlock()
        cont?.resume(throwing: error)
    }
}

public struct CodaRelayTicketResult: Sendable {
    public let ticket: String
    public let rawTCPHost: String?
    public let rawTCPPort: UInt16?
    public let wssURL: URL?
    public let relays: [CodaRelayNodeInfo]

    public init(
        ticket: String,
        rawTCPHost: String?,
        rawTCPPort: UInt16?,
        wssURL: URL?,
        relays: [CodaRelayNodeInfo] = []
    ) {
        self.ticket = ticket
        self.rawTCPHost = rawTCPHost
        self.rawTCPPort = rawTCPPort
        self.wssURL = wssURL
        self.relays = relays
    }
}

public struct CodaAsyncEvent: Sendable {
    public let op: String
    public let sourceDeviceId: UUID
    public let payloadData: Data

    public init(op: String, sourceDeviceId: UUID, payloadData: Data) {
        self.op = op
        self.sourceDeviceId = sourceDeviceId
        self.payloadData = payloadData
    }
}

public struct CodaTerminalScreenResult: Sendable {
    public let terminal: String
    public let text: String
    public let status: CodaHerdrAgentStatus?

    public init(terminal: String, text: String, status: CodaHerdrAgentStatus?) {
        self.terminal = terminal
        self.text = text
        self.status = status
    }
}

public struct CodaWorkspacesFetchResult: Sendable {
    public let machineName: String
    public let repos: [CodaRepoWorkspaceSummary]
    public let voiceConfig: CodaVoiceModelConfig?
    public let hostHealth: CodaHostHealthSnapshot?

    public init(
        machineName: String,
        repos: [CodaRepoWorkspaceSummary],
        voiceConfig: CodaVoiceModelConfig?,
        hostHealth: CodaHostHealthSnapshot? = nil
    ) {
        self.machineName = machineName
        self.repos = repos
        self.voiceConfig = voiceConfig
        self.hostHealth = hostHealth
    }
}

private struct DirectP2PPeerState {
    let connection: NWConnection
    var session: CodaE2EESession
    var routeMode: SmartPathRouteMode
    let generation: UInt64
    var readerTask: Task<Void, Never>?
}

private struct RelayPeerSessionState {
    var session: CodaE2EESession
    let sessionNonceB64: String
    let initTsMs: UInt64
    let signatureB64: String
    var ackedByPeer: Bool
}

private struct PendingRequest {
    let peerDeviceId: UUID
    let transportGen: UInt64
    let continuation: AsyncThrowingStream<Data, Error>.Continuation
    let timeoutTask: Task<Void, Never>
}

private struct SmartPathTarget {
    let targetDevice: CodaRemoteDevice
    let clientIdentity: CodaDeviceIdentity
    let clientDeviceId: UUID
    let udpProbeHost: String
    let udpProbePort: UInt16
}

/// Native Apple `actor` managing Multi-Tenant Auth, Device Pairing, SmartPath Auto-Negotiation,
/// and unified E2EE PeerMessage RPC across Direct P2P, Domestic Raw TCP, and CDN WSS Relays.
public actor CodaNetworkClient {
    public let serverBaseURL: URL
    private var bearerToken: String?
    private var wsTask: URLSessionWebSocketTask?
    private var wsGeneration: UInt64 = 0
    private var rawTCPConnection: NWConnection?
    private var rawTCPGeneration: UInt64 = 0
    private var relayReaderTask: Task<Void, Never>?
    private var relayKeepAliveTask: Task<Void, Never>?
    private var activeRelayRouteMode: SmartPathRouteMode?
    private var relayBoundPeerDeviceId: UUID?
    private var directP2PPeers: [UUID: DirectP2PPeerState] = [:]
    private var p2pPeerDeviceId: UUID?
    private var relayPeerSessions: [UUID: RelayPeerSessionState] = [:]
    private var connectedPeerDeviceId: UUID?
    private var pendingRequests: [String: PendingRequest] = [:]
    private var asyncEvents: [CodaAsyncEvent] = []
    private var asyncEventHandler: (@Sendable (CodaAsyncEvent) -> Void)?
    private var accountEventsWSTask: URLSessionWebSocketTask?
    private var accountEventsLoopTask: Task<Void, Never>?
    private var accountEventsGeneration: UInt64 = 0
    public private(set) var activeRouteMode: SmartPathRouteMode = .globalCDNWSS
    private var pathMonitor: NWPathMonitor?
    private var trackedSmartPathPeers: [UUID: SmartPathTarget] = [:]
    private var lanReprobeTasks: [UUID: Task<Void, Never>] = [:]
    private var inFlightDirectProbes: Set<UUID> = []
    private var lastOnLanInterface: Bool?
    private var smartPathPolicy = CodaSmartPathPolicy()
    private var currentGeneration: UInt64 = max(1, UInt64(Date().timeIntervalSince1970 * 1000))

    public init(serverBaseURL: URL) {
        self.serverBaseURL = serverBaseURL
    }

    public func setToken(_ token: String) {
        self.bearerToken = token
    }

    public func setAsyncEventHandler(_ handler: (@Sendable (CodaAsyncEvent) -> Void)?) {
        self.asyncEventHandler = handler
        if let handler, !asyncEvents.isEmpty {
            let buffered = asyncEvents
            asyncEvents.removeAll()
            for ev in buffered {
                handler(ev)
            }
        }
    }

    public func drainAsyncEvents() -> [CodaAsyncEvent] {
        let copy = asyncEvents
        asyncEvents.removeAll()
        return copy
    }

    public func disconnectAccountEventsWebSocket() {
        accountEventsGeneration &+= 1
        accountEventsLoopTask?.cancel()
        accountEventsLoopTask = nil
        accountEventsWSTask?.cancel(with: .normalClosure, reason: nil)
        accountEventsWSTask = nil
    }

    public func connectAccountEventsWebSocket(
        sessionToken: String,
        onEvent: @escaping @Sendable (CodaAccountLiveEvent) -> Void
    ) {
        self.bearerToken = sessionToken
        disconnectAccountEventsWebSocket()

        accountEventsGeneration &+= 1
        let loopGen = accountEventsGeneration
        guard var comps = URLComponents(
            url: serverBaseURL.appendingPathComponent("api/v1/events/ws"),
            resolvingAgainstBaseURL: false
        ) else { return }
        if comps.scheme == "https" {
            comps.scheme = "wss"
        } else if comps.scheme == "http" {
            comps.scheme = "ws"
        }
        guard let wsURL = comps.url else { return }

        accountEventsLoopTask = Task { [weak self] in
            var backoffNs: UInt64 = 300_000_000
            while !Task.isCancelled {
                guard let self else { return }
                let stillActive = await self.isAccountEventsGenCurrent(loopGen)
                if !stillActive { return }

                var req = URLRequest(url: wsURL)
                req.setValue("Bearer \(sessionToken)", forHTTPHeaderField: "Authorization")
                req.setValue("coda-auth, coda-bearer.\(sessionToken)", forHTTPHeaderField: "Sec-WebSocket-Protocol")
                let task = URLSession.shared.webSocketTask(with: req)
                await self.setAccountEventsTask(task, expectedGen: loopGen)
                task.resume()

                let pingTask = Task {
                    while !Task.isCancelled {
                        try? await Task.sleep(nanoseconds: 25_000_000_000)
                        if Task.isCancelled { break }
                        await withCheckedContinuation { (cont: CheckedContinuation<Void, Never>) in
                            task.sendPing { _ in cont.resume() }
                        }
                    }
                }
                defer { pingTask.cancel() }

                while !Task.isCancelled {
                    do {
                        let msg = try await task.receive()
                        backoffNs = 300_000_000
                        let msgData: Data
                        switch msg {
                        case .string(let s): msgData = Data(s.utf8)
                        case .data(let d): msgData = d
                        @unknown default: continue
                        }
                        if let decoded = try? JSONDecoder().decode(CodaAccountLiveEvent.self, from: msgData) {
                            onEvent(decoded)
                        }
                    } catch {
                        break
                    }
                }
                pingTask.cancel()
                task.cancel(with: .normalClosure, reason: nil)
                let stillActiveAfterDrop = await self.isAccountEventsGenCurrent(loopGen)
                if !stillActiveAfterDrop || Task.isCancelled { return }
                try? await Task.sleep(nanoseconds: backoffNs)
                backoffNs = min(backoffNs * 2, 5_000_000_000)
            }
        }
    }

    private func isAccountEventsGenCurrent(_ gen: UInt64) -> Bool {
        accountEventsGeneration == gen
    }

    private func setAccountEventsTask(_ task: URLSessionWebSocketTask, expectedGen: UInt64) {
        guard accountEventsGeneration == expectedGen else {
            task.cancel(with: .normalClosure, reason: nil)
            return
        }
        accountEventsWSTask?.cancel(with: .normalClosure, reason: nil)
        accountEventsWSTask = task
    }

    private func nextMonotonicGeneration() -> UInt64 {
        let nowMs = UInt64(Date().timeIntervalSince1970 * 1000)
        currentGeneration = max(currentGeneration &+ 1, nowMs)
        return currentGeneration
    }

    public func invalidatePeerSession(peerDeviceId: UUID) {
        relayPeerSessions.removeValue(forKey: peerDeviceId)
        trackedSmartPathPeers.removeValue(forKey: peerDeviceId)
        lanReprobeTasks.removeValue(forKey: peerDeviceId)?.cancel()
        inFlightDirectProbes.remove(peerDeviceId)
        smartPathPolicy.reset(peerId: peerDeviceId)
        if let peer = directP2PPeers.removeValue(forKey: peerDeviceId) {
            peer.readerTask?.cancel()
            peer.connection.cancel()
        }
        if p2pPeerDeviceId == peerDeviceId {
            p2pPeerDeviceId = nil
        }
        if connectedPeerDeviceId == peerDeviceId {
            connectedPeerDeviceId = nil
        }
        failPendingRequests(forPeer: peerDeviceId, error: URLError(.cancelled))
        if relayBoundPeerDeviceId == peerDeviceId {
            let activeGen = rawTCPConnection != nil ? rawTCPGeneration : wsGeneration
            relayBoundPeerDeviceId = nil
            handleRelayDisconnect(gen: activeGen)
        }
    }

    public func disconnectTransport() {
        disconnectAccountEventsWebSocket()
        pathMonitor?.cancel()
        pathMonitor = nil
        lastOnLanInterface = nil
        for (_, task) in lanReprobeTasks {
            task.cancel()
        }
        lanReprobeTasks.removeAll()
        trackedSmartPathPeers.removeAll()
        inFlightDirectProbes.removeAll()
        smartPathPolicy.reset()
        for (_, peer) in directP2PPeers {
            peer.readerTask?.cancel()
            peer.connection.cancel()
        }
        directP2PPeers.removeAll()
        p2pPeerDeviceId = nil
        relayKeepAliveTask?.cancel()
        relayKeepAliveTask = nil
        relayReaderTask?.cancel()
        relayReaderTask = nil
        rawTCPConnection?.cancel()
        rawTCPConnection = nil
        rawTCPGeneration &+= 1
        wsTask?.cancel(with: .normalClosure, reason: nil)
        wsTask = nil
        wsGeneration &+= 1
        activeRelayRouteMode = nil
        relayBoundPeerDeviceId = nil
        relayPeerSessions.removeAll()
        connectedPeerDeviceId = nil
        failAllPendingRequests(error: URLError(.cancelled))
    }

    // MARK: - Request ID Negotiation

    public func nextRequestID() -> String {
        let prefix = CodaE2EESession.hasNativeZstdFFI ? "coda" : "swift"
        return "\(prefix)-\(UUID().uuidString.lowercased())"
    }

    public func logout() async {
        let token = bearerToken
        bearerToken = nil
        disconnectTransport()
        guard let token, !token.isEmpty else { return }
        let url = serverBaseURL.appendingPathComponent("api/v1/auth/logout")
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        _ = try? await URLSession.shared.data(for: req)
    }

    // MARK: - Coordinator REST API

    public func login(emailOrHandle: String, password: String, clientLabel: String = "Apple-Native") async throws -> String {
        let url = serverBaseURL.appendingPathComponent("api/v1/auth/login")
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        let body: [String: String] = [
            "email_or_handle": emailOrHandle,
            "password": password,
            "client_label": clientLabel,
        ]
        req.httpBody = try JSONSerialization.data(withJSONObject: body)

        let (data, resp) = try await URLSession.shared.data(for: req)
        guard let http = resp as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
            throw CodaAPIError.fromResponse(data: data, response: resp, fallback: "Sign in failed")
        }
        let json = try JSONSerialization.jsonObject(with: data) as? [String: Any]
        guard let token = json?["token"] as? String else {
            throw CodaAPIError.invalidResponse
        }
        self.bearerToken = token
        return token
    }

    public func register(
        email: String,
        handle: String,
        displayName: String,
        password: String,
        inviteCode: String? = nil
    ) async throws -> String {
        let url = serverBaseURL.appendingPathComponent("api/v1/auth/register")
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        var body: [String: Any] = [
            "email": email,
            "handle": handle,
            "display_name": displayName,
            "password": password,
        ]
        if let inviteCode, !inviteCode.isEmpty {
            body["invite_code"] = inviteCode
        }
        req.httpBody = try JSONSerialization.data(withJSONObject: body)

        let (data, resp) = try await URLSession.shared.data(for: req)
        guard let http = resp as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
            throw CodaAPIError.fromResponse(data: data, response: resp, fallback: "Registration failed")
        }
        let json = try JSONSerialization.jsonObject(with: data) as? [String: Any]
        guard let token = json?["token"] as? String else {
            throw CodaAPIError.invalidResponse
        }
        self.bearerToken = token
        return token
    }

    public func approveDeviceAuth(userCode: String) async throws -> CodaRemoteDevice {
        let url = serverBaseURL.appendingPathComponent("api/v1/auth/device/approve")
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        if let token = bearerToken {
            req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        req.httpBody = try JSONSerialization.data(withJSONObject: [
            "user_code": userCode.trimmingCharacters(in: .whitespacesAndNewlines).uppercased()
        ])
        let (data, resp) = try await URLSession.shared.data(for: req)
        guard let http = resp as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
            throw CodaAPIError.fromResponse(data: data, response: resp, fallback: "Device approval failed")
        }
        return try JSONDecoder().decode(CodaRemoteDevice.self, from: data)
    }

    public func renameDevice(deviceId: UUID, name: String) async throws {
        let url = serverBaseURL.appendingPathComponent("api/v1/devices/\(deviceId.uuidString.lowercased())")
        var req = URLRequest(url: url)
        req.httpMethod = "PATCH"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        if let token = bearerToken {
            req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        req.httpBody = try JSONSerialization.data(withJSONObject: ["name": name])
        let (data, resp) = try await URLSession.shared.data(for: req)
        guard let http = resp as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
            throw CodaAPIError.fromResponse(data: data, response: resp, fallback: "Rename device failed")
        }
    }

    public func deleteDevice(deviceId: UUID) async throws {
        let url = serverBaseURL.appendingPathComponent("api/v1/devices/\(deviceId.uuidString.lowercased())")
        var req = URLRequest(url: url)
        req.httpMethod = "DELETE"
        if let token = bearerToken {
            req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        let (data, resp) = try await URLSession.shared.data(for: req)
        guard let http = resp as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
            throw CodaAPIError.fromResponse(data: data, response: resp, fallback: "Remove device failed")
        }
        invalidatePeerSession(peerDeviceId: deviceId)
    }

    public func pruneStaleDevices(
        maxIdleDays: Int = 0,
        excludeDeviceId: UUID? = nil
    ) async throws -> CodaPruneStaleDevicesResult {
        let url = serverBaseURL.appendingPathComponent("api/v1/devices/prune-stale")
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        if let token = bearerToken {
            req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        var body: [String: Any] = [
            "max_idle_days": max(0, maxIdleDays),
        ]
        if let excludeDeviceId {
            body["exclude_device_id"] = excludeDeviceId.uuidString.lowercased()
        }
        req.httpBody = try JSONSerialization.data(withJSONObject: body)
        let (data, resp) = try await URLSession.shared.data(for: req)
        guard let http = resp as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
            throw CodaAPIError.fromResponse(data: data, response: resp, fallback: "Prune stale web devices failed")
        }
        return try JSONDecoder().decode(CodaPruneStaleDevicesResult.self, from: data)
    }

    public func fetchNotificationSettings() async throws -> CodaNotificationWebhookSettings {
        let url = serverBaseURL.appendingPathComponent("api/v1/me/notifications")
        var req = URLRequest(url: url)
        req.httpMethod = "GET"
        if let token = bearerToken {
            req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        let (data, resp) = try await URLSession.shared.data(for: req)
        guard let http = resp as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
            throw CodaAPIError.fromResponse(data: data, response: resp, fallback: "Fetch notification settings failed")
        }
        return try JSONDecoder().decode(CodaNotificationWebhookSettings.self, from: data)
    }

    public func updateNotificationSettings(
        _ settings: CodaNotificationWebhookSettings
    ) async throws -> CodaNotificationWebhookSettings {
        let allowLoopback = serverBaseURL.scheme?.lowercased() == "http"
            && Self.isLoopbackHost(serverBaseURL.host ?? "")
        try settings.validateWebhookURLs(allowLoopbackHTTP: allowLoopback)
        let url = serverBaseURL.appendingPathComponent("api/v1/me/notifications")
        var req = URLRequest(url: url)
        req.httpMethod = "PUT"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        if let token = bearerToken {
            req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        req.httpBody = try JSONEncoder().encode(settings)
        let (data, resp) = try await URLSession.shared.data(for: req)
        guard let http = resp as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
            throw CodaAPIError.fromResponse(data: data, response: resp, fallback: "Save notification settings failed")
        }
        return try JSONDecoder().decode(CodaNotificationWebhookSettings.self, from: data)
    }

    public func testNotificationSettings() async throws -> CodaNotificationTestResult {
        let url = serverBaseURL.appendingPathComponent("api/v1/me/notifications/test")
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        if let token = bearerToken {
            req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        req.httpBody = Data("{}".utf8)
        let (data, resp) = try await URLSession.shared.data(for: req)
        guard let http = resp as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
            throw CodaAPIError.fromResponse(data: data, response: resp, fallback: "Test push failed")
        }
        return try JSONDecoder().decode(CodaNotificationTestResult.self, from: data)
    }

    public func changePassword(currentPassword: String, newPassword: String) async throws -> String? {
        let url = serverBaseURL.appendingPathComponent("api/v1/auth/password")
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        if let token = bearerToken {
            req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        req.httpBody = try JSONSerialization.data(withJSONObject: [
            "current_password": currentPassword,
            "new_password": newPassword,
        ])
        let (data, resp) = try await URLSession.shared.data(for: req)
        guard let http = resp as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
            throw CodaAPIError.fromResponse(data: data, response: resp, fallback: "Password update failed")
        }
        let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        if let newTok = json?["token"] as? String {
            self.bearerToken = newTok
            return newTok
        }
        return nil
    }

    public func deleteAccount(currentPassword: String) async throws {
        let url = serverBaseURL.appendingPathComponent("api/v1/auth/me")
        var req = URLRequest(url: url)
        req.httpMethod = "DELETE"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        if let token = bearerToken {
            req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        req.httpBody = try JSONSerialization.data(withJSONObject: [
            "password": currentPassword
        ])
        let (data, resp) = try await URLSession.shared.data(for: req)
        guard let http = resp as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
            throw CodaAPIError.fromResponse(data: data, response: resp, fallback: "Delete account failed")
        }
        bearerToken = nil
        disconnectTransport()
    }

    public func fetchAuthMe() async throws -> (handle: String, email: String) {
        let profile = try await fetchCurrentUserProfile()
        return (profile.handle, profile.email)
    }

    public func fetchCurrentUserProfile() async throws -> CodaAuthUserProfile {
        let url = serverBaseURL.appendingPathComponent("api/v1/auth/me")
        var req = URLRequest(url: url)
        req.httpMethod = "GET"
        if let token = bearerToken {
            req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        let (data, resp) = try await URLSession.shared.data(for: req)
        guard let http = resp as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
            throw CodaAPIError.fromResponse(data: data, response: resp, fallback: "Fetch account profile failed")
        }
        return try JSONDecoder().decode(CodaAuthUserProfile.self, from: data)
    }

    public func fetchPromptSnippets() async throws -> [CodaPromptSnippet] {
        let url = serverBaseURL.appendingPathComponent("api/v1/me/snippets")
        var req = URLRequest(url: url)
        req.httpMethod = "GET"
        if let token = bearerToken {
            req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        let (data, resp) = try await URLSession.shared.data(for: req)
        guard let http = resp as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
            throw CodaAPIError.fromResponse(data: data, response: resp, fallback: "Fetch prompt snippets failed")
        }
        if let envelope = try? JSONDecoder().decode(CodaPromptSnippetsEnvelope.self, from: data) {
            return CodaPromptSnippet.sanitizeList(envelope.snippets)
        }
        let direct = try JSONDecoder().decode([CodaPromptSnippet].self, from: data)
        return CodaPromptSnippet.sanitizeList(direct)
    }

    @discardableResult
    public func savePromptSnippets(_ snippets: [CodaPromptSnippet]) async throws -> [CodaPromptSnippet] {
        let clean = CodaPromptSnippet.sanitizeList(snippets)
        let url = serverBaseURL.appendingPathComponent("api/v1/me/snippets")
        var req = URLRequest(url: url)
        req.httpMethod = "PUT"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        if let token = bearerToken {
            req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        req.httpBody = try JSONEncoder().encode(CodaPromptSnippetsEnvelope(snippets: clean))
        let (data, resp) = try await URLSession.shared.data(for: req)
        guard let http = resp as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
            throw CodaAPIError.fromResponse(data: data, response: resp, fallback: "Save prompt snippets failed")
        }
        if let envelope = try? JSONDecoder().decode(CodaPromptSnippetsEnvelope.self, from: data) {
            return CodaPromptSnippet.sanitizeList(envelope.snippets)
        }
        if let direct = try? JSONDecoder().decode([CodaPromptSnippet].self, from: data) {
            return CodaPromptSnippet.sanitizeList(direct)
        }
        return clean
    }

    public func fetchAdminInvites() async throws -> [CodaInviteCodeItem] {
        let url = serverBaseURL.appendingPathComponent("api/v1/invites")
        var req = URLRequest(url: url)
        req.httpMethod = "GET"
        if let token = bearerToken {
            req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        let (data, resp) = try await URLSession.shared.data(for: req)
        guard let http = resp as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
            throw CodaAPIError.fromResponse(data: data, response: resp, fallback: "Fetch invite codes failed")
        }
        if let direct = try? JSONDecoder().decode([CodaInviteCodeItem].self, from: data) {
            return direct
        }
        struct InvitesEnvelope: Codable { let invites: [CodaInviteCodeItem] }
        return try JSONDecoder().decode(InvitesEnvelope.self, from: data).invites
    }

    public func createAdminInvite(
        note: String = "",
        maxUses: Int = 5,
        validDays: Int = 30
    ) async throws -> CodaInviteCodeItem {
        let url = serverBaseURL.appendingPathComponent("api/v1/invites")
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        if let token = bearerToken {
            req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        let body: [String: Any] = [
            "note": note.trimmingCharacters(in: .whitespacesAndNewlines),
            "max_uses": max(1, maxUses),
            "valid_days": max(1, validDays),
        ]
        req.httpBody = try JSONSerialization.data(withJSONObject: body)
        let (data, resp) = try await URLSession.shared.data(for: req)
        guard let http = resp as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
            throw CodaAPIError.fromResponse(data: data, response: resp, fallback: "Create invite code failed")
        }
        return try JSONDecoder().decode(CodaInviteCodeItem.self, from: data)
    }

    public func deleteAdminInvite(code: String) async throws {
        let cleanCode = code.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !cleanCode.isEmpty else { return }
        let url = serverBaseURL
            .appendingPathComponent("api/v1/invites")
            .appendingPathComponent(cleanCode)
        var req = URLRequest(url: url)
        req.httpMethod = "DELETE"
        if let token = bearerToken {
            req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        let (data, resp) = try await URLSession.shared.data(for: req)
        guard let http = resp as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
            throw CodaAPIError.fromResponse(data: data, response: resp, fallback: "Revoke invite code failed")
        }
    }

    public func fetchAdminUsers() async throws -> [CodaAdminUserSummary] {
        let url = serverBaseURL.appendingPathComponent("api/v1/admin/users")
        var req = URLRequest(url: url)
        req.httpMethod = "GET"
        if let token = bearerToken {
            req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        let (data, resp) = try await URLSession.shared.data(for: req)
        guard let http = resp as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
            throw CodaAPIError.fromResponse(data: data, response: resp, fallback: "Fetch admin members failed")
        }
        if let envelope = try? JSONDecoder().decode(CodaAdminUsersEnvelope.self, from: data) {
            return envelope.users
        }
        return try JSONDecoder().decode([CodaAdminUserSummary].self, from: data)
    }

    public func createAdminPasswordResetLink(userId: UUID) async throws -> CodaPasswordResetLinkResult {
        let url = serverBaseURL.appendingPathComponent("api/v1/admin/users/\(userId.uuidString.lowercased())/reset-link")
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        if let token = bearerToken {
            req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        req.httpBody = Data("{}".utf8)
        let (data, resp) = try await URLSession.shared.data(for: req)
        guard let http = resp as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
            throw CodaAPIError.fromResponse(data: data, response: resp, fallback: "Create password reset link failed")
        }
        return try JSONDecoder().decode(CodaPasswordResetLinkResult.self, from: data)
    }

    @discardableResult
    public func resetPasswordWithToken(
        token: String,
        newPassword: String
    ) async throws -> CodaPasswordResetConfirmResult {
        guard let cleanToken = extractResetTokenFromInput(token) else {
            throw CodaAPIError.httpError(
                statusCode: 400,
                message: "Paste a valid reset link or rst_... token."
            )
        }
        let pwdBytes = newPassword.utf8.count
        guard pwdBytes >= 8 && pwdBytes <= 256 else {
            throw CodaAPIError.httpError(
                statusCode: 400,
                message: "New password must be between 8 and 256 characters."
            )
        }
        let url = serverBaseURL.appendingPathComponent("api/v1/auth/reset-password")
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        let body: [String: String] = [
            "token": cleanToken,
            "new_password": newPassword,
        ]
        req.httpBody = try JSONSerialization.data(withJSONObject: body)
        let (data, resp) = try await URLSession.shared.data(for: req)
        guard let http = resp as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
            throw CodaAPIError.fromResponse(data: data, response: resp, fallback: "Password reset failed")
        }
        if let decoded = try? JSONDecoder().decode(CodaPasswordResetConfirmResult.self, from: data) {
            return decoded
        }
        return CodaPasswordResetConfirmResult(ok: true, handle: "")
    }

    public func registerClientDevice(
        identity: CodaDeviceIdentity,
        platform: String
    ) async throws -> CodaRemoteDevice {
        let url = serverBaseURL.appendingPathComponent("api/v1/devices/register-client")
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        if let token = bearerToken {
            req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        let body: [String: Any] = [
            "name": identity.name,
            "platform": platform,
            "ed25519_pubkey": identity.ed25519PublicB64,
            "x25519_pubkey": identity.x25519PublicB64,
            "endpoints": [],
        ]
        req.httpBody = try JSONSerialization.data(withJSONObject: body)
        let (data, resp) = try await URLSession.shared.data(for: req)
        guard let http = resp as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
            throw URLError(.userAuthenticationRequired)
        }
        return try JSONDecoder().decode(CodaRemoteDevice.self, from: data)
    }

    public func startDeviceAuth(
        deviceName: String,
        role: String = "client",
        platform: String = "ios",
        identity: CodaDeviceIdentity,
        endpoints: [CodaEndpointCandidate] = []
    ) async throws -> CodaDeviceAuthStart {
        let url = serverBaseURL.appendingPathComponent("api/v1/auth/device/start")
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        let epObjs = endpoints.map { ["kind": $0.kind, "addr": $0.addr] }
        let body: [String: Any] = [
            "device_name": deviceName,
            "role": role,
            "platform": platform,
            "ed25519_pubkey": identity.ed25519PublicB64,
            "x25519_pubkey": identity.x25519PublicB64,
            "endpoints": epObjs,
        ]
        req.httpBody = try JSONSerialization.data(withJSONObject: body)
        let (data, resp) = try await URLSession.shared.data(for: req)
        guard let http = resp as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
            throw URLError(.badServerResponse)
        }
        return try JSONDecoder().decode(CodaDeviceAuthStart.self, from: data)
    }

    public func pollDeviceAuth(deviceCode: String) async throws -> CodaDeviceAuthPollResult {
        let url = serverBaseURL.appendingPathComponent("api/v1/auth/device/poll")
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.httpBody = try JSONSerialization.data(withJSONObject: ["device_code": deviceCode])
        let (data, resp) = try await URLSession.shared.data(for: req)
        guard let http = resp as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
            throw URLError(.badServerResponse)
        }
        let decoded = try JSONDecoder().decode(CodaDeviceAuthPollResult.self, from: data)
        if decoded.state == "approved", let tok = decoded.sessionToken {
            self.bearerToken = tok
        }
        return decoded
    }

    public func mintRelayTicket(deviceId: UUID, peerDeviceId: UUID? = nil) async throws -> CodaRelayTicketResult {
        let url = serverBaseURL.appendingPathComponent("api/v1/relay/ticket")
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        if let token = bearerToken {
            req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        var body: [String: Any] = ["device_id": deviceId.uuidString.lowercased()]
        if let peerDeviceId {
            body["peer_device_id"] = peerDeviceId.uuidString.lowercased()
        }
        req.httpBody = try JSONSerialization.data(withJSONObject: body)
        let (data, resp) = try await URLSession.shared.data(for: req)
        guard let http = resp as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
            throw URLError(.userAuthenticationRequired)
        }
        struct TicketWireResp: Codable {
            let ticket: String
            let relays: [CodaRelayNodeInfo]
        }
        let wire = try JSONDecoder().decode(TicketWireResp.self, from: data)
        let isRemoteServer = serverBaseURL.scheme == "https" || !Self.isLoopbackHost(serverBaseURL.host ?? "")

        var rawTCPHost: String?
        var rawTCPPort: UInt16?
        var wssURL: URL?

        for relay in wire.relays {
            if relay.mode == "raw_tcp_no_sni",
               let parsed = Self.parseHostPort(relay.endpoint),
               !(isRemoteServer && Self.isLoopbackHost(parsed.0)) {
                if rawTCPHost == nil {
                    rawTCPHost = parsed.0
                    rawTCPPort = parsed.1
                }
            } else if relay.mode == "cdn_wss", let url = URL(string: relay.endpoint), wssURL == nil {
                wssURL = url
            }
        }
        return CodaRelayTicketResult(
            ticket: wire.ticket,
            rawTCPHost: rawTCPHost,
            rawTCPPort: rawTCPPort,
            wssURL: wssURL,
            relays: wire.relays
        )
    }

    private static func isLoopbackHost(_ host: String) -> Bool {
        let h = host.lowercased()
        return h == "127.0.0.1" || h == "localhost" || h == "::1" || h == "0.0.0.0"
    }

    public func createPairingCode(suggestedName: String?) async throws -> (code: String, oneLiner: String) {
        let url = serverBaseURL.appendingPathComponent("api/v1/devices/pair-token")
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        if let token = bearerToken {
            req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        req.httpBody = try JSONSerialization.data(withJSONObject: [
            "suggested_name": suggestedName as Any
        ])
        let (data, resp) = try await URLSession.shared.data(for: req)
        guard let http = resp as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
            throw URLError(.userAuthenticationRequired)
        }
        let json = try JSONSerialization.jsonObject(with: data) as? [String: Any]
        let code = json?["code"] as? String ?? "000-000"
        let cmd = json?["one_liner_command"] as? String ?? ""
        return (code, cmd)
    }

    private static func isAllowedRelayWSSURL(_ url: URL) -> Bool {
        let scheme = (url.scheme ?? "").lowercased()
        if scheme == "wss" { return true }
        if scheme == "ws", let host = url.host, isLoopbackHost(host) { return true }
        return false
    }

    private func startRelayKeepAliveLoop(forRawTCP conn: NWConnection, gen: UInt64) {
        relayKeepAliveTask?.cancel()
        relayKeepAliveTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 25_000_000_000)
                if Task.isCancelled { break }
                guard let self else { break }
                let stillActive = await self.isRawTCPGenCurrent(gen)
                if !stillActive { break }
                let ka: [String: Any] = [
                    "frame_type": "keep_alive",
                    "ts_ms": UInt64(Date().timeIntervalSince1970 * 1000),
                ]
                if let framed = try? Self.encodeLengthDelimitedJSON(ka) {
                    conn.send(content: framed, completion: .contentProcessed { _ in })
                }
            }
        }
    }

    private func startRelayKeepAliveLoop(forWSS task: URLSessionWebSocketTask, gen: UInt64) {
        relayKeepAliveTask?.cancel()
        relayKeepAliveTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 25_000_000_000)
                if Task.isCancelled { break }
                guard let self else { break }
                let stillActive = await self.isWSSGenCurrent(gen)
                if !stillActive { break }
                let ka: [String: Any] = [
                    "frame_type": "keep_alive",
                    "ts_ms": UInt64(Date().timeIntervalSince1970 * 1000),
                ]
                if let data = try? JSONSerialization.data(withJSONObject: ka),
                   let txt = String(data: data, encoding: .utf8) {
                    task.send(.string(txt)) { _ in }
                }
            }
        }
    }

    private func isRawTCPGenCurrent(_ gen: UInt64) -> Bool {
        rawTCPConnection != nil && rawTCPGeneration == gen
    }

    private func isWSSGenCurrent(_ gen: UInt64) -> Bool {
        wsTask != nil && wsGeneration == gen
    }

    // MARK: - Domestic No-ICP Raw TCP Relay (`Network.framework` NWConnection, TCP_NODELAY, Zero SNI)

    public func connectDomesticRawTCP(host: String, port: UInt16, relayTicket: String) {
        let tcpOptions = NWProtocolTCP.Options()
        tcpOptions.noDelay = true
        tcpOptions.enableKeepalive = true
        let params = NWParameters(tls: nil, tcp: tcpOptions)

        guard let nwPort = NWEndpoint.Port(rawValue: port) else { return }
        let conn = NWConnection(host: NWEndpoint.Host(host), port: nwPort, using: params)
        let gen = nextMonotonicGeneration()
        self.relayKeepAliveTask?.cancel()
        self.relayReaderTask?.cancel()
        self.rawTCPConnection?.cancel()
        self.rawTCPConnection = conn
        self.rawTCPGeneration = gen
        let oldWs = self.wsTask
        self.wsTask = nil
        self.wsGeneration &+= 1
        oldWs?.cancel(with: .normalClosure, reason: Data("switch-to-raw-tcp".utf8))
        self.activeRelayRouteMode = .domesticRawTCP

        conn.stateUpdateHandler = { state in
            if case .ready = state {
                let bindJSON: [String: Any] = [
                    "frame_type": "auth_bind",
                    "ticket": relayTicket,
                    "lane": "control_p0",
                    "generation": gen,
                ]
                if let framed = try? Self.encodeLengthDelimitedJSON(bindJSON) {
                    conn.send(content: framed, completion: .contentProcessed { _ in })
                }
            }
        }
        conn.start(queue: .global(qos: .userInteractive))
        self.relayReaderTask = Task { [weak self] in
            await Self.runRawTCPRelayReadLoop(client: self, conn: conn, gen: gen)
        }
        startRelayKeepAliveLoop(forRawTCP: conn, gen: gen)
    }

    private func connectDomesticRawTCPAsync(host: String, port: UInt16, relayTicket: String) async throws {
        let tcpOptions = NWProtocolTCP.Options()
        tcpOptions.noDelay = true
        tcpOptions.enableKeepalive = true
        let params = NWParameters(tls: nil, tcp: tcpOptions)

        guard let nwPort = NWEndpoint.Port(rawValue: port) else {
            throw URLError(.badURL)
        }
        let conn = NWConnection(host: NWEndpoint.Host(host), port: nwPort, using: params)
        let gen = nextMonotonicGeneration()

        let timeoutTask = Task {
            try await Task.sleep(nanoseconds: 5_000_000_000)
            conn.cancel()
        }
        defer { timeoutTask.cancel() }

        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Void, Error>) in
            let box = OnceResume<Void>(cont)
            conn.stateUpdateHandler = { state in
                switch state {
                case .ready: box.succeed(())
                case .failed(let err): box.fail(err)
                case .cancelled: box.fail(URLError(.timedOut))
                default: break
                }
            }
            conn.start(queue: .global(qos: .userInteractive))
        }

        let bindFramed = try Self.encodeLengthDelimitedJSON([
            "frame_type": "auth_bind",
            "ticket": relayTicket,
            "lane": "control_p0",
            "generation": gen,
        ])
        try await Self.sendNW(conn, data: bindFramed)
        let ack = try await Self.readLengthDelimitedFrame(conn)
        guard (ack["frame_type"] as? String) == "auth_ack",
              (ack["ok"] as? Bool) == true else {
            conn.cancel()
            throw URLError(.userAuthenticationRequired)
        }
        self.relayKeepAliveTask?.cancel()
        self.relayReaderTask?.cancel()
        self.rawTCPConnection?.cancel()
        self.rawTCPConnection = conn
        self.rawTCPGeneration = gen
        let oldWs = self.wsTask
        self.wsTask = nil
        self.wsGeneration &+= 1
        oldWs?.cancel(with: .normalClosure, reason: Data("switch-to-raw-tcp".utf8))
        self.activeRelayRouteMode = .domesticRawTCP
        self.relayReaderTask = Task { [weak self] in
            await Self.runRawTCPRelayReadLoop(client: self, conn: conn, gen: gen)
        }
        startRelayKeepAliveLoop(forRawTCP: conn, gen: gen)
    }

    // MARK: - Overseas CDN WebSocket Relay (With 75s Make-Before-Break Seamless Handoff)

    public func connectOverseasCDNWSS(wssURL: URL, relayTicket: String) async throws {
        guard Self.isAllowedRelayWSSURL(wssURL) else {
            throw CodaCryptoError.invalidHandshake("relay WebSocket must use wss:// (or loopback ws://)")
        }
        let nextGen = nextMonotonicGeneration()

        let task = URLSession.shared.webSocketTask(with: wssURL)
        task.maximumMessageSize = 8 * 1024 * 1024
        task.resume()
        let timeoutTask = Task {
            try await Task.sleep(nanoseconds: 5_000_000_000)
            task.cancel(with: .goingAway, reason: nil)
        }
        defer { timeoutTask.cancel() }

        let bindObj: [String: Any] = [
            "frame_type": "auth_bind",
            "ticket": relayTicket,
            "lane": "control_p0",
            "generation": nextGen,
        ]
        let bindStr = String(data: try JSONSerialization.data(withJSONObject: bindObj), encoding: .utf8) ?? "{}"
        try await task.send(.string(bindStr))

        let firstMsg = try await task.receive()
        let firstData: Data
        switch firstMsg {
        case .string(let s): firstData = Data(s.utf8)
        case .data(let d): firstData = d
        @unknown default:
            task.cancel(with: .protocolError, reason: nil)
            throw URLError(.badServerResponse)
        }
        guard let ackObj = try JSONSerialization.jsonObject(with: firstData) as? [String: Any],
              (ackObj["frame_type"] as? String) == "auth_ack",
              (ackObj["ok"] as? Bool) == true else {
            task.cancel(with: .policyViolation, reason: nil)
            throw URLError(.userAuthenticationRequired)
        }

        self.relayKeepAliveTask?.cancel()
        self.relayReaderTask?.cancel()
        self.rawTCPConnection?.cancel()
        self.rawTCPConnection = nil
        self.rawTCPGeneration &+= 1
        let oldTask = self.wsTask
        self.wsTask = task
        self.wsGeneration = nextGen
        oldTask?.cancel(with: .normalClosure, reason: Data("make-before-break".utf8))
        self.activeRelayRouteMode = .globalCDNWSS
        self.relayReaderTask = Task { [weak self] in
            await Self.runWSSRelayReadLoop(client: self, task: task, gen: nextGen)
        }
        startRelayKeepAliveLoop(forWSS: task, gen: nextGen)
    }

    // MARK: - Auto-SmartPath Negotiator (UDP LAN Discovery + TCP Direct P2P + Seamless Relay Failover)

    public func listDevices() async throws -> [CodaRemoteDevice] {
        let url = serverBaseURL.appendingPathComponent("api/v1/devices")
        var req = URLRequest(url: url)
        req.httpMethod = "GET"
        if let token = bearerToken {
            req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        let (data, resp) = try await URLSession.shared.data(for: req)
        guard let http = resp as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
            throw URLError(.userAuthenticationRequired)
        }
        return try JSONDecoder().decode([CodaRemoteDevice].self, from: data)
    }

    /// Zero-config Wi-Fi UDP discovery probe (`255.255.255.255:24444`).
    public static func probeLanUDP(
        targetDeviceId: UUID,
        host: String = "255.255.255.255",
        port: UInt16 = 24444,
        timeoutMs: Int = 180
    ) -> (host: String, port: UInt16)? {
        let fd = Darwin.socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP)
        guard fd >= 0 else { return nil }
        defer { Darwin.close(fd) }

        var broadcast: Int32 = 1
        _ = setsockopt(fd, SOL_SOCKET, SO_BROADCAST, &broadcast, socklen_t(MemoryLayout<Int32>.size))

        var tv = timeval(
            tv_sec: timeoutMs / 1000,
            tv_usec: __darwin_suseconds_t((timeoutMs % 1000) * 1000)
        )
        _ = setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, socklen_t(MemoryLayout<timeval>.size))

        var bindAddr = sockaddr_in()
        bindAddr.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
        bindAddr.sin_family = sa_family_t(AF_INET)
        bindAddr.sin_port = 0
        bindAddr.sin_addr = in_addr(s_addr: INADDR_ANY)
        let bindOK = withUnsafePointer(to: &bindAddr) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                Darwin.bind(fd, $0, socklen_t(MemoryLayout<sockaddr_in>.size))
            }
        }
        guard bindOK == 0 else { return nil }

        let targetLower = targetDeviceId.uuidString.lowercased()
        let probeObj: [String: Any] = [
            "coda_lan_probe": 1,
            "target_device_id": targetLower,
        ]
        guard let probeData = try? JSONSerialization.data(withJSONObject: probeObj) else {
            return nil
        }

        func sendProbe(to dstHost: String) {
            var dst = sockaddr_in()
            dst.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
            dst.sin_family = sa_family_t(AF_INET)
            dst.sin_port = port.bigEndian
            guard dstHost.withCString({ inet_pton(AF_INET, $0, &dst.sin_addr) }) == 1 else {
                return
            }
            _ = probeData.withUnsafeBytes { rawBuf in
                withUnsafePointer(to: &dst) { dstPtr in
                    dstPtr.withMemoryRebound(to: sockaddr.self, capacity: 1) { saPtr in
                        Darwin.sendto(
                            fd,
                            rawBuf.baseAddress,
                            probeData.count,
                            0,
                            saPtr,
                            socklen_t(MemoryLayout<sockaddr_in>.size)
                        )
                    }
                }
            }
        }

        sendProbe(to: host)
        if host == "255.255.255.255" {
            sendProbe(to: "127.0.0.1")
        }

        var recvBuf = [UInt8](repeating: 0, count: 512)
        var fromAddr = sockaddr_in()
        var fromLen = socklen_t(MemoryLayout<sockaddr_in>.size)

        while true {
            let n = recvBuf.withUnsafeMutableBytes { bufPtr in
                withUnsafeMutablePointer(to: &fromAddr) { addrPtr in
                    addrPtr.withMemoryRebound(to: sockaddr.self, capacity: 1) { saPtr in
                        Darwin.recvfrom(fd, bufPtr.baseAddress, 512, 0, saPtr, &fromLen)
                    }
                }
            }
            if n <= 0 {
                return nil
            }
            let data = Data(recvBuf.prefix(Int(n)))
            guard let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                  (json["coda_lan_here"] as? Int) == 1,
                  let devStr = json["device_id"] as? String,
                  devStr.lowercased() == targetLower,
                  let tcpPortInt = json["tcp_port"] as? Int,
                  tcpPortInt > 0 && tcpPortInt <= 65535
            else {
                continue
            }

            var ipC = [CChar](repeating: 0, count: Int(INET_ADDRSTRLEN))
            var addrCopy = fromAddr.sin_addr
            if inet_ntop(AF_INET, &addrCopy, &ipC, socklen_t(INET_ADDRSTRLEN)) != nil {
                let senderIP = String(decoding: ipC.prefix(while: { $0 != 0 }).map { UInt8(bitPattern: $0) }, as: UTF8.self)
                return (senderIP, UInt16(tcpPortInt))
            }
        }
    }

    private static func encodeLengthDelimitedJSON(_ obj: [String: Any]) throws -> Data {
        let payload = try JSONSerialization.data(withJSONObject: obj)
        var framed = Data(capacity: 4 + payload.count)
        var lenBE = UInt32(payload.count).bigEndian
        withUnsafeBytes(of: &lenBE) { framed.append(contentsOf: $0) }
        framed.append(payload)
        return framed
    }

    private static func sendNW(_ conn: NWConnection, data: Data) async throws {
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Void, Error>) in
            let box = OnceResume<Void>(cont)
            conn.send(content: data, completion: .contentProcessed { err in
                if let err {
                    box.fail(err)
                } else {
                    box.succeed(())
                }
            })
        }
    }

    private static func readExact(_ conn: NWConnection, length: Int) async throws -> Data {
        return try await withCheckedThrowingContinuation { cont in
            let box = OnceResume<Data>(cont)
            conn.receive(minimumIncompleteLength: length, maximumLength: length) { data, _, _, err in
                if let err {
                    box.fail(err)
                } else if let data, data.count == length {
                    box.succeed(data)
                } else {
                    box.fail(URLError(.cannotParseResponse))
                }
            }
        }
    }

    private static func readLengthDelimitedFrameData(_ conn: NWConnection) async throws -> Data {
        let hdr = try await readExact(conn, length: 4)
        let len = hdr.withUnsafeBytes { raw -> UInt32 in
            let v = raw.loadUnaligned(as: UInt32.self)
            return UInt32(bigEndian: v)
        }
        guard len > 0 && len <= 8 * 1024 * 1024 else {
            throw URLError(.badServerResponse)
        }
        return try await readExact(conn, length: Int(len))
    }

    private static func readLengthDelimitedFrame(_ conn: NWConnection) async throws -> [String: Any] {
        let body = try await readLengthDelimitedFrameData(conn)
        guard let obj = try JSONSerialization.jsonObject(with: body) as? [String: Any] else {
            throw URLError(.cannotParseResponse)
        }
        return obj
    }

    public func performDirectP2PHandshake(
        host: String,
        port: UInt16,
        clientIdentity: CodaDeviceIdentity,
        clientDeviceId: UUID,
        targetDeviceId: UUID,
        expectedDaemonEd25519Pubkey: String,
        expectedDaemonX25519Pubkey: String? = nil,
        timeoutMs: UInt64 = 350
    ) async throws -> SmartPathRouteMode {
        // Fail closed: Direct P2P requires a pinned Ed25519 public key from the coordinator directory.
        guard !expectedDaemonEd25519Pubkey.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            throw CodaCryptoError.invalidHandshake("missing pinned daemon ed25519_pubkey")
        }
        if let expectedX = expectedDaemonX25519Pubkey,
           expectedX.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            throw CodaCryptoError.invalidHandshake("empty pinned daemon x25519_pubkey")
        }

        let tcpOptions = NWProtocolTCP.Options()
        tcpOptions.noDelay = true
        tcpOptions.enableKeepalive = true
        let params = NWParameters(tls: nil, tcp: tcpOptions)

        guard let nwPort = NWEndpoint.Port(rawValue: port) else {
            throw URLError(.badURL)
        }
        let conn = NWConnection(host: NWEndpoint.Host(host), port: nwPort, using: params)

        let ephPrivate = Curve25519.KeyAgreement.PrivateKey()
        let ephPubB64 = ephPrivate.publicKey.rawRepresentation.base64EncodedString()
        let tsMs = UInt64(Date().timeIntervalSince1970 * 1000)
        let clientIdLower = clientDeviceId.uuidString.lowercased()
        let targetIdLower = targetDeviceId.uuidString.lowercased()
        let supportsZstd = CodaE2EESession.hasNativeZstdFFI
        let signedMsg = "p2p-hello-v2:\(clientIdLower):\(targetIdLower):\(clientIdentity.x25519PublicB64):\(ephPubB64):\(tsMs):zstd=\(supportsZstd ? 1 : 0)"
        let sigB64 = try clientIdentity.sign(message: Data(signedMsg.utf8))

        let helloFramed = try Self.encodeLengthDelimitedJSON([
            "frame_type": "direct_p2p_hello",
            "client_device_id": clientIdLower,
            "target_device_id": targetIdLower,
            "client_ed25519_pub_b64": clientIdentity.ed25519PublicB64,
            "client_x25519_pub_b64": clientIdentity.x25519PublicB64,
            "client_ephemeral_x25519_pub_b64": ephPubB64,
            "supports_zstd": supportsZstd,
            "timestamp_ms": tsMs,
            "signature_b64": sigB64,
        ])

        let timeoutTask = Task {
            try await Task.sleep(nanoseconds: timeoutMs * 1_000_000)
            conn.cancel()
        }
        defer { timeoutTask.cancel() }

        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Void, Error>) in
            let box = OnceResume<Void>(cont)
            conn.stateUpdateHandler = { state in
                switch state {
                case .ready: box.succeed(())
                case .failed(let err): box.fail(err)
                case .cancelled: box.fail(URLError(.timedOut))
                default: break
                }
            }
            conn.start(queue: .global(qos: .userInteractive))
        }

        try await Self.sendNW(conn, data: helloFramed)

        let ack = try await Self.readLengthDelimitedFrame(conn)
        guard (ack["frame_type"] as? String) == "direct_p2p_ack",
              (ack["ok"] as? Bool) == true,
              let daemonPubB64 = ack["daemon_x25519_pub_b64"] as? String,
              let ackDaemonIdStr = ack["daemon_device_id"] as? String,
              ackDaemonIdStr.lowercased() == targetIdLower,
              let daemonEdPubB64 = ack["daemon_ed25519_pub_b64"] as? String,
              let daemonEphPubB64 = ack["daemon_ephemeral_x25519_pub_b64"] as? String,
              !daemonEphPubB64.isEmpty,
              let ackSigB64 = ack["signature_b64"] as? String,
              let ackTsNum = ack["timestamp_ms"] as? NSNumber
        else {
            conn.cancel()
            throw CodaCryptoError.invalidHandshake("malformed direct_p2p_ack")
        }

        let ackTsMs = ackTsNum.uint64Value
        let nowMs = UInt64(Date().timeIntervalSince1970 * 1000)
        let skewMs = ackTsMs >= nowMs ? (ackTsMs - nowMs) : (nowMs - ackTsMs)
        guard skewMs <= 120_000 else {
            conn.cancel()
            throw CodaCryptoError.invalidHandshake("daemon timestamp skew \(skewMs)ms exceeds 120s")
        }
        guard daemonEdPubB64 == expectedDaemonEd25519Pubkey else {
            conn.cancel()
            throw CodaCryptoError.invalidHandshake("daemon ed25519_pubkey mismatch")
        }
        if let expectedX = expectedDaemonX25519Pubkey {
            guard daemonPubB64 == expectedX else {
                conn.cancel()
                throw CodaCryptoError.invalidHandshake("daemon x25519_pubkey mismatch")
            }
        }
        let expectedAckMsg = "p2p-ack-v2:\(clientIdLower):\(targetIdLower):\(ephPubB64):\(daemonEphPubB64):\(ackTsMs)"
        guard CodaDeviceIdentity.verifySignature(
            ed25519PublicB64: daemonEdPubB64,
            message: Data(expectedAckMsg.utf8),
            signatureB64: ackSigB64
        ) else {
            conn.cancel()
            throw CodaCryptoError.invalidHandshake("invalid direct_p2p_ack Ed25519 signature")
        }

        let pathKind = (ack["path_kind"] as? String) ?? "lan_ipv4"
        let routeMode: SmartPathRouteMode = (pathKind == "cn_public_ipv6") ? .cnIPv6P2P : .lanDirect
        let sess = try CodaE2EESession(
            directP2Pv2Identity: clientIdentity,
            peerStaticX25519PublicB64: daemonPubB64,
            myEphemeralPrivateKey: ephPrivate,
            peerEphemeralX25519PublicB64: daemonEphPubB64,
            clientDeviceId: clientDeviceId,
            daemonDeviceId: targetDeviceId,
            isInitiator: true
        )
        let gen = nextMonotonicGeneration()
        if let oldPeer = self.directP2PPeers[targetDeviceId] {
            oldPeer.readerTask?.cancel()
            oldPeer.connection.cancel()
            failPendingRequests(forTransportGen: oldPeer.generation, error: URLError(.cancelled))
        }
        let readerTask = Task { [weak self] in
            await Self.runDirectP2PReadLoop(client: self, conn: conn, targetDeviceId: targetDeviceId, gen: gen)
        }
        self.directP2PPeers[targetDeviceId] = DirectP2PPeerState(
            connection: conn,
            session: sess,
            routeMode: routeMode,
            generation: gen,
            readerTask: readerTask
        )
        self.p2pPeerDeviceId = targetDeviceId
        self.connectedPeerDeviceId = targetDeviceId
        self.activeRouteMode = routeMode
        return routeMode
    }

    private static func parseHostPort(_ addr: String) -> (String, UInt16)? {
        if addr.hasPrefix("[") {
            guard let closeIdx = addr.firstIndex(of: "]") else { return nil }
            let host = String(addr[addr.index(after: addr.startIndex)..<closeIdx])
            let rest = addr[addr.index(after: closeIdx)...]
            guard rest.hasPrefix(":"), let p = UInt16(rest.dropFirst()) else { return nil }
            return (host, p)
        }
        guard let colonIdx = addr.lastIndex(of: ":") else { return nil }
        let host = String(addr[..<colonIdx])
        guard let p = UInt16(addr[addr.index(after: colonIdx)...]) else { return nil }
        return (host, p)
    }

    private static func relaySessionSigningString(
        clientDeviceId: UUID,
        daemonDeviceId: UUID,
        senderX25519PubB64: String,
        sessionNonceB64: String,
        initTsMs: UInt64,
        supportsZstd: Bool = CodaE2EESession.hasNativeZstdFFI
    ) -> String {
        "relay-sess-v2:\(clientDeviceId.uuidString.lowercased()):\(daemonDeviceId.uuidString.lowercased()):\(senderX25519PubB64):\(sessionNonceB64):\(initTsMs):zstd=\(supportsZstd ? 1 : 0)"
    }

    private func resetRelayPeerSession(
        for targetDevice: CodaRemoteDevice,
        clientIdentity: CodaDeviceIdentity,
        clientDeviceId: UUID
    ) throws -> RelayPeerSessionState {
        let sessionNonceB64 = CodaE2EESession.randomSessionNonceB64()
        let session = try CodaE2EESession(
            relayV2Identity: clientIdentity,
            peerStaticX25519PublicB64: targetDevice.x25519Pubkey,
            sessionNonceB64: sessionNonceB64,
            clientDeviceId: clientDeviceId,
            daemonDeviceId: targetDevice.id,
            isInitiator: true
        )
        let nowMs = UInt64(Date().timeIntervalSince1970 * 1000)
        let sigMsg = Self.relaySessionSigningString(
            clientDeviceId: clientDeviceId,
            daemonDeviceId: targetDevice.id,
            senderX25519PubB64: clientIdentity.x25519PublicB64,
            sessionNonceB64: sessionNonceB64,
            initTsMs: nowMs
        )
        let sig = try clientIdentity.sign(message: Data(sigMsg.utf8))
        let state = RelayPeerSessionState(
            session: session,
            sessionNonceB64: sessionNonceB64,
            initTsMs: nowMs,
            signatureB64: sig,
            ackedByPeer: false
        )
        self.relayPeerSessions[targetDevice.id] = state
        return state
    }

    private func ensureRelayPeerSession(
        for targetDevice: CodaRemoteDevice,
        clientIdentity: CodaDeviceIdentity,
        clientDeviceId: UUID
    ) throws -> RelayPeerSessionState {
        if var existing = relayPeerSessions[targetDevice.id] {
            let nowMs = UInt64(Date().timeIntervalSince1970 * 1000)
            let ageMs = nowMs >= existing.initTsMs ? (nowMs - existing.initTsMs) : (existing.initTsMs - nowMs)
            if !existing.ackedByPeer && ageMs > 60_000 {
                let sigMsg = Self.relaySessionSigningString(
                    clientDeviceId: clientDeviceId,
                    daemonDeviceId: targetDevice.id,
                    senderX25519PubB64: clientIdentity.x25519PublicB64,
                    sessionNonceB64: existing.sessionNonceB64,
                    initTsMs: nowMs
                )
                let sig = try clientIdentity.sign(message: Data(sigMsg.utf8))
                existing = RelayPeerSessionState(
                    session: existing.session,
                    sessionNonceB64: existing.sessionNonceB64,
                    initTsMs: nowMs,
                    signatureB64: sig,
                    ackedByPeer: false
                )
                relayPeerSessions[targetDevice.id] = existing
            }
            return existing
        }
        return try resetRelayPeerSession(
            for: targetDevice,
            clientIdentity: clientIdentity,
            clientDeviceId: clientDeviceId
        )
    }

    public func connectAutoSmartPath(
        targetDevice: CodaRemoteDevice,
        clientIdentity: CodaDeviceIdentity,
        clientDeviceId: UUID,
        udpProbeHost: String = "255.255.255.255",
        udpProbePort: UInt16 = 24444,
        relayRawTCP: (host: String, port: UInt16)? = nil,
        relayWSS: URL? = nil,
        relayTicket: String? = nil,
        relayCandidates: [CodaRelayNodeInfo] = [],
        relayTicketBoundPeerId: UUID? = nil
    ) async throws -> SmartPathRouteMode {
        var candidates: [(host: String, port: UInt16)] = []
        let targetId = targetDevice.id

        let udpFound = await Task.detached {
            Self.probeLanUDP(targetDeviceId: targetId, host: udpProbeHost, port: udpProbePort)
        }.value
        if let found = udpFound {
            candidates.append(found)
        }

        for ep in targetDevice.endpoints {
            if ep.kind == "lan_ipv4" || ep.kind == "cn_public_ipv6",
               let parsed = Self.parseHostPort(ep.addr),
               !candidates.contains(where: { $0.host == parsed.0 && $0.port == parsed.1 }) {
                candidates.append(parsed)
            }
        }

        for cand in candidates {
            if let route = try? await performDirectP2PHandshake(
                host: cand.host,
                port: cand.port,
                clientIdentity: clientIdentity,
                clientDeviceId: clientDeviceId,
                targetDeviceId: targetId,
                expectedDaemonEd25519Pubkey: targetDevice.ed25519Pubkey,
                expectedDaemonX25519Pubkey: targetDevice.x25519Pubkey
            ) {
                startWiFiPathMonitor(
                    targetDevice: targetDevice,
                    clientIdentity: clientIdentity,
                    clientDeviceId: clientDeviceId,
                    udpProbeHost: udpProbeHost,
                    udpProbePort: udpProbePort
                )
                return route
            }
        }

        // Seamless Relay fallback with Relay v2 signed session establishment per peer daemon
        _ = try ensureRelayPeerSession(
            for: targetDevice,
            clientIdentity: clientIdentity,
            clientDeviceId: clientDeviceId
        )
        let canReuseSharedRelay = (self.relayBoundPeerDeviceId == nil || self.relayBoundPeerDeviceId == targetId)
        if canReuseSharedRelay,
           let relayMode = self.activeRelayRouteMode,
           (self.wsTask != nil || self.rawTCPConnection != nil) {
            self.connectedPeerDeviceId = targetId
            self.activeRouteMode = relayMode
            startWiFiPathMonitor(
                targetDevice: targetDevice,
                clientIdentity: clientIdentity,
                clientDeviceId: clientDeviceId,
                udpProbeHost: udpProbeHost,
                udpProbePort: udpProbePort
            )
            return relayMode
        }

        guard let ticket = relayTicket, !ticket.isEmpty else {
            throw URLError(.cannotConnectToHost)
        }

        let isRemoteServer = serverBaseURL.scheme == "https" || !Self.isLoopbackHost(serverBaseURL.host ?? "")
        enum RelayAttempt {
            case rawTCP(String, UInt16)
            case wss(URL)
        }
        var attempts: [RelayAttempt] = []
        if !relayCandidates.isEmpty {
            let sorted = relayCandidates.sorted { $0.priority < $1.priority }
            for node in sorted where node.mode == "raw_tcp_no_sni" {
                if let parsed = Self.parseHostPort(node.endpoint), !(isRemoteServer && Self.isLoopbackHost(parsed.0)) {
                    attempts.append(.rawTCP(parsed.0, parsed.1))
                }
            }
            for node in sorted where node.mode == "cdn_wss" {
                if let url = URL(string: node.endpoint) {
                    attempts.append(.wss(url))
                }
            }
            for node in sorted where node.mode == "raw_tcp_no_sni" {
                if let parsed = Self.parseHostPort(node.endpoint), isRemoteServer && Self.isLoopbackHost(parsed.0) {
                    attempts.append(.rawTCP(parsed.0, parsed.1))
                }
            }
        } else {
            let hasReachableRawTCP = relayRawTCP.map { !(isRemoteServer && Self.isLoopbackHost($0.host)) } ?? false
            if hasReachableRawTCP, let raw = relayRawTCP {
                attempts.append(.rawTCP(raw.host, raw.port))
            }
            if let wss = relayWSS {
                attempts.append(.wss(wss))
            }
            if !hasReachableRawTCP, let raw = relayRawTCP {
                attempts.append(.rawTCP(raw.host, raw.port))
            }
        }

        var connected = false
        var lastRelayError: Error?
        for attempt in attempts {
            do {
                switch attempt {
                case .rawTCP(let h, let p):
                    try await connectDomesticRawTCPAsync(host: h, port: p, relayTicket: ticket)
                    self.activeRouteMode = .domesticRawTCP
                case .wss(let url):
                    try await connectOverseasCDNWSS(wssURL: url, relayTicket: ticket)
                    self.activeRouteMode = .globalCDNWSS
                }
                self.relayBoundPeerDeviceId = relayTicketBoundPeerId
                connected = true
                break
            } catch {
                lastRelayError = error
            }
        }
        guard connected else {
            throw lastRelayError ?? URLError(.cannotConnectToHost)
        }

        _ = try resetRelayPeerSession(
            for: targetDevice,
            clientIdentity: clientIdentity,
            clientDeviceId: clientDeviceId
        )
        self.connectedPeerDeviceId = targetId
        startWiFiPathMonitor(
            targetDevice: targetDevice,
            clientIdentity: clientIdentity,
            clientDeviceId: clientDeviceId,
            udpProbeHost: udpProbeHost,
            udpProbePort: udpProbePort
        )
        return self.activeRouteMode
    }

    /// High-level SmartPath helper that mints an unpinned user-scoped relay ticket (`peerDeviceId: nil`)
    /// so a single shared Relay WebSocket / Raw TCP transport routes seamlessly to ALL paired hosts (`Host A..N`).
    public func connectBestAvailable(
        sessionToken: String,
        targetDevice: CodaRemoteDevice,
        clientIdentity: CodaDeviceIdentity
    ) async throws -> SmartPathRouteMode {
        self.bearerToken = sessionToken
        guard let clientDevId = clientIdentity.deviceId else {
            throw URLError(.userAuthenticationRequired)
        }
        if let peerState = directP2PPeers[targetDevice.id] {
            if let existingTarget = trackedSmartPathPeers[targetDevice.id] {
                trackedSmartPathPeers[targetDevice.id] = SmartPathTarget(
                    targetDevice: targetDevice,
                    clientIdentity: clientIdentity,
                    clientDeviceId: clientDevId,
                    udpProbeHost: existingTarget.udpProbeHost,
                    udpProbePort: existingTarget.udpProbePort
                )
            }
            self.activeRouteMode = peerState.routeMode
            return peerState.routeMode
        }
        let hasReusableSharedRelay = (wsTask != nil || rawTCPConnection != nil)
            && (relayBoundPeerDeviceId == nil || relayBoundPeerDeviceId == targetDevice.id)
        // Only reuse the shared Relay shortcut when `targetDevice` has already been probed and
        // enrolled in the SmartPath background reprobe loop; brand-new LAN peers must probe Direct first.
        if hasReusableSharedRelay, let existingTarget = trackedSmartPathPeers[targetDevice.id] {
            trackedSmartPathPeers[targetDevice.id] = SmartPathTarget(
                targetDevice: targetDevice,
                clientIdentity: clientIdentity,
                clientDeviceId: clientDevId,
                udpProbeHost: existingTarget.udpProbeHost,
                udpProbePort: existingTarget.udpProbePort
            )
            _ = try ensureRelayPeerSession(
                for: targetDevice,
                clientIdentity: clientIdentity,
                clientDeviceId: clientDevId
            )
            let relayMode = activeRelayRouteMode ?? activeRouteMode
            self.connectedPeerDeviceId = targetDevice.id
            self.activeRouteMode = relayMode
            return relayMode
        }
        if hasReusableSharedRelay, trackedSmartPathPeers[targetDevice.id] == nil {
            if let fastRoute = try? await connectAutoSmartPath(
                targetDevice: targetDevice,
                clientIdentity: clientIdentity,
                clientDeviceId: clientDevId,
                relayRawTCP: nil,
                relayWSS: nil,
                relayTicket: nil,
                relayCandidates: [],
                relayTicketBoundPeerId: nil
            ) {
                return fastRoute
            }
        }

        let ticketResp = try? await mintRelayTicket(deviceId: clientDevId, peerDeviceId: nil)
        let rawTuple = ticketResp.flatMap { res -> (host: String, port: UInt16)? in
            guard let h = res.rawTCPHost, let p = res.rawTCPPort else { return nil }
            return (h, p)
        }
        return try await connectAutoSmartPath(
            targetDevice: targetDevice,
            clientIdentity: clientIdentity,
            clientDeviceId: clientDevId,
            relayRawTCP: rawTuple,
            relayWSS: ticketResp?.wssURL,
            relayTicket: ticketResp?.ticket,
            relayCandidates: ticketResp?.relays ?? [],
            relayTicketBoundPeerId: nil
        )
    }

    private func emitRouteChangedEvent(peerDeviceId: UUID, routeMode: SmartPathRouteMode, isDirect: Bool) {
        let dict: [String: Any] = [
            "route_mode": routeMode.rawValue,
            "is_direct": isDirect,
        ]
        let payload = (try? JSONSerialization.data(withJSONObject: dict)) ?? Data()
        enqueueAsyncEvent(CodaAsyncEvent(op: "route_changed", sourceDeviceId: peerDeviceId, payloadData: payload))
    }

    /// Manually resets Smart-Path backoff and probes Direct LAN/IPv6 P2P for `targetDevice` without dropping Relay.
    public func probeDirectLanNow(
        for targetDevice: CodaRemoteDevice,
        clientIdentity: CodaDeviceIdentity,
        clientDeviceId: UUID,
        udpProbeHost: String = "255.255.255.255",
        udpProbePort: UInt16 = 24444
    ) async -> SmartPathRouteMode? {
        smartPathPolicy.reset(peerId: targetDevice.id)
        return await reprobeDirectIfNeeded(
            targetDevice: targetDevice,
            clientIdentity: clientIdentity,
            clientDeviceId: clientDeviceId,
            udpProbeHost: udpProbeHost,
            udpProbePort: udpProbePort,
            force: true
        )
    }

    private func startWiFiPathMonitor(
        targetDevice: CodaRemoteDevice,
        clientIdentity: CodaDeviceIdentity,
        clientDeviceId: UUID,
        udpProbeHost: String,
        udpProbePort: UInt16
    ) {
        let peerId = targetDevice.id
        trackedSmartPathPeers[peerId] = SmartPathTarget(
            targetDevice: targetDevice,
            clientIdentity: clientIdentity,
            clientDeviceId: clientDeviceId,
            udpProbeHost: udpProbeHost,
            udpProbePort: udpProbePort
        )

        if pathMonitor == nil {
            let monitor = NWPathMonitor()
            self.pathMonitor = monitor
            monitor.pathUpdateHandler = { [weak self] path in
                let onLanInterface = path.usesInterfaceType(.wifi) || path.usesInterfaceType(.wiredEthernet)
                Task { [weak self] in
                    await self?.handleNetworkPathTransition(onLanInterface: onLanInterface)
                }
            }
            monitor.start(queue: .global(qos: .utility))
        }

        // Per-peer background periodic LAN hot-upgrade loop while on Smart Relay (exponential backoff 15s -> 120s).
        if lanReprobeTasks[peerId] == nil {
            lanReprobeTasks[peerId] = Task { [weak self] in
                while !Task.isCancelled {
                    guard let self else { return }
                    let delaySec = await self.nextSmartPathProbeDelay(for: peerId)
                    let delayNs = UInt64(max(1.0, delaySec) * 1_000_000_000)
                    try? await Task.sleep(nanoseconds: delayNs)
                    if Task.isCancelled { return }
                    guard let target = await self.smartPathTarget(for: peerId) else { return }
                    let alreadyDirect = await self.isDirectPeerActive(peerId)
                    if alreadyDirect {
                        continue
                    }
                    _ = await self.reprobeDirectIfNeeded(
                        targetDevice: target.targetDevice,
                        clientIdentity: target.clientIdentity,
                        clientDeviceId: target.clientDeviceId,
                        udpProbeHost: target.udpProbeHost,
                        udpProbePort: target.udpProbePort,
                        force: false
                    )
                }
            }
        }
    }

    private func smartPathTarget(for peerId: UUID) -> SmartPathTarget? {
        trackedSmartPathPeers[peerId]
    }

    private func nextSmartPathProbeDelay(for peerId: UUID) -> TimeInterval {
        smartPathPolicy.nextDelay(for: peerId)
    }

    private func isDirectPeerActive(_ peerId: UUID) -> Bool {
        directP2PPeers[peerId] != nil
    }

    private func handleNetworkPathTransition(onLanInterface: Bool) async {
        let previous = lastOnLanInterface
        lastOnLanInterface = onLanInterface
        if onLanInterface {
            // Only reset backoff & reprobe on a genuine `false -> true` Wi-Fi/Ethernet transition;
            // ignore duplicate `true -> true` NWPathMonitor callbacks so exponential backoff holds.
            if previous == false {
                await handleNetworkBecameLanCapable()
            }
        } else if previous != false {
            handleNetworkLeftLan()
        }
    }

    private func handleNetworkBecameLanCapable() async {
        let targets = Array(trackedSmartPathPeers.values)
        for target in targets {
            let peerId = target.targetDevice.id
            if directP2PPeers[peerId] != nil { continue }
            smartPathPolicy.reset(peerId: peerId)
            _ = await reprobeDirectIfNeeded(
                targetDevice: target.targetDevice,
                clientIdentity: target.clientIdentity,
                clientDeviceId: target.clientDeviceId,
                udpProbeHost: target.udpProbeHost,
                udpProbePort: target.udpProbePort,
                force: false
            )
        }
    }

    private func handleNetworkLeftLan() {
        // When switching off Wi-Fi/Ethernet (e.g. to 5G), proactively drop ALL stale LAN sockets
        // so active Relay legs take over immediately without waiting for TCP keepalive timeout.
        let lanPeers: [(UUID, UInt64)] = directP2PPeers.compactMap { peerId, peer in
            peer.routeMode == .lanDirect ? (peerId, peer.generation) : nil
        }
        for (peerId, gen) in lanPeers {
            handleDirectP2PDisconnect(targetDeviceId: peerId, gen: gen)
        }
    }

    @discardableResult
    private func reprobeDirectIfNeeded(
        targetDevice: CodaRemoteDevice,
        clientIdentity: CodaDeviceIdentity,
        clientDeviceId: UUID,
        udpProbeHost: String,
        udpProbePort: UInt16,
        force: Bool = false
    ) async -> SmartPathRouteMode? {
        let targetId = targetDevice.id
        if let existing = directP2PPeers[targetId] {
            return existing.routeMode
        }
        guard !inFlightDirectProbes.contains(targetId) else {
            return nil
        }
        if !force && !smartPathPolicy.canProbeNow(peerId: targetId, isDirect: false) {
            return nil
        }
        inFlightDirectProbes.insert(targetId)
        defer { inFlightDirectProbes.remove(targetId) }

        let udpFound = await Task.detached {
            Self.probeLanUDP(targetDeviceId: targetId, host: udpProbeHost, port: udpProbePort)
        }.value
        guard !Task.isCancelled && (force || trackedSmartPathPeers[targetId] != nil) else {
            return nil
        }
        if let existing = directP2PPeers[targetId] {
            return existing.routeMode
        }
        let candidates = CodaSmartPathPolicy.buildDirectCandidates(
            udpFound: udpFound,
            endpoints: targetDevice.endpoints
        )
        for cand in candidates {
            guard !Task.isCancelled && (force || trackedSmartPathPeers[targetId] != nil) else {
                return nil
            }
            if let route = try? await performDirectP2PHandshake(
                host: cand.host,
                port: cand.port,
                clientIdentity: clientIdentity,
                clientDeviceId: clientDeviceId,
                targetDeviceId: targetId,
                expectedDaemonEd25519Pubkey: targetDevice.ed25519Pubkey,
                expectedDaemonX25519Pubkey: targetDevice.x25519Pubkey
            ) {
                if Task.isCancelled || (!force && trackedSmartPathPeers[targetId] == nil) {
                    if let leaked = directP2PPeers.removeValue(forKey: targetId) {
                        leaked.readerTask?.cancel()
                        leaked.connection.cancel()
                    }
                    if p2pPeerDeviceId == targetId {
                        p2pPeerDeviceId = nil
                    }
                    return nil
                }
                smartPathPolicy.recordUpgradeSuccess(peerId: targetId)
                emitRouteChangedEvent(peerDeviceId: targetId, routeMode: route, isDirect: true)
                return route
            }
        }
        smartPathPolicy.recordProbeFailure(peerId: targetId)
        return nil
    }

    // MARK: - Unified E2EE PeerMessage RPC (Direct P2P, WSS Relay, Domestic Raw TCP)

    private func buildRoutePacketDict(
        sealed: CodaEncryptedPayload,
        clientIdentity: CodaDeviceIdentity,
        clientDeviceId: UUID,
        targetDeviceId: UUID,
        lane: String,
        relayAuthState: RelayPeerSessionState? = nil
    ) -> [String: Any] {
        var e2eeDict: [String: Any] = [
            "seq": sealed.seq,
            "compressed": sealed.compressed,
            "ciphertext_b64": sealed.ciphertextB64,
        ]
        if let nonce = sealed.sessionNonceB64 {
            e2eeDict["session_nonce_b64"] = nonce
        }
        var pkt: [String: Any] = [
            "frame_type": "route_packet",
            "src_device_id": clientDeviceId.uuidString.lowercased(),
            "dst_device_id": targetDeviceId.uuidString.lowercased(),
            "lane": lane,
            "e2ee": e2eeDict,
            "sender_x25519_pub_b64": clientIdentity.x25519PublicB64,
        ]
        if let auth = relayAuthState, !auth.ackedByPeer {
            pkt["session_auth"] = [
                "session_nonce_b64": auth.sessionNonceB64,
                "init_ts_ms": auth.initTsMs,
                "sender_ed25519_pub_b64": clientIdentity.ed25519PublicB64,
                "sender_x25519_pub_b64": clientIdentity.x25519PublicB64,
                "supports_zstd": CodaE2EESession.hasNativeZstdFFI,
                "signature_b64": auth.signatureB64,
            ]
        }
        return pkt
    }

    private func failPendingRequest(reqId: String, error: Error) {
        guard let pending = pendingRequests.removeValue(forKey: reqId) else { return }
        pending.timeoutTask.cancel()
        pending.continuation.finish(throwing: error)
    }

    private func failPendingRequests(forTransportGen gen: UInt64, error: Error) {
        let doomed = pendingRequests.filter { $0.value.transportGen == gen }.map(\.key)
        for reqId in doomed {
            failPendingRequest(reqId: reqId, error: error)
        }
    }

    private func failPendingRequests(forPeer peerDeviceId: UUID, error: Error) {
        let doomed = pendingRequests.filter { $0.value.peerDeviceId == peerDeviceId }.map(\.key)
        for reqId in doomed {
            failPendingRequest(reqId: reqId, error: error)
        }
    }

    private func failPendingRequests(forPeer peerDeviceId: UUID, transportGen: UInt64, error: Error) {
        let doomed = pendingRequests.filter {
            $0.value.peerDeviceId == peerDeviceId && $0.value.transportGen == transportGen
        }.map(\.key)
        for reqId in doomed {
            failPendingRequest(reqId: reqId, error: error)
        }
    }

    private func failAllPendingRequests(error: Error) {
        let keys = Array(pendingRequests.keys)
        for reqId in keys {
            failPendingRequest(reqId: reqId, error: error)
        }
    }

    private func enqueueAsyncEvent(_ event: CodaAsyncEvent) {
        if let handler = asyncEventHandler {
            handler(event)
            return
        }
        if asyncEvents.count >= 2048 {
            asyncEvents.removeFirst(asyncEvents.count - 2047)
        }
        asyncEvents.append(event)
    }

    private func handleDecryptedPeerPayload(_ pt: Data, sourceDeviceId: UUID) {
        guard let obj = try? JSONSerialization.jsonObject(with: pt) as? [String: Any],
              let op = obj["op"] as? String else {
            return
        }
        if op == "pty_output_chunk" || op == "herdr_live_event" {
            enqueueAsyncEvent(CodaAsyncEvent(op: op, sourceDeviceId: sourceDeviceId, payloadData: pt))
            return
        }
        if let reqId = obj["req_id"] as? String, !reqId.isEmpty,
           let pending = pendingRequests[reqId] {
            guard pending.peerDeviceId == sourceDeviceId else {
                // Reject cross-peer req_id spoofing / collision without consuming the legitimate pending request.
                return
            }
            pendingRequests.removeValue(forKey: reqId)
            pending.timeoutTask.cancel()
            pending.continuation.yield(pt)
            pending.continuation.finish()
            return
        }
        enqueueAsyncEvent(CodaAsyncEvent(op: op, sourceDeviceId: sourceDeviceId, payloadData: pt))
    }

    private func handleInboundDirectData(_ frameData: Data, targetDeviceId: UUID, gen: UInt64) {
        guard var peer = directP2PPeers[targetDeviceId], peer.generation == gen else { return }
        guard let frame = try? JSONSerialization.jsonObject(with: frameData) as? [String: Any] else { return }
        if (frame["frame_type"] as? String) == "route_packet",
           let e2eeDict = frame["e2ee"] as? [String: Any],
           let e2eeData = try? JSONSerialization.data(withJSONObject: e2eeDict),
           let encPayload = try? JSONDecoder().decode(CodaEncryptedPayload.self, from: e2eeData),
           let pt = try? peer.session.openJSON(encPayload) {
            directP2PPeers[targetDeviceId] = peer
            handleDecryptedPeerPayload(pt, sourceDeviceId: targetDeviceId)
        }
    }

    private func handleDirectP2PDisconnect(targetDeviceId: UUID, gen: UInt64) {
        var didDrop = false
        if let existing = directP2PPeers[targetDeviceId], existing.generation == gen {
            directP2PPeers.removeValue(forKey: targetDeviceId)
            existing.readerTask?.cancel()
            existing.connection.cancel()
            if p2pPeerDeviceId == targetDeviceId {
                p2pPeerDeviceId = nil
            }
            didDrop = true
        }
        failPendingRequests(forTransportGen: gen, error: URLError(.networkConnectionLost))
        if didDrop {
            smartPathPolicy.recordDirectDrop(peerId: targetDeviceId)
            let hasOpenRelay = (wsTask != nil || rawTCPConnection != nil)
            if hasOpenRelay {
                let fallbackRoute = activeRelayRouteMode ?? .globalCDNWSS
                if connectedPeerDeviceId == targetDeviceId {
                    activeRouteMode = fallbackRoute
                }
                emitRouteChangedEvent(peerDeviceId: targetDeviceId, routeMode: fallbackRoute, isDirect: false)
            } else {
                if connectedPeerDeviceId == targetDeviceId {
                    activeRouteMode = .globalCDNWSS
                }
                enqueueAsyncEvent(
                    CodaAsyncEvent(op: "direct_disconnected_need_relay", sourceDeviceId: targetDeviceId, payloadData: Data())
                )
            }
        }
    }

    private func handleInboundRelayData(_ frameData: Data, gen: UInt64) {
        guard rawTCPGeneration == gen || wsGeneration == gen else { return }
        guard let frame = try? JSONSerialization.jsonObject(with: frameData) as? [String: Any],
              let frameType = frame["frame_type"] as? String else { return }
        if frameType == "route_packet",
           let srcStr = frame["src_device_id"] as? String,
           let srcUUID = UUID(uuidString: srcStr),
           let e2eeDict = frame["e2ee"] as? [String: Any],
           let e2eeData = try? JSONSerialization.data(withJSONObject: e2eeDict),
           let encPayload = try? JSONDecoder().decode(CodaEncryptedPayload.self, from: e2eeData),
           var state = relayPeerSessions[srcUUID],
           let pt = try? state.session.openJSON(encPayload) {
            state.ackedByPeer = true
            relayPeerSessions[srcUUID] = state
            // Prevent Relay vs Direct P2P dual-stream PTY chunk duplication while Direct P2P owns `srcUUID`.
            if directP2PPeers[srcUUID] != nil,
               let obj = try? JSONSerialization.jsonObject(with: pt) as? [String: Any],
               (obj["op"] as? String) == "pty_output_chunk" {
                return
            }
            handleDecryptedPeerPayload(pt, sourceDeviceId: srcUUID)
        } else if frameType == "session_reset",
                  let srcStr = frame["src_device_id"] as? String,
                  let srcUUID = UUID(uuidString: srcStr) {
            relayPeerSessions.removeValue(forKey: srcUUID)
            failPendingRequests(forPeer: srcUUID, transportGen: gen, error: URLError(.networkConnectionLost))
            enqueueAsyncEvent(CodaAsyncEvent(op: "peer_session_reset", sourceDeviceId: srcUUID, payloadData: Data()))
        } else if frameType == "peer_presence",
                  let devStr = frame["device_id"] as? String,
                  let devUUID = UUID(uuidString: devStr) {
            let isOnline = (frame["is_online"] as? Bool) ?? false
            relayPeerSessions.removeValue(forKey: devUUID)
            failPendingRequests(forPeer: devUUID, transportGen: gen, error: URLError(.networkConnectionLost))
            if isOnline {
                enqueueAsyncEvent(CodaAsyncEvent(op: "peer_session_reset", sourceDeviceId: devUUID, payloadData: Data()))
            } else if directP2PPeers[devUUID] == nil {
                failPendingRequests(forPeer: devUUID, error: URLError(.networkConnectionLost))
                enqueueAsyncEvent(CodaAsyncEvent(op: "peer_offline", sourceDeviceId: devUUID, payloadData: Data()))
            }
        } else if frameType == "auth_ack" {
            if (frame["ok"] as? Bool) == false {
                handleRelayDisconnect(gen: gen)
            }
        } else if frameType == "error" {
            let msg = ((frame["message"] as? String) ?? "").lowercased()
            let matchingPeer = relayPeerSessions.keys.first { msg.contains($0.uuidString.lowercased()) }
            if let peerId = matchingPeer {
                failPendingRequests(forPeer: peerId, transportGen: gen, error: URLError(.cannotConnectToHost))
            } else {
                failPendingRequests(forTransportGen: gen, error: URLError(.cannotConnectToHost))
            }
        }
    }

    private func handleRelayDisconnect(gen: UInt64) {
        var didDropRelay = false
        if rawTCPGeneration == gen {
            relayKeepAliveTask?.cancel()
            relayKeepAliveTask = nil
            rawTCPConnection?.cancel()
            rawTCPConnection = nil
            activeRelayRouteMode = nil
            relayBoundPeerDeviceId = nil
            didDropRelay = true
        }
        if wsGeneration == gen {
            relayKeepAliveTask?.cancel()
            relayKeepAliveTask = nil
            wsTask?.cancel(with: .normalClosure, reason: nil)
            wsTask = nil
            activeRelayRouteMode = nil
            relayBoundPeerDeviceId = nil
            didDropRelay = true
        }
        failPendingRequests(forTransportGen: gen, error: URLError(.networkConnectionLost))
        if didDropRelay {
            let affectedPeers = Array(relayPeerSessions.keys)
            relayPeerSessions.removeAll()
            for peerId in affectedPeers where directP2PPeers[peerId] == nil {
                enqueueAsyncEvent(CodaAsyncEvent(op: "peer_session_reset", sourceDeviceId: peerId, payloadData: Data()))
            }
        }
    }

    private static func runDirectP2PReadLoop(
        client: CodaNetworkClient?,
        conn: NWConnection,
        targetDeviceId: UUID,
        gen: UInt64
    ) async {
        while !Task.isCancelled {
            do {
                let frameData = try await readLengthDelimitedFrameData(conn)
                await client?.handleInboundDirectData(frameData, targetDeviceId: targetDeviceId, gen: gen)
            } catch {
                break
            }
        }
        await client?.handleDirectP2PDisconnect(targetDeviceId: targetDeviceId, gen: gen)
    }

    private static func runRawTCPRelayReadLoop(
        client: CodaNetworkClient?,
        conn: NWConnection,
        gen: UInt64
    ) async {
        while !Task.isCancelled {
            do {
                let frameData = try await readLengthDelimitedFrameData(conn)
                await client?.handleInboundRelayData(frameData, gen: gen)
            } catch {
                break
            }
        }
        await client?.handleRelayDisconnect(gen: gen)
    }

    private static func runWSSRelayReadLoop(
        client: CodaNetworkClient?,
        task: URLSessionWebSocketTask,
        gen: UInt64
    ) async {
        while !Task.isCancelled {
            do {
                let wsMsg = try await task.receive()
                let msgData: Data
                switch wsMsg {
                case .string(let s): msgData = Data(s.utf8)
                case .data(let d): msgData = d
                @unknown default: continue
                }
                await client?.handleInboundRelayData(msgData, gen: gen)
            } catch {
                break
            }
        }
        await client?.handleRelayDisconnect(gen: gen)
    }

    private func awaitRegisteredRequest(
        reqId: String,
        stream: AsyncThrowingStream<Data, Error>
    ) async throws -> Data {
        defer {
            if let leftover = pendingRequests.removeValue(forKey: reqId) {
                leftover.timeoutTask.cancel()
            }
        }
        for try await reply in stream {
            return reply
        }
        throw URLError(.cannotParseResponse)
    }

    public func requestOverDirectP2P(
        _ rawMsg: Data,
        clientIdentity: CodaDeviceIdentity,
        clientDeviceId: UUID,
        targetDeviceId: UUID,
        lane: String = "control_p0",
        timeoutSeconds: UInt64 = 15
    ) async throws -> Data {
        guard var peerState = self.directP2PPeers[targetDeviceId] else {
            throw URLError(.networkConnectionLost)
        }
        var finalData = rawMsg
        let reqId: String
        if var dict = try? JSONSerialization.jsonObject(with: rawMsg) as? [String: Any] {
            if let existing = dict["req_id"] as? String, !existing.isEmpty {
                reqId = existing
            } else {
                let generated = nextRequestID()
                dict["req_id"] = generated
                reqId = generated
                finalData = try JSONSerialization.data(withJSONObject: dict)
            }
        } else {
            reqId = nextRequestID()
        }

        let sealed = try peerState.session.sealJSON(finalData)
        self.directP2PPeers[targetDeviceId] = peerState

        let packetData = try Self.encodeLengthDelimitedJSON(
            buildRoutePacketDict(
                sealed: sealed,
                clientIdentity: clientIdentity,
                clientDeviceId: clientDeviceId,
                targetDeviceId: targetDeviceId,
                lane: lane
            )
        )

        let timeoutNs = max(5, timeoutSeconds) * 1_000_000_000
        let (stream, cont) = AsyncThrowingStream<Data, Error>.makeStream(bufferingPolicy: .bufferingNewest(1))
        let timeoutTask = Task { [weak self] in
            try? await Task.sleep(nanoseconds: timeoutNs)
            guard !Task.isCancelled else { return }
            await self?.failPendingRequest(reqId: reqId, error: URLError(.timedOut))
        }
        self.pendingRequests[reqId] = PendingRequest(
            peerDeviceId: targetDeviceId,
            transportGen: peerState.generation,
            continuation: cont,
            timeoutTask: timeoutTask
        )
        peerState.connection.send(content: packetData, completion: .contentProcessed { [weak self] err in
            if let err {
                Task { await self?.failPendingRequest(reqId: reqId, error: err) }
            }
        })
        return try await awaitRegisteredRequest(reqId: reqId, stream: stream)
    }

    public func sendPeerMessageData(
        _ messageJSONData: Data,
        to targetDevice: CodaRemoteDevice,
        clientIdentity: CodaDeviceIdentity,
        clientDeviceId: UUID,
        lane: String = "control_p0",
        timeoutSeconds: UInt64 = 15
    ) async throws -> Data {
        var finalData = messageJSONData
        let reqId: String
        if var dict = try? JSONSerialization.jsonObject(with: messageJSONData) as? [String: Any] {
            if let existing = dict["req_id"] as? String, !existing.isEmpty {
                reqId = existing
            } else {
                let generated = nextRequestID()
                dict["req_id"] = generated
                reqId = generated
                finalData = try JSONSerialization.data(withJSONObject: dict)
            }
        } else {
            reqId = nextRequestID()
        }

        // Path 1: Direct P2P (LAN IPv4 / Mainland China IPv6)
        if directP2PPeers[targetDevice.id] != nil {
            return try await requestOverDirectP2P(
                finalData,
                clientIdentity: clientIdentity,
                clientDeviceId: clientDeviceId,
                targetDeviceId: targetDevice.id,
                lane: lane,
                timeoutSeconds: timeoutSeconds
            )
        }

        // Prepare Relay v2 E2EE session for `targetDevice.id`
        var relayState = try ensureRelayPeerSession(
            for: targetDevice,
            clientIdentity: clientIdentity,
            clientDeviceId: clientDeviceId
        )
        let sealed = try relayState.session.sealJSON(finalData)
        relayPeerSessions[targetDevice.id] = relayState

        let routePacket = buildRoutePacketDict(
            sealed: sealed,
            clientIdentity: clientIdentity,
            clientDeviceId: clientDeviceId,
            targetDeviceId: targetDevice.id,
            lane: lane,
            relayAuthState: relayState
        )

        let timeoutNs = max(5, timeoutSeconds) * 1_000_000_000

        // Path 2: Global CDN WebSocket Relay
        if let ws = self.wsTask {
            let txt = String(data: try JSONSerialization.data(withJSONObject: routePacket), encoding: .utf8) ?? "{}"
            let (stream, cont) = AsyncThrowingStream<Data, Error>.makeStream(bufferingPolicy: .bufferingNewest(1))
            let timeoutTask = Task { [weak self] in
                try? await Task.sleep(nanoseconds: timeoutNs)
                guard !Task.isCancelled else { return }
                await self?.failPendingRequest(reqId: reqId, error: URLError(.timedOut))
            }
            self.pendingRequests[reqId] = PendingRequest(
                peerDeviceId: targetDevice.id,
                transportGen: self.wsGeneration,
                continuation: cont,
                timeoutTask: timeoutTask
            )
            ws.send(.string(txt)) { [weak self] err in
                if let err {
                    Task { await self?.failPendingRequest(reqId: reqId, error: err) }
                }
            }
            return try await awaitRegisteredRequest(reqId: reqId, stream: stream)
        }

        // Path 3: Domestic No-ICP Raw TCP Relay
        if let tcpConn = self.rawTCPConnection {
            let framed = try Self.encodeLengthDelimitedJSON(routePacket)
            let (stream, cont) = AsyncThrowingStream<Data, Error>.makeStream(bufferingPolicy: .bufferingNewest(1))
            let timeoutTask = Task { [weak self] in
                try? await Task.sleep(nanoseconds: timeoutNs)
                guard !Task.isCancelled else { return }
                await self?.failPendingRequest(reqId: reqId, error: URLError(.timedOut))
            }
            self.pendingRequests[reqId] = PendingRequest(
                peerDeviceId: targetDevice.id,
                transportGen: self.rawTCPGeneration,
                continuation: cont,
                timeoutTask: timeoutTask
            )
            tcpConn.send(content: framed, completion: .contentProcessed { [weak self] err in
                if let err {
                    Task { await self?.failPendingRequest(reqId: reqId, error: err) }
                }
            })
            return try await awaitRegisteredRequest(reqId: reqId, stream: stream)
        }

        throw URLError(.networkConnectionLost)
    }

    /// Dispatches an un-acked one-way E2EE `PeerMessage` (such as `pty_input` / `pty_resize`)
    /// across Direct P2P, CDN WSS, or Domestic Raw TCP without waiting for a response frame.
    public func sendOneWayPeerMessageData(
        _ messageJSONData: Data,
        to targetDevice: CodaRemoteDevice,
        clientIdentity: CodaDeviceIdentity,
        clientDeviceId: UUID,
        lane: String = "control_p0"
    ) async throws {
        if var peerState = directP2PPeers[targetDevice.id] {
            let sealed = try peerState.session.sealJSON(messageJSONData)
            self.directP2PPeers[targetDevice.id] = peerState
            let packetData = try Self.encodeLengthDelimitedJSON(
                buildRoutePacketDict(
                    sealed: sealed,
                    clientIdentity: clientIdentity,
                    clientDeviceId: clientDeviceId,
                    targetDeviceId: targetDevice.id,
                    lane: lane
                )
            )
            peerState.connection.send(content: packetData, completion: .contentProcessed { _ in })
            return
        }

        var relayState = try ensureRelayPeerSession(
            for: targetDevice,
            clientIdentity: clientIdentity,
            clientDeviceId: clientDeviceId
        )
        let sealed = try relayState.session.sealJSON(messageJSONData)
        relayPeerSessions[targetDevice.id] = relayState

        let routePacket = buildRoutePacketDict(
            sealed: sealed,
            clientIdentity: clientIdentity,
            clientDeviceId: clientDeviceId,
            targetDeviceId: targetDevice.id,
            lane: lane,
            relayAuthState: relayState
        )

        if let ws = self.wsTask {
            let txt = String(data: try JSONSerialization.data(withJSONObject: routePacket), encoding: .utf8) ?? "{}"
            ws.send(.string(txt)) { _ in }
            return
        }

        if let tcpConn = self.rawTCPConnection {
            let framed = try Self.encodeLengthDelimitedJSON(routePacket)
            tcpConn.send(content: framed, completion: .contentProcessed { _ in })
            return
        }

        throw URLError(.networkConnectionLost)
    }

    // MARK: - High-Level Typed RPCs

    public func listTerminals(
        on device: CodaRemoteDevice,
        clientIdentity: CodaDeviceIdentity,
        clientDeviceId: UUID,
        includeFinished: Bool = false
    ) async throws -> [CodaHerdrTerminalSummary] {
        let reqData = try JSONSerialization.data(withJSONObject: [
            "op": "list_terminals",
            "req_id": nextRequestID(),
            "include_finished": includeFinished,
        ])
        let respData = try await sendPeerMessageData(
            reqData,
            to: device,
            clientIdentity: clientIdentity,
            clientDeviceId: clientDeviceId,
            lane: "control_p0"
        )
        struct Resp: Decodable {
            let terminals: [CodaHerdrTerminalSummary]
        }
        return try JSONDecoder().decode(Resp.self, from: respData).terminals
    }

    public func listWorkspaces(
        on device: CodaRemoteDevice,
        clientIdentity: CodaDeviceIdentity,
        clientDeviceId: UUID,
        rootDirs: [String] = []
    ) async throws -> CodaWorkspacesFetchResult {
        let reqData = try JSONSerialization.data(withJSONObject: [
            "op": "list_workspaces",
            "req_id": nextRequestID(),
            "root_dirs": rootDirs,
        ])
        let respData = try await sendPeerMessageData(
            reqData,
            to: device,
            clientIdentity: clientIdentity,
            clientDeviceId: clientDeviceId,
            lane: "bulk_p1"
        )
        struct Resp: Decodable {
            let machineName: String
            let repos: [CodaRepoWorkspaceSummary]
            let voiceConfig: CodaVoiceModelConfig?
            let hostHealth: CodaHostHealthSnapshot?
            enum CodingKeys: String, CodingKey {
                case machineName = "machine_name"
                case repos
                case voiceConfig = "voice_config"
                case hostHealth = "host_health"
            }
        }
        let decoded = try JSONDecoder().decode(Resp.self, from: respData)
        return CodaWorkspacesFetchResult(
            machineName: decoded.machineName,
            repos: decoded.repos,
            voiceConfig: decoded.voiceConfig,
            hostHealth: decoded.hostHealth
        )
    }

    public func upgradeRemoteDaemon(
        on device: CodaRemoteDevice,
        clientIdentity: CodaDeviceIdentity,
        clientDeviceId: UUID,
        targetVersion: String? = nil
    ) async throws -> (ok: Bool, previousVersion: String, targetVersion: String?, message: String) {
        var reqObj: [String: Any] = [
            "op": "upgrade_daemon",
            "req_id": nextRequestID(),
        ]
        if let targetVersion, !targetVersion.isEmpty {
            reqObj["target_version"] = targetVersion
        }
        let reqData = try JSONSerialization.data(withJSONObject: reqObj)
        let respData = try await sendPeerMessageData(
            reqData,
            to: device,
            clientIdentity: clientIdentity,
            clientDeviceId: clientDeviceId,
            lane: "control_p0"
        )
        struct Resp: Decodable {
            let ok: Bool
            let previousVersion: String
            let targetVersion: String?
            let message: String
            enum CodingKeys: String, CodingKey {
                case ok
                case previousVersion = "previous_version"
                case targetVersion = "target_version"
                case message
            }
        }
        let decoded = try JSONDecoder().decode(Resp.self, from: respData)
        if decoded.ok {
            relayPeerSessions.removeValue(forKey: device.id)
            if let existing = directP2PPeers.removeValue(forKey: device.id) {
                existing.readerTask?.cancel()
                existing.connection.cancel()
            }
        }
        return (decoded.ok, decoded.previousVersion, decoded.targetVersion, decoded.message)
    }

    public func readTerminalScreen(
        on device: CodaRemoteDevice,
        clientIdentity: CodaDeviceIdentity,
        clientDeviceId: UUID,
        terminal: String,
        historyLines: Int = 180
    ) async throws -> CodaTerminalScreenResult {
        let reqData = try JSONSerialization.data(withJSONObject: [
            "op": "read_terminal_screen",
            "req_id": nextRequestID(),
            "terminal": terminal,
            "history_lines": historyLines,
        ])
        let respData = try await sendPeerMessageData(
            reqData,
            to: device,
            clientIdentity: clientIdentity,
            clientDeviceId: clientDeviceId,
            lane: "bulk_p1"
        )
        struct Resp: Decodable {
            let terminal: String
            let text: String
            let status: CodaHerdrAgentStatus?
        }
        let decoded = try JSONDecoder().decode(Resp.self, from: respData)
        return CodaTerminalScreenResult(
            terminal: decoded.terminal,
            text: decoded.text,
            status: decoded.status
        )
    }

    public func sendTerminalText(
        on device: CodaRemoteDevice,
        clientIdentity: CodaDeviceIdentity,
        clientDeviceId: UUID,
        terminal: String,
        text: String,
        submit: Bool = true
    ) async throws {
        let finalText = submit && !text.hasSuffix("\n") && !text.hasSuffix("\r") ? text + "\n" : text
        let reqData = try JSONSerialization.data(withJSONObject: [
            "op": "send_terminal_text",
            "req_id": nextRequestID(),
            "terminal": terminal,
            "text": finalText,
        ])
        _ = try await sendPeerMessageData(
            reqData,
            to: device,
            clientIdentity: clientIdentity,
            clientDeviceId: clientDeviceId,
            lane: "control_p0"
        )
    }

    public func sendTerminalKeys(
        on device: CodaRemoteDevice,
        clientIdentity: CodaDeviceIdentity,
        clientDeviceId: UUID,
        terminal: String,
        keys: [String]
    ) async throws {
        let reqData = try JSONSerialization.data(withJSONObject: [
            "op": "send_terminal_keys",
            "req_id": nextRequestID(),
            "terminal": terminal,
            "keys": keys,
        ])
        _ = try await sendPeerMessageData(
            reqData,
            to: device,
            clientIdentity: clientIdentity,
            clientDeviceId: clientDeviceId,
            lane: "control_p0"
        )
    }

    public func attachTerminalPty(
        on device: CodaRemoteDevice,
        clientIdentity: CodaDeviceIdentity,
        clientDeviceId: UUID,
        terminal: String,
        cols: Int = 100,
        rows: Int = 32
    ) async throws -> Bool {
        let clampedCols = max(20, min(400, cols))
        let clampedRows = max(6, min(200, rows))
        let reqData = try JSONSerialization.data(withJSONObject: [
            "op": "pty_attach",
            "req_id": nextRequestID(),
            "terminal": terminal,
            "cols": clampedCols,
            "rows": clampedRows,
        ])
        let respData = try await sendPeerMessageData(
            reqData,
            to: device,
            clientIdentity: clientIdentity,
            clientDeviceId: clientDeviceId,
            lane: "control_p0"
        )
        struct Resp: Decodable {
            let ok: Bool
        }
        return (try? JSONDecoder().decode(Resp.self, from: respData).ok) ?? false
    }

    public func sendPtyInput(
        on device: CodaRemoteDevice,
        clientIdentity: CodaDeviceIdentity,
        clientDeviceId: UUID,
        terminal: String,
        data: Data
    ) async throws {
        let msgData = try JSONSerialization.data(withJSONObject: [
            "op": "pty_input",
            "terminal": terminal,
            "data_b64": data.base64EncodedString(),
        ])
        do {
            try await sendOneWayPeerMessageData(
                msgData,
                to: device,
                clientIdentity: clientIdentity,
                clientDeviceId: clientDeviceId,
                lane: "control_p0"
            )
        } catch {
            if let str = String(data: data, encoding: .utf8) {
                let reqData = try JSONSerialization.data(withJSONObject: [
                    "op": "send_terminal_text",
                    "req_id": nextRequestID(),
                    "terminal": terminal,
                    "text": str,
                ])
                _ = try await sendPeerMessageData(
                    reqData,
                    to: device,
                    clientIdentity: clientIdentity,
                    clientDeviceId: clientDeviceId,
                    lane: "control_p0"
                )
            } else {
                throw error
            }
        }
    }

    public func resizeTerminalPty(
        on device: CodaRemoteDevice,
        clientIdentity: CodaDeviceIdentity,
        clientDeviceId: UUID,
        terminal: String,
        cols: Int,
        rows: Int
    ) async throws {
        let clampedCols = max(20, min(400, cols))
        let clampedRows = max(6, min(200, rows))
        let msgData = try JSONSerialization.data(withJSONObject: [
            "op": "pty_resize",
            "terminal": terminal,
            "cols": clampedCols,
            "rows": clampedRows,
        ])
        try await sendOneWayPeerMessageData(
            msgData,
            to: device,
            clientIdentity: clientIdentity,
            clientDeviceId: clientDeviceId,
            lane: "control_p0"
        )
    }

    public func interruptTerminal(
        on device: CodaRemoteDevice,
        clientIdentity: CodaDeviceIdentity,
        clientDeviceId: UUID,
        terminal: String
    ) async throws {
        let reqData = try JSONSerialization.data(withJSONObject: [
            "op": "interrupt_terminal",
            "req_id": nextRequestID(),
            "terminal": terminal,
        ])
        _ = try await sendPeerMessageData(
            reqData,
            to: device,
            clientIdentity: clientIdentity,
            clientDeviceId: clientDeviceId,
            lane: "control_p0"
        )
    }

    public func killTerminal(
        on device: CodaRemoteDevice,
        clientIdentity: CodaDeviceIdentity,
        clientDeviceId: UUID,
        terminal: String
    ) async throws {
        let reqData = try JSONSerialization.data(withJSONObject: [
            "op": "kill_terminal",
            "req_id": nextRequestID(),
            "terminal": terminal,
        ])
        _ = try await sendPeerMessageData(
            reqData,
            to: device,
            clientIdentity: clientIdentity,
            clientDeviceId: clientDeviceId,
            lane: "control_p0"
        )
    }

    public func launchAgent(
        on device: CodaRemoteDevice,
        clientIdentity: CodaDeviceIdentity,
        clientDeviceId: UUID,
        agent: String,
        name: String? = nil,
        cwd: String? = nil,
        initialPrompt: String? = nil,
        cols: Int = 100,
        rows: Int = 32
    ) async throws -> CodaHerdrTerminalSummary? {
        var obj: [String: Any] = [
            "op": "launch_agent",
            "req_id": nextRequestID(),
            "agent": agent,
            "cols": cols,
            "rows": rows,
        ]
        if let name { obj["name"] = name }
        if let cwd { obj["cwd"] = cwd }
        if let initialPrompt { obj["initial_prompt"] = initialPrompt }

        let reqData = try JSONSerialization.data(withJSONObject: obj)
        let respData = try await sendPeerMessageData(
            reqData,
            to: device,
            clientIdentity: clientIdentity,
            clientDeviceId: clientDeviceId,
            lane: "control_p0"
        )
        struct Resp: Decodable {
            let ok: Bool
            let terminal: CodaHerdrTerminalSummary?
        }
        return (try? JSONDecoder().decode(Resp.self, from: respData))?.terminal
    }

    public func uploadTerminalFile(
        on device: CodaRemoteDevice,
        clientIdentity: CodaDeviceIdentity,
        clientDeviceId: UUID,
        terminal: String,
        filename: String,
        data: Data
    ) async throws -> String? {
        let reqData = try JSONSerialization.data(withJSONObject: [
            "op": "upload_terminal_file",
            "req_id": nextRequestID(),
            "terminal": terminal,
            "filename": filename,
            "data_b64": data.base64EncodedString(),
        ])
        let respData = try await sendPeerMessageData(
            reqData,
            to: device,
            clientIdentity: clientIdentity,
            clientDeviceId: clientDeviceId,
            lane: "bulk_p1"
        )
        struct Resp: Decodable {
            let ok: Bool
            let remotePath: String?
            enum CodingKeys: String, CodingKey {
                case ok
                case remotePath = "remote_path"
            }
        }
        return (try? JSONDecoder().decode(Resp.self, from: respData))?.remotePath
    }

    public func fetchRemoteGitDiff(
        device: CodaRemoteDevice,
        clientIdentity: CodaDeviceIdentity,
        clientDeviceId: UUID,
        cwd: String
    ) async throws -> CodaGitDiffResult {
        let reqData = try JSONSerialization.data(withJSONObject: [
            "op": "git_diff_request",
            "req_id": nextRequestID(),
            "cwd": cwd,
        ])
        let respData = try await sendPeerMessageData(
            reqData,
            to: device,
            clientIdentity: clientIdentity,
            clientDeviceId: clientDeviceId,
            lane: "bulk_p1"
        )
        return try JSONDecoder().decode(CodaGitDiffResult.self, from: respData)
    }

    public func fetchGitDiff(
        on device: CodaRemoteDevice,
        clientIdentity: CodaDeviceIdentity,
        clientDeviceId: UUID,
        cwd: String
    ) async throws -> CodaGitDiffResult {
        try await fetchRemoteGitDiff(
            device: device,
            clientIdentity: clientIdentity,
            clientDeviceId: clientDeviceId,
            cwd: cwd
        )
    }

    public func bootstrapRemoteWorkspace(
        device: CodaRemoteDevice,
        clientIdentity: CodaDeviceIdentity,
        clientDeviceId: UUID,
        parentDir: String,
        folderName: String,
        gitCloneUrl: String? = nil
    ) async throws -> CodaBootstrapWorkspaceResult {
        let cleanParent = parentDir.trimmingCharacters(in: .whitespacesAndNewlines)
        let cleanFolder = folderName.trimmingCharacters(in: .whitespacesAndNewlines)
        let cleanClone = gitCloneUrl?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""

        guard !cleanParent.isEmpty else {
            throw CodaAPIError.httpError(statusCode: 400, message: "Parent directory cannot be empty.")
        }
        guard CodaWorkspaceBootstrapValidator.isValidFolderName(cleanFolder) else {
            throw CodaAPIError.httpError(
                statusCode: 400,
                message: "Invalid folder name (must be a single safe directory name without '/' or leading '-')."
            )
        }
        guard CodaWorkspaceBootstrapValidator.isAllowedGitCloneURL(cleanClone) else {
            throw CodaAPIError.httpError(
                statusCode: 400,
                message: "Git clone URL must use https://, ssh://, or git@<host>:<path>."
            )
        }

        var reqObj: [String: Any] = [
            "op": "bootstrap_workspace",
            "req_id": nextRequestID(),
            "parent_dir": cleanParent,
            "folder_name": cleanFolder,
        ]
        if !cleanClone.isEmpty {
            reqObj["git_clone_url"] = cleanClone
        }
        let reqData = try JSONSerialization.data(withJSONObject: reqObj)
        let respData = try await sendPeerMessageData(
            reqData,
            to: device,
            clientIdentity: clientIdentity,
            clientDeviceId: clientDeviceId,
            lane: "control_p0",
            timeoutSeconds: cleanClone.isEmpty ? 20 : 65
        )
        return try JSONDecoder().decode(CodaBootstrapWorkspaceResult.self, from: respData)
    }

    public func detachTerminalPty(
        on device: CodaRemoteDevice,
        clientIdentity: CodaDeviceIdentity,
        clientDeviceId: UUID,
        terminal: String
    ) async throws {
        let reqData = try JSONSerialization.data(withJSONObject: [
            "op": "pty_detach",
            "req_id": nextRequestID(),
            "terminal": terminal,
        ])
        _ = try await sendPeerMessageData(
            reqData,
            to: device,
            clientIdentity: clientIdentity,
            clientDeviceId: clientDeviceId,
            lane: "control_p0"
        )
    }

    /// Best-effort one-way `pty_detach` sent explicitly over the Relay transport (even when Direct P2P
    /// is already registered) so the daemon immediately tears down the superseded Relay PTY subscription.
    public func detachTerminalPtyOverRelay(
        on device: CodaRemoteDevice,
        clientIdentity: CodaDeviceIdentity,
        clientDeviceId: UUID,
        terminal: String
    ) async {
        guard wsTask != nil || rawTCPConnection != nil,
              var relayState = relayPeerSessions[device.id] else {
            return
        }
        guard let reqData = try? JSONSerialization.data(withJSONObject: [
            "op": "pty_detach",
            "req_id": nextRequestID(),
            "terminal": terminal,
        ]),
              let sealed = try? relayState.session.sealJSON(reqData) else {
            return
        }
        relayPeerSessions[device.id] = relayState
        let routePacket = buildRoutePacketDict(
            sealed: sealed,
            clientIdentity: clientIdentity,
            clientDeviceId: clientDeviceId,
            targetDeviceId: device.id,
            lane: "control_p0",
            relayAuthState: relayState
        )
        if let ws = self.wsTask,
           let jsonData = try? JSONSerialization.data(withJSONObject: routePacket),
           let txt = String(data: jsonData, encoding: .utf8) {
            ws.send(.string(txt)) { _ in }
        } else if let tcpConn = self.rawTCPConnection,
                  let framed = try? Self.encodeLengthDelimitedJSON(routePacket) {
            tcpConn.send(content: framed, completion: .contentProcessed { _ in })
        }
    }

    public func setVoiceConfig(
        on device: CodaRemoteDevice,
        clientIdentity: CodaDeviceIdentity,
        clientDeviceId: UUID,
        config: CodaVoiceModelConfig
    ) async throws -> CodaVoiceModelConfig {
        let reqData = try JSONSerialization.data(withJSONObject: [
            "op": "set_voice_config",
            "req_id": nextRequestID(),
            "config": [
                "provider": config.provider,
                "base_url": config.baseURL,
                "api_key": config.apiKey,
                "model": config.model,
            ],
        ])
        let respData = try await sendPeerMessageData(
            reqData,
            to: device,
            clientIdentity: clientIdentity,
            clientDeviceId: clientDeviceId,
            lane: "control_p0"
        )
        struct Resp: Decodable {
            let ok: Bool
            let config: CodaVoiceModelConfig
        }
        let decoded = try JSONDecoder().decode(Resp.self, from: respData)
        return decoded.config
    }

    public func listRemoteDirectory(
        on device: CodaRemoteDevice,
        clientIdentity: CodaDeviceIdentity,
        clientDeviceId: UUID,
        rootPath: String,
        subPath: String = ""
    ) async throws -> CodaDirectoryListingResult {
        let reqData = try JSONSerialization.data(withJSONObject: [
            "op": "list_directory",
            "req_id": nextRequestID(),
            "root_path": rootPath,
            "sub_path": subPath,
        ])
        let respData = try await sendPeerMessageData(
            reqData,
            to: device,
            clientIdentity: clientIdentity,
            clientDeviceId: clientDeviceId,
            lane: "bulk_p1"
        )
        struct Resp: Decodable {
            let rootPath: String
            let subPath: String
            let entries: [CodaRemoteFileEntry]
            let error: String?
            enum CodingKeys: String, CodingKey {
                case rootPath = "root_path"
                case subPath = "sub_path"
                case entries
                case error
            }
        }
        let decoded = try JSONDecoder().decode(Resp.self, from: respData)
        return CodaDirectoryListingResult(
            rootPath: decoded.rootPath,
            subPath: decoded.subPath,
            entries: decoded.entries,
            error: decoded.error
        )
    }

    public func transcribeAudio(
        on device: CodaRemoteDevice,
        clientIdentity: CodaDeviceIdentity,
        clientDeviceId: UUID,
        audioData: Data,
        mimeType: String = "audio/m4a",
        filename: String = "dictation.m4a",
        language: String? = nil
    ) async throws -> CodaTranscribeAudioResult {
        var reqObj: [String: Any] = [
            "op": "transcribe_audio",
            "req_id": nextRequestID(),
            "audio_b64": audioData.base64EncodedString(),
            "mime_type": mimeType,
            "filename": filename,
        ]
        if let language, !language.isEmpty {
            reqObj["language"] = language
        }
        let reqData = try JSONSerialization.data(withJSONObject: reqObj)
        let respData = try await sendPeerMessageData(
            reqData,
            to: device,
            clientIdentity: clientIdentity,
            clientDeviceId: clientDeviceId,
            lane: "bulk_p1",
            timeoutSeconds: 45
        )
        return try JSONDecoder().decode(CodaTranscribeAudioResult.self, from: respData)
    }

    public func readRemoteFilePreview(
        on device: CodaRemoteDevice,
        clientIdentity: CodaDeviceIdentity,
        clientDeviceId: UUID,
        path: String,
        maxBytes: Int = 65536
    ) async throws -> CodaFilePreviewResult {
        let reqData = try JSONSerialization.data(withJSONObject: [
            "op": "read_file_preview",
            "req_id": nextRequestID(),
            "path": path,
            "max_bytes": maxBytes,
        ])
        let respData = try await sendPeerMessageData(
            reqData,
            to: device,
            clientIdentity: clientIdentity,
            clientDeviceId: clientDeviceId,
            lane: "bulk_p1"
        )
        struct Resp: Decodable {
            let path: String
            let content: String
            let error: String?
            let truncated: Bool?
        }
        let decoded = try JSONDecoder().decode(Resp.self, from: respData)
        return CodaFilePreviewResult(
            path: decoded.path,
            content: decoded.content,
            error: decoded.error,
            truncated: decoded.truncated ?? false
        )
    }

    public func fetchLocalPortPreview(
        on device: CodaRemoteDevice,
        clientIdentity: CodaDeviceIdentity,
        clientDeviceId: UUID,
        port: Int,
        path: String = "/"
    ) async throws -> CodaPortPreviewResult {
        let clampedPort = max(1024, min(65535, port))
        let cleanPath = path.hasPrefix("/") ? path : "/\(path)"
        let reqData = try JSONSerialization.data(withJSONObject: [
            "op": "http_local_preview_request",
            "req_id": nextRequestID(),
            "port": clampedPort,
            "path": cleanPath,
        ])
        let respData = try await sendPeerMessageData(
            reqData,
            to: device,
            clientIdentity: clientIdentity,
            clientDeviceId: clientDeviceId,
            lane: "bulk_p1"
        )
        struct Resp: Decodable {
            let statusCode: Int
            let contentType: String
            let bodyB64: String
            let truncated: Bool?
            enum CodingKeys: String, CodingKey {
                case statusCode = "status_code"
                case contentType = "content_type"
                case bodyB64 = "body_b64"
                case truncated
            }
        }
        let decoded = try JSONDecoder().decode(Resp.self, from: respData)
        let rawBytes = Data(base64Encoded: decoded.bodyB64) ?? Data()
        let ctLower = decoded.contentType.lowercased()
        let isHtmlPayload = ctLower.contains("text/html") || ctLower.contains("application/xhtml")
        let text: String
        let rawHTML: String?
        if ctLower.hasPrefix("image/") || ctLower.hasPrefix("audio/") || ctLower.hasPrefix("video/") {
            text = "(Binary payload: \(rawBytes.count) bytes, Content-Type: \(decoded.contentType))"
            rawHTML = nil
        } else {
            let utf8 = String(decoding: rawBytes, as: UTF8.self)
            rawHTML = isHtmlPayload ? utf8 : nil
            if ctLower.contains("json"),
               let jsonObj = try? JSONSerialization.jsonObject(with: rawBytes),
               let pretty = try? JSONSerialization.data(withJSONObject: jsonObj, options: [.prettyPrinted, .sortedKeys]) {
                text = String(decoding: pretty, as: UTF8.self)
            } else {
                text = utf8.count > 65536 ? String(utf8.prefix(65536)) + "\n… [truncated at 64 KB]" : utf8
            }
        }
        return CodaPortPreviewResult(
            statusCode: decoded.statusCode,
            contentType: decoded.contentType,
            bodyText: text,
            rawHTML: rawHTML,
            truncated: decoded.truncated ?? (rawBytes.count > 65536)
        )
    }
}
