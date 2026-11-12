import Foundation
import Network

/// Native Apple `actor` managing Multi-Tenant Auth, Device Pairing, and SmartPath Dual-Transport
/// (`NWConnection` Raw TCP No-SNI for Domestic Cloud + `URLSessionWebSocketTask` for Overseas CDN).
public actor CodaNetworkClient {
    public let serverBaseURL: URL
    private var bearerToken: String?
    private var wsTask: URLSessionWebSocketTask?
    private var rawTCPConnection: NWConnection?
    private var currentGeneration: UInt64 = 1

    public init(serverBaseURL: URL) {
        self.serverBaseURL = serverBaseURL
    }

    public func setToken(_ token: String) {
        self.bearerToken = token
    }

    // MARK: - Coordinator REST API (Auth, Friend Invites, 6-Digit Pairing)

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
        let (data, _) = try await URLSession.shared.data(for: req)
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
                if let payload = try? JSONSerialization.data(withJSONObject: bindJSON) {
                    var framed = Data(capacity: 4 + payload.count)
                    var lenBE = UInt32(payload.count).bigEndian
                    withUnsafeBytes(of: &lenBE) { framed.append(contentsOf: $0) }
                    framed.append(payload)
                    conn.send(content: framed, completion: .contentProcessed { _ in })
                }
            }
        }
        conn.start(queue: .global(qos: .userInteractive))
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

    // MARK: - Phase 3: Direct P2P Engine (`L0 LAN IPv4` + `L1 Mainland China Native IPv6 Direct`)

    public func connectDirectP2P(
        host: String,
        port: UInt16,
        clientIdentity: CodaDeviceIdentity,
        clientDeviceId: UUID,
        targetDeviceId: UUID
    ) throws {
        let tcpOptions = NWProtocolTCP.Options()
        tcpOptions.noDelay = true
        tcpOptions.enableKeepalive = true
        let params = NWParameters(tls: nil, tcp: tcpOptions)

        guard let nwPort = NWEndpoint.Port(rawValue: port) else { return }
        let conn = NWConnection(host: NWEndpoint.Host(host), port: nwPort, using: params)
        self.rawTCPConnection = conn

        let tsMs = UInt64(Date().timeIntervalSince1970 * 1000)
        let clientIdLower = clientDeviceId.uuidString.lowercased()
        let targetIdLower = targetDeviceId.uuidString.lowercased()
        let signedMsg = "p2p:\(clientIdLower):\(targetIdLower):\(tsMs)"
        let sigB64 = try clientIdentity.sign(message: Data(signedMsg.utf8))

        let helloObj: [String: Any] = [
            "frame_type": "direct_p2p_hello",
            "client_device_id": clientIdLower,
            "target_device_id": targetIdLower,
            "client_ed25519_pub_b64": clientIdentity.ed25519PublicB64,
            "client_x25519_pub_b64": clientIdentity.x25519PublicB64,
            "timestamp_ms": tsMs,
            "signature_b64": sigB64,
        ]
        let payload = try JSONSerialization.data(withJSONObject: helloObj)

        conn.stateUpdateHandler = { state in
            if case .ready = state {
                var framed = Data(capacity: 4 + payload.count)
                var lenBE = UInt32(payload.count).bigEndian
                withUnsafeBytes(of: &lenBE) { framed.append(contentsOf: $0) }
                framed.append(payload)
                conn.send(content: framed, completion: .contentProcessed { _ in })
            }
        }
        conn.start(queue: .global(qos: .userInteractive))
    }
}
