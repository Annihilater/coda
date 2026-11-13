import CodaGhosttyKit
import CodaKit
import Darwin
import Foundation
import Network

@main
struct CodaSelfTest {
    static func main() async throws {
        // 1. Apple CryptoKit Curve25519 + HKDF-SHA256 + ChaChaPoly E2EE & Anti-Replay
        let iphoneClient = CodaDeviceIdentity.generate(name: "iPhone Client")
        let studioHost = CodaDeviceIdentity.generate(name: "Studio Host")

        var clientSess = try CodaE2EESession(
            myX25519SecretB64: iphoneClient.x25519SecretB64,
            peerX25519PublicB64: studioHost.x25519PublicB64,
            isClientInitiator: true
        )
        var daemonSess = try CodaE2EESession(
            myX25519SecretB64: studioHost.x25519SecretB64,
            peerX25519PublicB64: iphoneClient.x25519PublicB64,
            isClientInitiator: false
        )

        let msg = Data(#"{"op":"ping","req_id":"swift-1","sent_ms":100}"#.utf8)
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

        // 2. OSC 7501 Semicolon + Colon Formats & Foldable Width Reflow
        var surface = GhosttyReflowEngine(columns: 40, rows: 10)
        let stream = "\u{1B}]7501;state=blocked;kind=permission;app=claude;title=RunTests;progress=85\u{07}"
            + "Claude Code modified crates/coda-server/src/relay.rs and awaits approval to run tests\n"
        surface.feed(Data(stream.utf8))
        precondition(surface.latestHerdrStatus?.state == .needsYou)
        precondition(surface.latestHerdrStatus?.kind == "permission")
        precondition(surface.latestHerdrStatus?.progress == 85)
        precondition(surface.visibleLines.count > 2)

        surface.resizeReflow(columns: 108, rows: 28)
        precondition(surface.visibleLines.count == 1)

        // 3. TrueColor 24-bit ANSI SGR + Git Diff Classification + Colon OSC 7501 + Tactile Chords
        let ptyChunkB64 = Data("\u{1B}[32m* PTY Live Stream OK\u{1B}[0m\r\n".utf8).base64EncodedString()
        surface.feedBase64PtyChunk(ptyChunkB64)
        precondition(surface.visibleLines.last?.contains("PTY Live Stream OK") == true)

        let richRaw = "\u{1B}]7501:blocked:Approve deploy\u{07}\u{1B}[38;2;56;189;248m+added diff line\u{1B}[0m\n-removed diff line\n1. Yes\n"
        let styled = GhosttyReflowEngine.renderStyledLines(rawOutput: richRaw, targetColumns: 80)
        precondition(styled.latestStatus?.state == .needsYou)
        precondition(styled.latestStatus?.message == "Approve deploy")
        precondition(styled.lines.count == 3)
        precondition(styled.lines[0].kind == .diffAdded)
        precondition(styled.lines[0].spans.first?.fg == GhosttyRGBColor(r: 56, g: 189, b: 248))
        precondition(styled.lines[1].kind == .diffRemoved)
        precondition(styled.lines[2].kind == .promptChoice)
        precondition(GhosttyReflowEngine.encodeKey(.shiftTab) == Data("\u{1B}[Z".utf8))
        precondition(GhosttyReflowEngine.encodeModifiedText("c", ctrl: true, alt: false) == Data([0x03]))

        // 4. Herdr Inbox Priority Ordering (`Needs you` -> `Error` -> `Done (unread)` -> `Working` -> `Idle`)
        let now = Date()
        let sIdle = CodaAgentSession(id: "s4", terminalAddress: "t4", workspaceName: "coda", hostName: "studio", agentName: "zsh", taskTitle: "Shell", state: .idle, updatedAt: now)
        let sWork = CodaAgentSession(id: "s3", terminalAddress: "t3", workspaceName: "coda", hostName: "studio", agentName: "codex", taskTitle: "Build", state: .working, updatedAt: now.addingTimeInterval(-10))
        let sDoneUnread = CodaAgentSession(id: "s2", terminalAddress: "t2", workspaceName: "coda", hostName: "studio", agentName: "claude", taskTitle: "Done task", state: .done, unreadCompletion: true, updatedAt: now.addingTimeInterval(-20))
        let sNeeds = CodaAgentSession(id: "s1", terminalAddress: "t1", workspaceName: "coda", hostName: "studio", agentName: "claude", taskTitle: "Approve tool", state: .needsYou, updatedAt: now.addingTimeInterval(-30))
        let sortedIDs = [sIdle, sWork, sDoneUnread, sNeeds].sorted(by: CodaAgentSession.inboxOrder).map(\.id)
        precondition(sortedIDs == ["s1", "s2", "s3", "s4"], "Unexpected inbox priority order: \(sortedIDs)")

        // 5. Wire Codable Compatibility Checks
        let voiceCfg = CodaVoiceModelConfig(
            provider: "openai",
            baseURL: "https://api.openai.com/v1",
            apiKey: "test-placeholder-token",
            model: "gpt-4o-mini-transcribe"
        )
        let voiceJSON = try JSONEncoder().encode(voiceCfg)
        let decodedVoice = try JSONDecoder().decode(CodaVoiceModelConfig.self, from: voiceJSON)
        precondition(decodedVoice == voiceCfg)

        let sampleRepoJSON = Data("""
        {
          "repo_name": "coda",
          "root_path": "/Users/dev/coda",
          "machine_name": "Studio",
          "branches": [
            {
              "branch": "main",
              "worktree_path": "/Users/dev/coda",
              "diff_added": 124,
              "diff_deleted": 16,
              "status_summary": "2 panes working"
            }
          ]
        }
        """.utf8)
        let repo = try JSONDecoder().decode(CodaRepoWorkspaceSummary.self, from: sampleRepoJSON)
        precondition(repo.repoName == "coda" && repo.branches.first?.diffAdded == 124)

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

        // 6. Auto-LAN UDP Discovery + Direct P2P TCP Handshake + E2EE ListTerminals / SendTerminalText RPCs
        try await runAutoLanSmartPathLoopbackTest()

        print("[OK] ALL CODA APPLE NATIVE SELF-TESTS PASSED!")
    }

    static func runAutoLanSmartPathLoopbackTest() async throws {
        let daemonId = UUID()
        let clientId = UUID()
        let daemonIdentity = CodaDeviceIdentity.generate(name: "Loopback-Host")
        let clientIdentity = CodaDeviceIdentity.generate(name: "Loopback-iPhone")

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
        precondition(p2pPort > 0, "failed to bind loopback Direct P2P listener")
        let fixedP2pPort = Int(p2pPort)

        let udpFD = Darwin.socket(AF_INET, SOCK_DGRAM, IPPROTO_UDP)
        precondition(udpFD >= 0, "failed creating UDP beacon socket")
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
        precondition(bRes == 0, "failed binding UDP beacon")

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
            name: "Loopback-Host",
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

        // Verify raw E2EE Ping/Pong
        let reqData = Data(#"{"op":"ping","req_id":"swift-ping-1","sent_ms":4242}"#.utf8)
        let replyData = try await net.requestOverDirectP2P(
            reqData,
            clientIdentity: clientIdentity,
            clientDeviceId: clientId,
            targetDeviceId: daemonId
        )
        let reply = try JSONSerialization.jsonObject(with: replyData) as? [String: Any]
        precondition((reply?["op"] as? String) == "pong")
        precondition((reply?["daemon_hostname"] as? String) == "Loopback-Host")

        // Verify high-level typed RPC `listTerminals` and `sendTerminalText`
        let terms = try await net.listTerminals(
            on: targetDevice,
            clientIdentity: clientIdentity,
            clientDeviceId: clientId
        )
        precondition(terms.count == 1 && terms.first?.address == "t1")
        precondition(terms.first?.status?.state == "blocked")

        try await net.sendTerminalText(
            on: targetDevice,
            clientIdentity: clientIdentity,
            clientDeviceId: clientId,
            terminal: "t1",
            text: "cargo check",
            submit: true
        )

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
                servePacketLoop(conn: conn, daemonId: daemonId, sess: initialSess)
            }
        }
    }

    private static func servePacketLoop(conn: NWConnection, daemonId: UUID, sess: CodaE2EESession) {
        readFramed(conn: conn) { pkt in
            var nextSess = sess
            guard let pkt,
                  (pkt["frame_type"] as? String) == "route_packet",
                  let e2eeObj = pkt["e2ee"] as? [String: Any],
                  let e2eeData = try? JSONSerialization.data(withJSONObject: e2eeObj),
                  let enc = try? JSONDecoder().decode(CodaEncryptedPayload.self, from: e2eeData),
                  let pt = try? nextSess.openJSON(enc),
                  let req = try? JSONSerialization.jsonObject(with: pt) as? [String: Any],
                  let op = req["op"] as? String,
                  let reqId = req["req_id"] as? String
            else {
                conn.cancel()
                return
            }

            let replyObj: [String: Any]
            switch op {
            case "ping":
                replyObj = [
                    "op": "pong",
                    "req_id": reqId,
                    "sent_ms": req["sent_ms"] ?? 0,
                    "daemon_hostname": "Loopback-Host",
                    "herdr_version": "0.1.20",
                ]
            case "list_terminals":
                replyObj = [
                    "op": "terminals_snapshot",
                    "req_id": reqId,
                    "core_name": "loopback",
                    "core_id": "core-1",
                    "terminals": [
                        [
                            "address": "t1",
                            "name": "Auth refactor",
                            "argv": ["claude"],
                            "cwd": "/Users/dev/coda",
                            "cols": 100,
                            "rows": 32,
                            "alive": true,
                            "status": [
                                "state": "blocked",
                                "kind": "permission",
                                "app": "claude",
                                "title": "Run cargo test",
                                "msg": "Allow running tests?",
                            ],
                        ]
                    ],
                ]
            case "send_terminal_text":
                replyObj = [
                    "op": "action_ack",
                    "req_id": reqId,
                    "ok": true,
                    "detail": "text_sent",
                ]
            default:
                replyObj = [
                    "op": "action_ack",
                    "req_id": reqId,
                    "ok": true,
                    "detail": "ok",
                ]
            }

            guard let respData = try? JSONSerialization.data(withJSONObject: replyObj),
                  let sealed = try? nextSess.sealJSON(respData) else {
                conn.cancel()
                return
            }

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
            let updatedSess = nextSess
            sendFramed(conn: conn, obj: respFrame) {
                servePacketLoop(conn: conn, daemonId: daemonId, sess: updatedSess)
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
