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

    public init(ticket: String, rawTCPHost: String?, rawTCPPort: UInt16?, wssURL: URL?) {
        self.ticket = ticket
        self.rawTCPHost = rawTCPHost
        self.rawTCPPort = rawTCPPort
        self.wssURL = wssURL
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

public struct CodaGitDiffResult: Sendable {
    public let branch: String
    public let statusShort: String
    public let diffUnified: String

    public init(branch: String, statusShort: String, diffUnified: String) {
        self.branch = branch
        self.statusShort = statusShort
        self.diffUnified = diffUnified
    }
}

public struct CodaWorkspacesFetchResult: Sendable {
    public let machineName: String
    public let repos: [CodaRepoWorkspaceSummary]
    public let voiceConfig: CodaVoiceModelConfig?

    public init(machineName: String, repos: [CodaRepoWorkspaceSummary], voiceConfig: CodaVoiceModelConfig?) {
        self.machineName = machineName
        self.repos = repos
        self.voiceConfig = voiceConfig
    }
}

/// Native Apple `actor` managing Multi-Tenant Auth, Device Pairing, SmartPath Auto-Negotiation,
/// and unified E2EE PeerMessage RPC across Direct P2P, Domestic Raw TCP, and CDN WSS Relays.
public actor CodaNetworkClient {
    public let serverBaseURL: URL
    private var bearerToken: String?
    private var wsTask: URLSessionWebSocketTask?
    private var rawTCPConnection: NWConnection?
    private var directP2PConnection: NWConnection?
    private var p2pPeerDeviceId: UUID?
    private var directE2EESession: CodaE2EESession?
    private var relayPeerSessions: [UUID: CodaE2EESession] = [:]
    private var connectedPeerDeviceId: UUID?
    private var asyncEvents: [CodaAsyncEvent] = []
    public private(set) var activeRouteMode: SmartPathRouteMode = .globalCDNWSS
    private var pathMonitor: NWPathMonitor?
    private var currentGeneration: UInt64 = 1

    public init(serverBaseURL: URL) {
        self.serverBaseURL = serverBaseURL
    }

    public func setToken(_ token: String) {
        self.bearerToken = token
    }

    public func drainAsyncEvents() -> [CodaAsyncEvent] {
        let copy = asyncEvents
        asyncEvents.removeAll()
        return copy
    }

    public func disconnectTransport() {
        pathMonitor?.cancel()
        pathMonitor = nil
        directP2PConnection?.cancel()
        directP2PConnection = nil
        p2pPeerDeviceId = nil
        directE2EESession = nil
        rawTCPConnection?.cancel()
        rawTCPConnection = nil
        wsTask?.cancel(with: .normalClosure, reason: nil)
        wsTask = nil
        relayPeerSessions.removeAll()
        connectedPeerDeviceId = nil
    }

    // MARK: - Request ID Negotiation

    public func nextRequestID() -> String {
        let prefix = CodaE2EESession.hasNativeZstdFFI ? "ffi" : "swift"
        return "\(prefix)-\(UUID().uuidString.lowercased())"
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
            throw URLError(.userAuthenticationRequired)
        }
        let json = try JSONSerialization.jsonObject(with: data) as? [String: Any]
        guard let token = json?["token"] as? String else {
            throw URLError(.cannotParseResponse)
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
            throw URLError(.userAuthenticationRequired)
        }
        let json = try JSONSerialization.jsonObject(with: data) as? [String: Any]
        guard let token = json?["token"] as? String else {
            throw URLError(.cannotParseResponse)
        }
        self.bearerToken = token
        return token
    }

    public func fetchAuthMe() async throws -> (handle: String, email: String) {
        let url = serverBaseURL.appendingPathComponent("api/v1/auth/me")
        var req = URLRequest(url: url)
        req.httpMethod = "GET"
        if let token = bearerToken {
            req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        let (data, resp) = try await URLSession.shared.data(for: req)
        guard let http = resp as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
            throw URLError(.userAuthenticationRequired)
        }
        let json = try JSONSerialization.jsonObject(with: data) as? [String: Any]
        let handle = (json?["handle"] as? String) ?? ""
        let email = (json?["email"] as? String) ?? ""
        return (handle, email)
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
            wssURL: wssURL
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

    // MARK: - Domestic No-ICP Raw TCP Relay (`Network.framework` NWConnection, TCP_NODELAY, Zero SNI)

    public func connectDomesticRawTCP(host: String, port: UInt16, relayTicket: String) {
        let tcpOptions = NWProtocolTCP.Options()
        tcpOptions.noDelay = true
        tcpOptions.enableKeepalive = true
        let params = NWParameters(tls: nil, tcp: tcpOptions)

        guard let nwPort = NWEndpoint.Port(rawValue: port) else { return }
        let conn = NWConnection(host: NWEndpoint.Host(host), port: nwPort, using: params)
        self.rawTCPConnection?.cancel()
        self.rawTCPConnection = conn
        let gen = self.currentGeneration
        self.currentGeneration += 1

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
        let gen = self.currentGeneration
        self.currentGeneration += 1

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
        self.rawTCPConnection?.cancel()
        self.rawTCPConnection = conn
    }

    // MARK: - Overseas CDN WebSocket Relay (With 75s Make-Before-Break Seamless Handoff)

    public func connectOverseasCDNWSS(wssURL: URL, relayTicket: String) async throws {
        let nextGen = self.currentGeneration
        self.currentGeneration += 1

        let task = URLSession.shared.webSocketTask(with: wssURL)
        task.resume()
        let bindObj: [String: Any] = [
            "frame_type": "auth_bind",
            "ticket": relayTicket,
            "lane": "control_p0",
            "generation": nextGen,
        ]
        let bindStr = String(data: try JSONSerialization.data(withJSONObject: bindObj), encoding: .utf8) ?? "{}"
        try await task.send(.string(bindStr))

        let oldTask = self.wsTask
        self.wsTask = task
        oldTask?.cancel(with: .normalClosure, reason: Data("make-before-break".utf8))
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

    private static func readLengthDelimitedFrame(_ conn: NWConnection) async throws -> [String: Any] {
        let hdr = try await readExact(conn, length: 4)
        let len = hdr.withUnsafeBytes { raw -> UInt32 in
            let v = raw.loadUnaligned(as: UInt32.self)
            return UInt32(bigEndian: v)
        }
        guard len > 0 && len <= 8 * 1024 * 1024 else {
            throw URLError(.badServerResponse)
        }
        let body = try await readExact(conn, length: Int(len))
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
        timeoutMs: UInt64 = 350
    ) async throws -> SmartPathRouteMode {
        let tcpOptions = NWProtocolTCP.Options()
        tcpOptions.noDelay = true
        tcpOptions.enableKeepalive = true
        let params = NWParameters(tls: nil, tcp: tcpOptions)

        guard let nwPort = NWEndpoint.Port(rawValue: port) else {
            throw URLError(.badURL)
        }
        let conn = NWConnection(host: NWEndpoint.Host(host), port: nwPort, using: params)

        let tsMs = UInt64(Date().timeIntervalSince1970 * 1000)
        let clientIdLower = clientDeviceId.uuidString.lowercased()
        let targetIdLower = targetDeviceId.uuidString.lowercased()
        let signedMsg = "p2p:\(clientIdLower):\(targetIdLower):\(tsMs)"
        let sigB64 = try clientIdentity.sign(message: Data(signedMsg.utf8))

        let helloFramed = try Self.encodeLengthDelimitedJSON([
            "frame_type": "direct_p2p_hello",
            "client_device_id": clientIdLower,
            "target_device_id": targetIdLower,
            "client_ed25519_pub_b64": clientIdentity.ed25519PublicB64,
            "client_x25519_pub_b64": clientIdentity.x25519PublicB64,
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
              let daemonPubB64 = ack["daemon_x25519_pub_b64"] as? String
        else {
            conn.cancel()
            throw URLError(.userAuthenticationRequired)
        }

        let pathKind = (ack["path_kind"] as? String) ?? "lan_ipv4"
        self.directE2EESession = try CodaE2EESession(
            myX25519SecretB64: clientIdentity.x25519SecretB64,
            peerX25519PublicB64: daemonPubB64,
            isClientInitiator: true
        )
        self.directP2PConnection?.cancel()
        self.directP2PConnection = conn
        self.p2pPeerDeviceId = targetDeviceId
        self.connectedPeerDeviceId = targetDeviceId
        self.activeRouteMode = (pathKind == "cn_public_ipv6") ? .cnIPv6P2P : .lanDirect
        return self.activeRouteMode
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

    public func connectAutoSmartPath(
        targetDevice: CodaRemoteDevice,
        clientIdentity: CodaDeviceIdentity,
        clientDeviceId: UUID,
        udpProbeHost: String = "255.255.255.255",
        udpProbePort: UInt16 = 24444,
        relayRawTCP: (host: String, port: UInt16)? = nil,
        relayWSS: URL? = nil,
        relayTicket: String? = nil
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
                targetDeviceId: targetId
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

        // Seamless Relay fallback with dedicated directional E2EE session per peer daemon
        self.relayPeerSessions[targetId] = try CodaE2EESession(
            myIdentity: clientIdentity,
            peerX25519PublicB64: targetDevice.x25519Pubkey,
            isInitiator: true
        )
        let isRemoteHTTPS = serverBaseURL.scheme == "https"
        let hasReachableRawTCP = relayRawTCP.map { !Self.isLoopbackHost($0.host) } ?? false
        if !(isRemoteHTTPS && !hasReachableRawTCP), let raw = relayRawTCP, let ticket = relayTicket {
            try await connectDomesticRawTCPAsync(host: raw.host, port: raw.port, relayTicket: ticket)
            self.activeRouteMode = .domesticRawTCP
        } else if let wss = relayWSS, let ticket = relayTicket {
            try await connectOverseasCDNWSS(wssURL: wss, relayTicket: ticket)
            self.activeRouteMode = .globalCDNWSS
        } else if let raw = relayRawTCP, let ticket = relayTicket {
            try await connectDomesticRawTCPAsync(host: raw.host, port: raw.port, relayTicket: ticket)
            self.activeRouteMode = .domesticRawTCP
        } else {
            throw URLError(.cannotConnectToHost)
        }

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

    /// High-level SmartPath helper that mints relay tickets and connects to `targetDevice` automatically.
    public func connectBestAvailable(
        sessionToken: String,
        targetDevice: CodaRemoteDevice,
        clientIdentity: CodaDeviceIdentity
    ) async throws -> SmartPathRouteMode {
        self.bearerToken = sessionToken
        guard let clientDevId = clientIdentity.deviceId else {
            throw URLError(.userAuthenticationRequired)
        }
        if directP2PConnection != nil, p2pPeerDeviceId == targetDevice.id,
           (activeRouteMode == .lanDirect || activeRouteMode == .cnIPv6P2P) {
            return activeRouteMode
        }
        if connectedPeerDeviceId == targetDevice.id,
           (wsTask != nil || rawTCPConnection != nil),
           relayPeerSessions[targetDevice.id] != nil {
            return activeRouteMode
        }

        let ticketResp = try? await mintRelayTicket(deviceId: clientDevId, peerDeviceId: targetDevice.id)
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
            relayTicket: ticketResp?.ticket
        )
    }

    private func startWiFiPathMonitor(
        targetDevice: CodaRemoteDevice,
        clientIdentity: CodaDeviceIdentity,
        clientDeviceId: UUID,
        udpProbeHost: String,
        udpProbePort: UInt16
    ) {
        pathMonitor?.cancel()
        let monitor = NWPathMonitor()
        self.pathMonitor = monitor
        monitor.pathUpdateHandler = { [weak self] path in
            guard path.usesInterfaceType(.wifi) || path.usesInterfaceType(.wiredEthernet) else {
                return
            }
            Task { [weak self] in
                await self?.reprobeDirectIfNeeded(
                    targetDevice: targetDevice,
                    clientIdentity: clientIdentity,
                    clientDeviceId: clientDeviceId,
                    udpProbeHost: udpProbeHost,
                    udpProbePort: udpProbePort
                )
            }
        }
        monitor.start(queue: .global(qos: .utility))
    }

    private func reprobeDirectIfNeeded(
        targetDevice: CodaRemoteDevice,
        clientIdentity: CodaDeviceIdentity,
        clientDeviceId: UUID,
        udpProbeHost: String,
        udpProbePort: UInt16
    ) async {
        guard directP2PConnection == nil else { return }
        if let found = Self.probeLanUDP(targetDeviceId: targetDevice.id, host: udpProbeHost, port: udpProbePort) {
            _ = try? await performDirectP2PHandshake(
                host: found.host,
                port: found.port,
                clientIdentity: clientIdentity,
                clientDeviceId: clientDeviceId,
                targetDeviceId: targetDevice.id
            )
        }
    }

    // MARK: - Unified E2EE PeerMessage RPC (Direct P2P, WSS Relay, Domestic Raw TCP)

    public func requestOverDirectP2P(
        _ rawMsg: Data,
        clientIdentity: CodaDeviceIdentity,
        clientDeviceId: UUID,
        targetDeviceId: UUID,
        lane: String = "control_p0"
    ) async throws -> Data {
        guard let conn = self.directP2PConnection, var sess = self.directE2EESession else {
            throw URLError(.networkConnectionLost)
        }
        let sealed = try sess.sealJSON(rawMsg)

        let packetData = try Self.encodeLengthDelimitedJSON([
            "frame_type": "route_packet",
            "src_device_id": clientDeviceId.uuidString.lowercased(),
            "dst_device_id": targetDeviceId.uuidString.lowercased(),
            "lane": lane,
            "e2ee": [
                "seq": sealed.seq,
                "compressed": sealed.compressed,
                "ciphertext_b64": sealed.ciphertextB64,
            ],
            "sender_x25519_pub_b64": clientIdentity.x25519PublicB64,
        ])

        try await Self.sendNW(conn, data: packetData)

        let reqObj = try? JSONSerialization.jsonObject(with: rawMsg) as? [String: Any]
        let expectedReqId = reqObj?["req_id"] as? String

        while true {
            let replyFrame = try await Self.readLengthDelimitedFrame(conn)
            if (replyFrame["frame_type"] as? String) == "route_packet",
               let e2eeDict = replyFrame["e2ee"] as? [String: Any],
               let e2eeData = try? JSONSerialization.data(withJSONObject: e2eeDict) {
                let encPayload = try JSONDecoder().decode(CodaEncryptedPayload.self, from: e2eeData)
                let pt = try sess.openJSON(encPayload)
                self.directE2EESession = sess
                if let event = Self.extractAsyncEventIfUnrelated(pt, expectedReqId: expectedReqId, srcDevice: targetDeviceId) {
                    self.asyncEvents.append(event)
                    continue
                }
                return pt
            }
        }
    }

    public func sendPeerMessageData(
        _ messageJSONData: Data,
        to targetDevice: CodaRemoteDevice,
        clientIdentity: CodaDeviceIdentity,
        clientDeviceId: UUID,
        lane: String = "control_p0"
    ) async throws -> Data {
        var finalData = messageJSONData
        var reqId: String? = nil
        if var dict = try? JSONSerialization.jsonObject(with: messageJSONData) as? [String: Any] {
            if let existing = dict["req_id"] as? String, !existing.isEmpty {
                reqId = existing
            } else {
                let generated = nextRequestID()
                dict["req_id"] = generated
                reqId = generated
                finalData = try JSONSerialization.data(withJSONObject: dict)
            }
        }

        // Path 1: Direct P2P (LAN IPv4 / Mainland China IPv6)
        if (activeRouteMode == .lanDirect || activeRouteMode == .cnIPv6P2P),
           directP2PConnection != nil,
           p2pPeerDeviceId == targetDevice.id {
            return try await requestOverDirectP2P(
                finalData,
                clientIdentity: clientIdentity,
                clientDeviceId: clientDeviceId,
                targetDeviceId: targetDevice.id,
                lane: lane
            )
        }

        // Prepare Relay E2EE session for `targetDevice.id`
        var sess = try relayPeerSessions[targetDevice.id] ?? CodaE2EESession(
            myIdentity: clientIdentity,
            peerX25519PublicB64: targetDevice.x25519Pubkey,
            isInitiator: true
        )
        let sealed = try sess.sealJSON(finalData)
        relayPeerSessions[targetDevice.id] = sess

        let routePacket: [String: Any] = [
            "frame_type": "route_packet",
            "src_device_id": clientDeviceId.uuidString.lowercased(),
            "dst_device_id": targetDevice.id.uuidString.lowercased(),
            "lane": lane,
            "e2ee": [
                "seq": sealed.seq,
                "compressed": sealed.compressed,
                "ciphertext_b64": sealed.ciphertextB64,
            ],
            "sender_x25519_pub_b64": clientIdentity.x25519PublicB64,
        ]

        // Path 2: Global CDN WebSocket Relay
        if let ws = self.wsTask {
            let txt = String(data: try JSONSerialization.data(withJSONObject: routePacket), encoding: .utf8) ?? "{}"
            try await ws.send(.string(txt))
            while true {
                let wsMsg = try await ws.receive()
                let msgData: Data
                switch wsMsg {
                case .string(let s): msgData = Data(s.utf8)
                case .data(let d): msgData = d
                @unknown default: continue
                }
                guard let frame = try? JSONSerialization.jsonObject(with: msgData) as? [String: Any],
                      let frameType = frame["frame_type"] as? String else {
                    continue
                }
                if frameType == "route_packet",
                   let srcStr = (frame["src_device_id"] as? String)?.lowercased(),
                   srcStr == targetDevice.id.uuidString.lowercased(),
                   let e2eeDict = frame["e2ee"] as? [String: Any],
                   let e2eeData = try? JSONSerialization.data(withJSONObject: e2eeDict) {
                    let encPayload = try JSONDecoder().decode(CodaEncryptedPayload.self, from: e2eeData)
                    let pt = try sess.openJSON(encPayload)
                    relayPeerSessions[targetDevice.id] = sess
                    if let event = Self.extractAsyncEventIfUnrelated(pt, expectedReqId: reqId, srcDevice: targetDevice.id) {
                        self.asyncEvents.append(event)
                        continue
                    }
                    return pt
                } else if frameType == "error" {
                    throw URLError(.cannotConnectToHost)
                }
            }
        }

        // Path 3: Domestic No-ICP Raw TCP Relay
        if let tcpConn = self.rawTCPConnection {
            let framed = try Self.encodeLengthDelimitedJSON(routePacket)
            try await Self.sendNW(tcpConn, data: framed)
            while true {
                let frame = try await Self.readLengthDelimitedFrame(tcpConn)
                guard let frameType = frame["frame_type"] as? String else { continue }
                if frameType == "route_packet",
                   let srcStr = (frame["src_device_id"] as? String)?.lowercased(),
                   srcStr == targetDevice.id.uuidString.lowercased(),
                   let e2eeDict = frame["e2ee"] as? [String: Any],
                   let e2eeData = try? JSONSerialization.data(withJSONObject: e2eeDict) {
                    let encPayload = try JSONDecoder().decode(CodaEncryptedPayload.self, from: e2eeData)
                    let pt = try sess.openJSON(encPayload)
                    relayPeerSessions[targetDevice.id] = sess
                    if let event = Self.extractAsyncEventIfUnrelated(pt, expectedReqId: reqId, srcDevice: targetDevice.id) {
                        self.asyncEvents.append(event)
                        continue
                    }
                    return pt
                } else if frameType == "error" {
                    throw URLError(.cannotConnectToHost)
                }
            }
        }

        throw URLError(.networkConnectionLost)
    }

    private static func extractAsyncEventIfUnrelated(
        _ pt: Data,
        expectedReqId: String?,
        srcDevice: UUID
    ) -> CodaAsyncEvent? {
        guard let obj = try? JSONSerialization.jsonObject(with: pt) as? [String: Any],
              let op = obj["op"] as? String else {
            return nil
        }
        if op == "pty_output_chunk" || op == "herdr_live_event" {
            return CodaAsyncEvent(op: op, sourceDeviceId: srcDevice, payloadData: pt)
        }
        if let expectedReqId, let gotReqId = obj["req_id"] as? String, gotReqId != expectedReqId {
            return CodaAsyncEvent(op: op, sourceDeviceId: srcDevice, payloadData: pt)
        }
        return nil
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
        return (try? JSONDecoder().decode(Resp.self, from: respData).terminals) ?? []
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
            enum CodingKeys: String, CodingKey {
                case machineName = "machine_name"
                case repos
                case voiceConfig = "voice_config"
            }
        }
        let decoded = try JSONDecoder().decode(Resp.self, from: respData)
        return CodaWorkspacesFetchResult(
            machineName: decoded.machineName,
            repos: decoded.repos,
            voiceConfig: decoded.voiceConfig
        )
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

    public func sendPtyInput(
        on device: CodaRemoteDevice,
        clientIdentity: CodaDeviceIdentity,
        clientDeviceId: UUID,
        terminal: String,
        data: Data
    ) async throws {
        // Note: `PtyInput` is an un-acked stream frame; also support fallback via `send_terminal_keys`
        // for standard control sequences or send `send_terminal_text` when UTF-8 text is given.
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
        }
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

    public func fetchGitDiff(
        on device: CodaRemoteDevice,
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
        struct Resp: Decodable {
            let branch: String
            let statusShort: String
            let diffUnified: String
            enum CodingKeys: String, CodingKey {
                case branch
                case statusShort = "status_short"
                case diffUnified = "diff_unified"
            }
        }
        let decoded = try JSONDecoder().decode(Resp.self, from: respData)
        return CodaGitDiffResult(
            branch: decoded.branch,
            statusShort: decoded.statusShort,
            diffUnified: decoded.diffUnified
        )
    }
}
