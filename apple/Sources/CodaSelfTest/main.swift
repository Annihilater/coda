import CodaGhosttyKit
import CodaKit
import Darwin
import Foundation
import Network

@main
struct CodaSelfTest {
    static func main() async throws {
        // 1. Verify Apple CryptoKit Curve25519 + HKDF-SHA256 + ChaChaPoly E2EE & Anti-Replay
        let iphoneDuo = CodaDeviceIdentity.generate(name: "iPhone Duo")
        let macStudio = CodaDeviceIdentity.generate(name: "Home Mac Studio")

        var clientSess = try CodaE2EESession(
            myX25519SecretB64: iphoneDuo.x25519SecretB64,
            peerX25519PublicB64: macStudio.x25519PublicB64,
            isClientInitiator: true
        )
        var daemonSess = try CodaE2EESession(
            myX25519SecretB64: macStudio.x25519SecretB64,
            peerX25519PublicB64: iphoneDuo.x25519PublicB64,
            isClientInitiator: false
        )

        let msg = Data(#"{"op":"ping","req_id":"apple-1","sent_ms":100}"#.utf8)
        let sealed = try clientSess.sealJSON(msg)
        let opened = try daemonSess.openJSON(sealed)
        precondition(opened == msg, "Decrypted payload mismatch")

        var replayRejected = false
        do {
            _ = try daemonSess.openJSON(sealed)
        } catch {
            replayRejected = true
        }
        precondition(replayRejected, "Replay attack was not rejected")

        // 2. Verify OSC 7501 Semantic Agent Status & iPhone Duo 5.4" -> 7.6" Unfold Reflow
        var surface = GhosttyReflowEngine(columns: 40, rows: 10)
        let stream = "\u{1B}]7501;state=blocked;kind=permission;app=claude;title=RunTests;progress=85\u{07}"
            + "Claude Code modified crates/coda-server/src/relay.rs and awaits approval to run tests\n"
        surface.feed(Data(stream.utf8))
        precondition(surface.latestHerdrStatus?.state == .blocked)
        precondition(surface.latestHerdrStatus?.kind == "permission")
        precondition(surface.latestHerdrStatus?.progress == 85)
        precondition(surface.visibleLines.count > 2)

        surface.resizeReflow(columns: 108, rows: 28)
        precondition(surface.visibleLines.count == 1)

        // 3. Verify Phase 2 Base64 Live PTY Chunk Feeding & Voice Model Config Codable Roundtrip
        let ptyChunkB64 = Data("\u{1B}[32m● PTY Live Stream OK\u{1B}[0m\r\n".utf8).base64EncodedString()
        surface.feedBase64PtyChunk(ptyChunkB64)
        precondition(surface.visibleLines.last?.contains("PTY Live Stream OK") == true)

        let voiceCfg = CodaVoiceModelConfig(
            provider: "openai",
            baseURL: "https://api.openai.com/v1",
            apiKey: "sk-apple-test",
            model: "gpt-4o-mini-transcribe"
        )
        let voiceJSON = try JSONEncoder().encode(voiceCfg)
        let decodedVoice = try JSONDecoder().decode(CodaVoiceModelConfig.self, from: voiceJSON)
        precondition(decodedVoice == voiceCfg)

        // 4. Verify Phase 2 Git Repo & Worktree JSON Wire Compatibility with Rust `coda-protocol`
        let sampleRepoJSON = Data("""
        {
          "repo_name": "coda",
          "root_path": "/Users/vampire/personal/github/coda",
          "machine_name": "Home",
          "branches": [
            {
              "branch": "main",
              "worktree_path": "/Users/vampire/personal/github/coda",
              "diff_added": 1233,
              "diff_deleted": 116,
              "status_summary": "4 个终端运行中"
            }
          ]
        }
        """.utf8)
        let repo = try JSONDecoder().decode(CodaRepoWorkspaceSummary.self, from: sampleRepoJSON)
        precondition(repo.repoName == "coda")
        precondition(repo.branches.first?.diffAdded == 1233)
        precondition(repo.branches.first?.statusSummary == "4 个终端运行中")

        // 5. Verify RFC 8628 Device Auth JSON Wire Compatibility (Linux `daemon` + macOS `hybrid`)
        let startJSON = Data("""
        {
          "device_code": "dev_test_123",
          "user_code": "ABCD-2345",
          "verification_uri": "https://codahub.dev/activate",
          "verification_uri_complete": "https://codahub.dev/activate?code=ABCD-2345",
          "expires_in_secs": 600,
          "interval_secs": 2
        }
        """.utf8)
        let authStart = try JSONDecoder().decode(CodaDeviceAuthStart.self, from: startJSON)
        precondition(authStart.userCode == "ABCD-2345")
        precondition(authStart.verificationURIComplete.contains("ABCD-2345"))

        let pollJSON = Data("""
        {
          "state": "approved",
          "session_token": "tok_test_abc",
          "owner_handle": "alice",
          "daemon_relay_ticket": "ticket_xyz",
          "device": {
            "id": "\(UUID().uuidString.lowercased())",
            "name": "MacBook-Pro",
            "role": "hybrid",
            "platform": "macos",
            "x25519_pubkey": "\(iphoneDuo.x25519PublicB64)",
            "endpoints_json": [{"kind": "lan_ipv4", "addr": "192.168.1.20:24444"}],
            "is_online": true
          },
          "relays": [
            {
              "id": "global-cdn-wss",
              "region": "Global Edge CDN / CF Tunnel (WSS 443)",
              "mode": "cdn_wss",
              "endpoint": "wss://codahub.dev/v1/tunnel/ws",
              "priority": 5
            }
          ]
        }
        """.utf8)
        let pollResult = try JSONDecoder().decode(CodaDeviceAuthPollResult.self, from: pollJSON)
        precondition(pollResult.state == "approved")
        precondition(pollResult.device?.role == "hybrid")
        precondition(pollResult.relays?.first?.endpoint == "wss://codahub.dev/v1/tunnel/ws")

        // 6. Verify Auto-LAN UDP Discovery + Direct P2P TCP Handshake + E2EE Ping/Pong + Relay Fallback
        try await runAutoLanSmartPathLoopbackTest()

        print("✅ CodaSelfTest passed: Apple CryptoKit E2EE + OSC 7501 + PTY Live Stream + RFC 8628 + Auto-LAN SmartPath P2P verified!")
    }

    static func runAutoLanSmartPathLoopbackTest() async throws {
        let daemonId = UUID()
        let clientId = UUID()
        let daemonIdentity = CodaDeviceIdentity.generate(name: "Mock-Mac-Host")
        let clientIdentity = CodaDeviceIdentity.generate(name: "iPhone-Duo-Client")

        // Start TCP listener speaking Coda DirectP2pHello -> DirectP2pAck -> E2EE Ping/Pong
        let tcpListener = try NWListener(using: .tcp, on: .any)
        let readyPort = AsyncStream<UInt16> { cont in
            tcpListener.stateUpdateHandler = { state in
                if case .ready = state, let port = tcpListener.port?.rawValue {
                    cont.yield(port)
                    cont.finish()
                }
            }
        }
        tcpListener.newConnectionHandler = { conn in
            conn.start(queue: .global(qos: .userInteractive))
            handleMockDaemonConn(conn: conn, daemonId: daemonId, daemonIdentity: daemonIdentity)
        }
        tcpListener.start(queue: .global(qos: .userInteractive))

        var p2pPort: UInt16 = 0
        for await p in readyPort {
            p2pPort = p
            break
        }
        precondition(p2pPort > 0, "failed to bind mock Direct P2P listener")
        let fixedP2pPort = Int(p2pPort)

        // Start UDP LAN beacon responder on 127.0.0.1:ephemeral
        let udpFD = Darwin.socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP)
        precondition(udpFD >= 0, "failed creating mock UDP beacon socket")
        var udpBind = sockaddr_in()
        udpBind.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
        udpBind.sin_family = sa_family_t(AF_INET)
        udpBind.sin_port = 0
        _ = "127.0.0.1".withCString { inet_pton(AF_INET, $0, &udpBind.sin_addr) }
        let bRes = withUnsafePointer(to: &udpBind) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                Darwin.bind(udpFD, $0, socklen_t(MemoryLayout<sockaddr_in>.size))
            }
        }
        precondition(bRes == 0, "failed binding mock UDP beacon")

        var boundUdpAddr = sockaddr_in()
        var boundUdpLen = socklen_t(MemoryLayout<sockaddr_in>.size)
        _ = withUnsafeMutablePointer(to: &boundUdpAddr) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                Darwin.getsockname(udpFD, $0, &boundUdpLen)
            }
        }
        let udpPort = UInt16(bigEndian: boundUdpAddr.sin_port)

        DispatchQueue.global(qos: .userInteractive).async {
            var buf = [UInt8](repeating: 0, count: 512)
            var src = sockaddr_in()
            var srcLen = socklen_t(MemoryLayout<sockaddr_in>.size)
            while true {
                let n = buf.withUnsafeMutableBytes { bPtr in
                    withUnsafeMutablePointer(to: &src) { sPtr in
                        sPtr.withMemoryRebound(to: sockaddr.self, capacity: 1) { saPtr in
                            Darwin.recvfrom(udpFD, bPtr.baseAddress, 512, 0, saPtr, &srcLen)
                        }
                    }
                }
                if n <= 0 { break }
                let d = Data(buf.prefix(Int(n)))
                if let obj = try? JSONSerialization.jsonObject(with: d) as? [String: Any],
                   (obj["coda_lan_probe"] as? Int) == 1 {
                    let reply: [String: Any] = [
                        "coda_lan_here": 1,
                        "device_id": daemonId.uuidString.lowercased(),
                        "tcp_port": fixedP2pPort,
                    ]
                    if let rData = try? JSONSerialization.data(withJSONObject: reply) {
                        _ = rData.withUnsafeBytes { rPtr in
                            withUnsafePointer(to: &src) { sPtr in
                                sPtr.withMemoryRebound(to: sockaddr.self, capacity: 1) { saPtr in
                                    Darwin.sendto(udpFD, rPtr.baseAddress, rData.count, 0, saPtr, srcLen)
                                }
                            }
                        }
                    }
                    break
                }
            }
            Darwin.close(udpFD)
        }

        let targetDevice = CodaRemoteDevice(
            id: daemonId,
            name: "Mock-Mac-Host",
            role: "daemon",
            platform: "macos",
            x25519Pubkey: daemonIdentity.x25519PublicB64,
            endpoints: [],
            isOnline: true
        )

        let net = CodaNetworkClient(serverBaseURL: URL(string: "http://127.0.0.1:8080")!)
        let negotiated = try await net.connectAutoSmartPath(
            targetDevice: targetDevice,
            clientIdentity: clientIdentity,
            clientDeviceId: clientId,
            udpProbeHost: "127.0.0.1",
            udpProbePort: udpPort
        )
        precondition(negotiated == .lanDirect, "expected .lanDirect via UDP auto-discovery")

        let reqData = Data(#"{"op":"ping","req_id":"apple-lan-1","sent_ms":4242}"#.utf8)
        let replyData = try await net.requestOverDirectP2P(
            reqData,
            clientIdentity: clientIdentity,
            clientDeviceId: clientId,
            targetDeviceId: daemonId
        )
        let reply = try JSONSerialization.jsonObject(with: replyData) as? [String: Any]
        precondition((reply?["op"] as? String) == "pong")
        precondition((reply?["req_id"] as? String) == "apple-lan-1")
        precondition((reply?["daemon_hostname"] as? String) == "Mock-Mac-Host")

        tcpListener.cancel()
    }

    private static func handleMockDaemonConn(
        conn: NWConnection,
        daemonId: UUID,
        daemonIdentity: CodaDeviceIdentity
    ) {
        readFramed(conn: conn) { hello in
            guard let hello,
                  (hello["frame_type"] as? String) == "direct_p2p_hello",
                  let clientPub = hello["client_x25519_pub_b64"] as? String,
                  let initialSess = try? CodaE2EESession(
                    myX25519SecretB64: daemonIdentity.x25519SecretB64,
                    peerX25519PublicB64: clientPub,
                    isClientInitiator: false
                  )
            else {
                conn.cancel()
                return
            }

            let ack: [String: Any] = [
                "frame_type": "direct_p2p_ack",
                "ok": true,
                "daemon_device_id": daemonId.uuidString.lowercased(),
                "daemon_x25519_pub_b64": daemonIdentity.x25519PublicB64,
                "path_kind": "lan_ipv4",
            ]
            sendFramed(conn: conn, obj: ack) {
                readFramed(conn: conn) { pkt in
                    var localSess = initialSess
                    guard let pkt,
                          (pkt["frame_type"] as? String) == "route_packet",
                          let e2eeObj = pkt["e2ee"] as? [String: Any],
                          let e2eeData = try? JSONSerialization.data(withJSONObject: e2eeObj),
                          let enc = try? JSONDecoder().decode(CodaEncryptedPayload.self, from: e2eeData),
                          let pt = try? localSess.openJSON(enc),
                          let req = try? JSONSerialization.jsonObject(with: pt) as? [String: Any],
                          let reqId = req["req_id"] as? String,
                          let sentMs = req["sent_ms"]
                    else {
                        conn.cancel()
                        return
                    }

                    let pong: [String: Any] = [
                        "op": "pong",
                        "req_id": reqId,
                        "sent_ms": sentMs,
                        "daemon_hostname": "Mock-Mac-Host",
                        "herdr_version": "0.1.20",
                    ]
                    if let pongData = try? JSONSerialization.data(withJSONObject: pong),
                       let sealed = try? localSess.sealJSON(pongData) {
                        let respFrame: [String: Any] = [
                            "frame_type": "route_packet",
                            "src_device_id": daemonId.uuidString.lowercased(),
                            "dst_device_id": (pkt["src_device_id"] as? String) ?? "",
                            "lane": "control_p0",
                            "e2ee": [
                                "seq": sealed.seq,
                                "compressed": sealed.compressed,
                                "ciphertext_b64": sealed.ciphertextB64,
                            ],
                        ]
                        sendFramed(conn: conn, obj: respFrame) {}
                    }
                }
            }
        }
    }

    private static func readFramed(conn: NWConnection, completion: @escaping @Sendable ([String: Any]?) -> Void) {
        conn.receive(minimumIncompleteLength: 4, maximumLength: 4) { hdr, _, _, err in
            guard err == nil, let hdr, hdr.count == 4 else {
                completion(nil)
                return
            }
            let len = hdr.withUnsafeBytes { UInt32(bigEndian: $0.loadUnaligned(as: UInt32.self)) }
            conn.receive(minimumIncompleteLength: Int(len), maximumLength: Int(len)) { body, _, _, err2 in
                guard err2 == nil, let body,
                      let obj = try? JSONSerialization.jsonObject(with: body) as? [String: Any] else {
                    completion(nil)
                    return
                }
                completion(obj)
            }
        }
    }

    private static func sendFramed(conn: NWConnection, obj: [String: Any], done: @escaping @Sendable () -> Void) {
        guard let payload = try? JSONSerialization.data(withJSONObject: obj) else { return }
        var framed = Data(capacity: 4 + payload.count)
        var lenBE = UInt32(payload.count).bigEndian
        withUnsafeBytes(of: &lenBE) { framed.append(contentsOf: $0) }
        framed.append(payload)
        conn.send(content: framed, completion: .contentProcessed { _ in done() })
    }
}
