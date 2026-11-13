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

/// Native Apple `actor` managing Multi-Tenant Auth, Device Pairing, and SmartPath Auto-Negotiation
/// (`L0 LAN IPv4` UDP+TCP `<5ms` / `L1 CN Public IPv6` <-> `L2 Raw TCP No-SNI` / `L2 CDN WSS`).
public actor CodaNetworkClient {
    public let serverBaseURL: URL
    private var bearerToken: String?
    private var wsTask: URLSessionWebSocketTask?
    private var rawTCPConnection: NWConnection?
    private var directP2PConnection: NWConnection?
    private var directE2EESession: CodaE2EESession?
    private var relayE2EESession: CodaE2EESession?
    public private(set) var activeRouteMode: SmartPathRouteMode = .domesticRawTCP
    private var pathMonitor: NWPathMonitor?
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

    // MARK: - Phase 3: Auto-SmartPath Negotiator (UDP LAN Discovery + TCP Direct P2P + Seamless Relay Failover)

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
    /// Never calls `connect()` on the UDP socket so unicast replies from concrete LAN IPs are accepted.
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
        self.directP2PConnection = conn

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

    private static func encodeLengthDelimitedJSON(_ obj: [String: Any]) throws -> Data {
        let payload = try JSONSerialization.data(withJSONObject: obj)
        var framed = Data(capacity: 4 + payload.count)
        var lenBE = UInt32(payload.count).bigEndian
        withUnsafeBytes(of: &lenBE) { framed.append(contentsOf: $0) }
        framed.append(payload)
        return framed
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

    /// Races a Direct LAN/IPv6 TCP connection + Ed25519/X25519 handshake with strict timeout.
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
                case .ready:
                    box.succeed(())
                case .failed(let err):
                    box.fail(err)
                case .cancelled:
                    box.fail(URLError(.timedOut))
                default:
                    break
                }
            }
            conn.start(queue: .global(qos: .userInteractive))
        }

        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Void, Error>) in
            let box = OnceResume<Void>(cont)
            conn.send(content: helloFramed, completion: .contentProcessed { err in
                if let err {
                    box.fail(err)
                } else {
                    box.succeed(())
                }
            })
        }

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

    /// Automatically negotiates LAN UDP discovery + Direct P2P TCP, falling back to Relay and
    /// starting a Wi-Fi `NWPathMonitor` for zero-config hot-upgrade when returning to the same Wi-Fi.
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

        // Seamless Relay fallback (retaining its own independent E2EE session!)
        self.relayE2EESession = try CodaE2EESession(
            myX25519SecretB64: clientIdentity.x25519SecretB64,
            peerX25519PublicB64: targetDevice.x25519Pubkey,
            isClientInitiator: true
        )
        if let raw = relayRawTCP, let ticket = relayTicket {
            connectDomesticRawTCP(host: raw.host, port: raw.port, relayTicket: ticket)
            self.activeRouteMode = .domesticRawTCP
        } else if let wss = relayWSS, let ticket = relayTicket {
            try await connectOverseasCDNWSS(wssURL: wss, relayTicket: ticket)
            self.activeRouteMode = .globalCDNWSS
        } else {
            throw URLError(.cannotConnectToHost)
        }

        startWiFiPathMonitor(
            targetDevice: targetDevice,
            clientIdentity: clientIdentity,
            clientDeviceId: clientDeviceId,
            udpProbeHost: udpProbeHost,
            udpProbePort: udpProbePort
        )
        return self.activeRouteMode
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

    /// Sends an E2EE `PeerMessage` JSON `Data` payload over the active Direct P2P connection
    /// and returns the decrypted JSON `Data` reply (`Sendable` across Swift 6 actor boundaries).
    public func requestOverDirectP2P(
        _ rawMsg: Data,
        clientIdentity: CodaDeviceIdentity,
        clientDeviceId: UUID,
        targetDeviceId: UUID
    ) async throws -> Data {
        guard let conn = self.directP2PConnection, var sess = self.directE2EESession else {
            throw URLError(.networkConnectionLost)
        }
        let sealed = try sess.sealJSON(rawMsg)

        let packetData = try Self.encodeLengthDelimitedJSON([
            "frame_type": "route_packet",
            "src_device_id": clientDeviceId.uuidString.lowercased(),
            "dst_device_id": targetDeviceId.uuidString.lowercased(),
            "lane": "control_p0",
            "e2ee": [
                "seq": sealed.seq,
                "compressed": sealed.compressed,
                "ciphertext_b64": sealed.ciphertextB64,
            ],
            "sender_x25519_pub_b64": clientIdentity.x25519PublicB64,
        ])

        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Void, Error>) in
            let box = OnceResume<Void>(cont)
            conn.send(content: packetData, completion: .contentProcessed { err in
                if let err {
                    box.fail(err)
                } else {
                    box.succeed(())
                }
            })
        }

        while true {
            let replyFrame = try await Self.readLengthDelimitedFrame(conn)
            if (replyFrame["frame_type"] as? String) == "route_packet",
               let e2eeDict = replyFrame["e2ee"] as? [String: Any],
               let e2eeData = try? JSONSerialization.data(withJSONObject: e2eeDict) {
                let encPayload = try JSONDecoder().decode(CodaEncryptedPayload.self, from: e2eeData)
                let pt = try sess.openJSON(encPayload)
                self.directE2EESession = sess
                return pt
            }
        }
    }
}
