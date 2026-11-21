import CodaGhosttyKit
import CodaKit
import CryptoKit
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

        // 6. iPhone Duo (466x678, 890x626 / 626x890 with trailing 84pt system rail) & Mainstream iPhone 14-18 Form-Factor Matrix + Fold Reflow
        precondition(CodaIPhoneDeviceSpec.mainstreamCatalog.count == 23)
        for spec in CodaIPhoneDeviceSpec.mainstreamCatalog {
            let portrait = CodaFormFactor.detect(
                width: spec.logicalSize.width,
                height: spec.logicalSize.height,
                isHorizontalRegular: spec.isHorizontalRegularInPortrait,
                isVerticalRegular: spec.isVerticalRegularInPortrait
            )
            precondition(
                portrait == spec.expectedPortraitFormFactor,
                "Portrait form factor mismatch for \(spec.name): got \(portrait)"
            )

            let landscape = CodaFormFactor.detect(
                width: spec.logicalSize.height,
                height: spec.logicalSize.width,
                isHorizontalRegular: spec.isHorizontalRegularInLandscape,
                isVerticalRegular: spec.isVerticalRegularInLandscape
            )
            precondition(
                landscape == spec.expectedLandscapeFormFactor,
                "Landscape form factor mismatch for \(spec.name): got \(landscape)"
            )
            precondition(landscape.usesVerticalSideRail)

            if portrait.isIPhoneDuo {
                precondition(portrait.usesVerticalSideRail && landscape.usesVerticalSideRail)
                precondition(portrait.placesSideRailOnTrailingEdge && landscape.placesSideRailOnTrailingEdge)
                precondition(spec.portraitSafeArea.trailing == CodaEdgeSafeAreas.duoSideRailWidth)
                precondition(spec.landscapeSafeArea.trailing == CodaEdgeSafeAreas.duoSideRailWidth)
                precondition(spec.portraitSafeArea.top == 0 && spec.landscapeSafeArea.top == 0)
                precondition(spec.portraitSafeArea.isHorizontallyAsymmetric && spec.landscapeSafeArea.isHorizontallyAsymmetric)
            } else {
                precondition(!portrait.usesVerticalSideRail)
                precondition(!portrait.placesSideRailOnTrailingEdge && !landscape.placesSideRailOnTrailingEdge)
                precondition(!spec.portraitSafeArea.isHorizontallyAsymmetric)
            }

            let expectedPlatform = (portrait.isIPhoneDuo || landscape.isIPhoneDuo) ? "iphone_duo" : "ios"
            precondition(portrait.platformIdentifier(forMacOS: false) == expectedPlatform)
            precondition(landscape.platformIdentifier(forMacOS: false) == expectedPlatform)
        }

        precondition(CodaFormFactor.detect(width: 466, height: 678) == .iphoneDuoCoverPortrait)
        precondition(CodaFormFactor.detect(width: 678, height: 466) == .iphoneDuoCoverLandscape)
        precondition(CodaFormFactor.detect(width: 890, height: 626, isHorizontalRegular: true, isVerticalRegular: true) == .iphoneDuoInnerLandscape)
        precondition(CodaFormFactor.detect(width: 626, height: 890, isHorizontalRegular: true, isVerticalRegular: true) == .iphoneDuoInnerPortrait)
        precondition(CodaFormFactor.detect(width: 951, height: 669, isHorizontalRegular: true, isVerticalRegular: true) == .iphoneDuoInnerLandscape)
        precondition(CodaFormFactor.detect(width: 669, height: 951, isHorizontalRegular: true, isVerticalRegular: true) == .iphoneDuoInnerPortrait)
        precondition(!CodaFormFactor.iphoneDuoInnerLandscape.isCompact && CodaFormFactor.iphoneDuoInnerLandscape.isIPhoneDuo)
        precondition(CodaFormFactor.detect(width: 1194, height: 834, isHorizontalRegular: true, isVerticalRegular: true) == .ipadOrDesktop)
        precondition(CodaFormFactor.ipadOrDesktop.platformIdentifier(forMacOS: false) == "ipados")

        if let coverRightRail = CodaReservedRegion.duoRightSystemRail(in: CGSize(width: 466, height: 678)) {
            precondition(coverRightRail.kind == .occlusion)
            precondition(coverRightRail.rect == CGRect(x: 382, y: 0, width: 84, height: 678))
        } else {
            preconditionFailure("Expected right system rail region for Duo Cover 466x678")
        }
        if let innerRightRail = CodaReservedRegion.duoRightSystemRail(in: CGSize(width: 890, height: 626)) {
            precondition(innerRightRail.kind == .occlusion)
            precondition(innerRightRail.rect == CGRect(x: 806, y: 0, width: 84, height: 626))
        } else {
            preconditionFailure("Expected right system rail region for Duo Inner 890x626")
        }

        if let landscapeCrease = CodaReservedRegion.duoFoldDivision(in: CGSize(width: 890, height: 626), posture: .bookHalfOpen) {
            precondition(landscapeCrease.kind == .division)
            precondition(landscapeCrease.rect == CGRect(x: 433, y: 0, width: 24, height: 626))
            let leftSpan = landscapeCrease.rect.minX
            let creaseSpan = landscapeCrease.rect.width
            let rightRailSpan = CodaEdgeSafeAreas.duoSideRailWidth
            let rightLeafSpan = 890 - landscapeCrease.rect.maxX - rightRailSpan
            precondition(leftSpan == 433 && creaseSpan == 24 && rightLeafSpan == 349 && rightRailSpan == 84)
            precondition(leftSpan + creaseSpan + rightLeafSpan + rightRailSpan == 890)
        } else {
            preconditionFailure("Expected vertical fold crease division for Duo inner landscape")
        }
        precondition(CodaReservedRegion.duoFoldDivision(in: CGSize(width: 626, height: 890), posture: .flat) == nil)
        if let tabletopCrease = CodaReservedRegion.duoFoldDivision(in: CGSize(width: 626, height: 890), posture: .tabletopLaptop) {
            precondition(tabletopCrease.kind == .division)
            precondition(tabletopCrease.rect == CGRect(x: 0, y: 433, width: 626, height: 24))
        } else {
            preconditionFailure("Expected horizontal fold crease division for Duo tabletop pose")
        }
        precondition(CodaEdgeSafeAreas(top: 0, leading: 0, bottom: 34, trailing: 84).isHorizontallyAsymmetric)
        precondition(!CodaEdgeSafeAreas(top: 0, leading: 59, bottom: 21, trailing: 59).isHorizontallyAsymmetric)

        let duoSample = "\u{1B}]7501:blocked:Run suite\u{07}\u{1B}[38;2;56;189;248m+    let prefer_raw = self.peer_lacks_zstd || req_id.starts_with(\"swift-\");\u{1B}[0m\n-    let old = true;\n1. Yes, approve cargo test\n"
        let cols48Styled = GhosttyReflowEngine.renderStyledLines(rawOutput: duoSample, targetColumns: 48)
        let cols56Styled = GhosttyReflowEngine.renderStyledLines(rawOutput: duoSample, targetColumns: 56)
        let cols58Styled = GhosttyReflowEngine.renderStyledLines(rawOutput: duoSample, targetColumns: 58)
        let cols80Styled = GhosttyReflowEngine.renderStyledLines(rawOutput: duoSample, targetColumns: 80)
        let panoStyled = GhosttyReflowEngine.renderStyledLines(rawOutput: duoSample, targetColumns: 98)
        precondition(cols48Styled.lines.count >= cols56Styled.lines.count)
        precondition(cols56Styled.lines.count >= cols58Styled.lines.count)
        precondition(cols58Styled.lines.count >= cols80Styled.lines.count)
        precondition(cols80Styled.lines.count >= panoStyled.lines.count)
        precondition(cols48Styled.lines.count > panoStyled.lines.count)
        precondition(cols48Styled.latestStatus?.state == .needsYou && panoStyled.latestStatus?.state == .needsYou)
        precondition(panoStyled.lines.contains(where: { $0.kind == .diffAdded }) && panoStyled.lines.contains(where: { $0.kind == .diffRemoved }))
        precondition(panoStyled.lines.contains(where: { $0.kind == .promptChoice }))
        precondition(GhosttyReflowEngine.encodeKey(.backspace) == Data([0x7F]))
        precondition(HerdrQuickKey.backspace.herdrKeyNames == ["backspace"])
        precondition(HerdrQuickKey.esc.herdrKeyNames == ["esc"])
        precondition(HerdrQuickKey.shiftTab.herdrKeyNames == ["shift+tab"])
        precondition(HerdrQuickKey.ctrlC.herdrKeyNames == ["ctrl+c"])
        precondition(GhosttyReflowEngine.encodeKey(.slash) == Data("/".utf8))

        // 7. Auto-LAN UDP Discovery + Direct P2P TCP Handshake + One-Way PtyResize + E2EE RPCs
        try await runAutoLanSmartPathLoopbackTest()

        // 8. Security, Hybrid/Relay v2 E2EE, Rust FFI Zstd Interop, Keychain/0600 Secrets, Stateful VT & Path Guards
        try runSecurityFFIAndVTAssertions()

        // 9. Optional Live Multi-Host Mesh Test (`CODA_LIVE_MESH_TEST_URL`)
        if let liveURL = ProcessInfo.processInfo.environment["CODA_LIVE_MESH_TEST_URL"], !liveURL.isEmpty {
            try await runLiveMeshSelfTest(baseURLString: liveURL)
        }

        print("[OK] ALL CODA APPLE NATIVE SELF-TESTS PASSED!")
    }

    private final class LiveMeshCaptureBox: @unchecked Sendable {
        private let lock = NSLock()
        private var chunks: [String: String] = [:]
        private var wsEvents: [CodaAccountLiveEvent] = []

        func appendChunk(deviceId: UUID, terminal: String, text: String) {
            let key = "\(deviceId.uuidString.lowercased())/\(terminal)"
            lock.lock()
            chunks[key, default: ""] += text
            lock.unlock()
        }

        func output(deviceId: UUID, terminal: String) -> String {
            let key = "\(deviceId.uuidString.lowercased())/\(terminal)"
            lock.lock()
            let v = chunks[key] ?? ""
            lock.unlock()
            return v
        }

        func appendEvent(_ ev: CodaAccountLiveEvent) {
            lock.lock()
            wsEvents.append(ev)
            lock.unlock()
        }

        func hasConnectedEvent() -> Bool {
            lock.lock()
            let ok = wsEvents.contains { $0.kind == "connected" }
            lock.unlock()
            return ok
        }
    }

    static func runLiveMeshSelfTest(baseURLString: String) async throws {
        guard let url = URL(string: baseURLString) else {
            preconditionFailure("Invalid CODA_LIVE_MESH_TEST_URL: \(baseURLString)")
        }
        let email = ProcessInfo.processInfo.environment["CODA_LIVE_MESH_EMAIL"] ?? "mesh@coda.dev"
        let password = ProcessInfo.processInfo.environment["CODA_LIVE_MESH_PASSWORD"] ?? "MeshPass2026!"
        let minHosts = Int(ProcessInfo.processInfo.environment["CODA_EXPECTED_MIN_HOSTS"] ?? "1") ?? 1

        var clientIdentity = CodaDeviceIdentity.generate(name: "Swift-Mobile-Verifier")
        let net = CodaNetworkClient(serverBaseURL: url)
        let token = try await net.login(emailOrHandle: email, password: password, clientLabel: "Swift-SelfTest")
        let registeredDev = try await net.registerClientDevice(
            identity: clientIdentity,
            platform: CodaFormFactor.iphoneDuoInnerLandscape.platformIdentifier(forMacOS: false)
        )
        clientIdentity.deviceId = registeredDev.id
        guard let clientDevId = clientIdentity.deviceId else {
            preconditionFailure("registerClientDevice returned nil device id")
        }

        let capture = LiveMeshCaptureBox()
        await net.setAsyncEventHandler { event in
            if event.op == "pty_output_chunk",
               let obj = try? JSONSerialization.jsonObject(with: event.payloadData) as? [String: Any],
               let term = obj["terminal"] as? String,
               let b64 = obj["data_b64"] as? String,
               let bytes = Data(base64Encoded: b64) {
                capture.appendChunk(
                    deviceId: event.sourceDeviceId,
                    terminal: term,
                    text: String(decoding: bytes, as: UTF8.self)
                )
            }
        }
        await net.connectAccountEventsWebSocket(sessionToken: token) { ev in
            capture.appendEvent(ev)
        }

        var gotConnected = false
        for _ in 0..<50 {
            if capture.hasConnectedEvent() {
                gotConnected = true
                break
            }
            try await Task.sleep(nanoseconds: 100_000_000)
        }
        precondition(gotConnected, "Timed out waiting for /api/v1/events/ws connected event in Swift client")

        var onlineHosts: [CodaRemoteDevice] = []
        for _ in 0..<40 {
            let devices = try await net.listDevices()
            onlineHosts = devices.filter { ($0.role == "daemon" || $0.role == "hybrid") && $0.isOnline }
            if onlineHosts.count >= minHosts {
                break
            }
            try await Task.sleep(nanoseconds: 250_000_000)
        }
        precondition(onlineHosts.count >= minHosts, "Expected at least \(minHosts) online hosts, got \(onlineHosts.count)")

        for host in onlineHosts {
            _ = try await net.connectBestAvailable(
                sessionToken: token,
                targetDevice: host,
                clientIdentity: clientIdentity
            )
            let ws = try await net.listWorkspaces(
                on: host,
                clientIdentity: clientIdentity,
                clientDeviceId: clientDevId
            )
            let launched = try await net.launchAgent(
                on: host,
                clientIdentity: clientIdentity,
                clientDeviceId: clientDevId,
                agent: "sh",
                name: "swift-verify-\(UUID().uuidString.lowercased().prefix(8))",
                cwd: ws.repos.first?.rootPath,
                initialPrompt: nil,
                cols: 100,
                rows: 30
            )
            guard let termAddr = launched?.address else {
                preconditionFailure("launchAgent failed to return terminal address for host \(host.name)")
            }

            let attached = try await net.attachTerminalPty(
                on: host,
                clientIdentity: clientIdentity,
                clientDeviceId: clientDevId,
                terminal: termAddr,
                cols: 100,
                rows: 30
            )
            precondition(attached, "attachTerminalPty failed for host \(host.name)")

            let shortId = String(host.id.uuidString.lowercased().prefix(8))
            let marker = "SWIFT_MOBILE_MESH_OK_\(shortId)"
            try await net.sendPtyInput(
                on: host,
                clientIdentity: clientIdentity,
                clientDeviceId: clientDevId,
                terminal: termAddr,
                data: Data("echo \(marker)\n".utf8)
            )

            var foundMarker = false
            for _ in 0..<80 {
                if capture.output(deviceId: host.id, terminal: termAddr).contains(marker) {
                    foundMarker = true
                    break
                }
                try await Task.sleep(nanoseconds: 100_000_000)
            }
            precondition(foundMarker, "Timed out waiting for \(marker) via pty_output_chunk from \(host.name)")

            if let repo = ws.repos.first {
                let diff = try await net.fetchGitDiff(
                    on: host,
                    clientIdentity: clientIdentity,
                    clientDeviceId: clientDevId,
                    cwd: repo.rootPath
                )
                precondition(!diff.branch.isEmpty, "Empty git diff branch on host \(host.name)")
            }

            try? await net.sendPtyInput(
                on: host,
                clientIdentity: clientIdentity,
                clientDeviceId: clientDevId,
                terminal: termAddr,
                data: Data("exit\n".utf8)
            )
        }

        await net.disconnectTransport()
        print("[OK] CODA APPLE LIVE MESH VERIFIED ACROSS \(onlineHosts.count) HOSTS!")
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
        var udpTimeout = timeval(tv_sec: 5, tv_usec: 0)
        _ = withUnsafePointer(to: &udpTimeout) { ptr in
            Darwin.setsockopt(udpFD, SOL_SOCKET, SO_RCVTIMEO, ptr, socklen_t(MemoryLayout<timeval>.size))
        }
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
            ed25519Pubkey: daemonIdentity.ed25519PublicB64,
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

        try await net.resizeTerminalPty(
            on: targetDevice,
            clientIdentity: clientIdentity,
            clientDeviceId: clientId,
            terminal: "t1",
            cols: 98,
            rows: 34
        )

        try await net.sendTerminalText(
            on: targetDevice,
            clientIdentity: clientIdentity,
            clientDeviceId: clientId,
            terminal: "t1",
            text: "cargo check",
            submit: true
        )

        let loopbackDiff = try await net.fetchRemoteGitDiff(
            device: targetDevice,
            clientIdentity: clientIdentity,
            clientDeviceId: clientId,
            cwd: "/Users/dev/coda"
        )
        precondition(loopbackDiff.branch == "feat/v0.9")
        precondition(loopbackDiff.isGitRepo && loopbackDiff.repoRoot == "/Users/dev/coda")
        precondition(loopbackDiff.changedFiles.count == 2)
        precondition(loopbackDiff.changedFiles[0].path == "src/main.rs")

        let loopbackTranscribe = try await net.transcribeAudio(
            on: targetDevice,
            clientIdentity: clientIdentity,
            clientDeviceId: clientId,
            audioData: Data(repeating: 0x42, count: 128),
            mimeType: "audio/m4a",
            filename: "dictation.m4a",
            language: "en"
        )
        precondition(loopbackTranscribe.ok && loopbackTranscribe.text == "run cargo test --workspace")

        let loopbackBoot = try await net.bootstrapRemoteWorkspace(
            device: targetDevice,
            clientIdentity: clientIdentity,
            clientDeviceId: clientId,
            parentDir: "~/workspace",
            folderName: "demo-repo",
            gitCloneUrl: "https://github.com/org/demo-repo.git"
        )
        precondition(loopbackBoot.ok && loopbackBoot.workspacePath == "/Users/dev/workspace/demo-repo")

        tcpListener.cancel()
    }

    private static func handleMockDaemonConn(
        conn: NWConnection,
        daemonId: UUID,
        daemonIdentity: CodaDeviceIdentity
    ) {
        readFramed(conn: conn) { hello in
            let daemonEphPriv = Curve25519.KeyAgreement.PrivateKey()
            let daemonEphPubB64 = daemonEphPriv.publicKey.rawRepresentation.base64EncodedString()

            guard let hello,
                  (hello["frame_type"] as? String) == "direct_p2p_hello",
                  let clientIdStr = hello["client_device_id"] as? String,
                  let clientDevUUID = UUID(uuidString: clientIdStr),
                  let clientPub = hello["client_x25519_pub_b64"] as? String,
                  let clientEphPub = hello["client_ephemeral_x25519_pub_b64"] as? String,
                  let clientEdPub = hello["client_ed25519_pub_b64"] as? String,
                  let helloTs = (hello["timestamp_ms"] as? NSNumber)?.uint64Value,
                  let helloSig = hello["signature_b64"] as? String,
                  let supportsZstd = (hello["supports_zstd"] as? Bool) ?? Optional(false),
                  CodaDeviceIdentity.verifySignature(
                      ed25519PublicB64: clientEdPub,
                      message: Data("p2p-hello-v2:\(clientIdStr.lowercased()):\(daemonId.uuidString.lowercased()):\(clientPub):\(clientEphPub):\(helloTs):zstd=\(supportsZstd ? 1 : 0)".utf8),
                      signatureB64: helloSig
                  ),
                  let initialSess = try? CodaE2EESession(
                    directP2Pv2Identity: daemonIdentity,
                    peerStaticX25519PublicB64: clientPub,
                    myEphemeralPrivateKey: daemonEphPriv,
                    peerEphemeralX25519PublicB64: clientEphPub,
                    clientDeviceId: clientDevUUID,
                    daemonDeviceId: daemonId,
                    isInitiator: false
                  )
            else {
                conn.cancel()
                return
            }

            let ackTsMs = UInt64(Date().timeIntervalSince1970 * 1000)
            let ackTranscript = "p2p-ack-v2:\(clientIdStr.lowercased()):\(daemonId.uuidString.lowercased()):\(clientEphPub):\(daemonEphPubB64):\(ackTsMs)"
            guard let ackSigB64 = try? daemonIdentity.sign(message: Data(ackTranscript.utf8)) else {
                conn.cancel()
                return
            }

            let ack: [String: Any] = [
                "frame_type": "direct_p2p_ack",
                "ok": true,
                "daemon_device_id": daemonId.uuidString.lowercased(),
                "daemon_x25519_pub_b64": daemonIdentity.x25519PublicB64,
                "daemon_ed25519_pub_b64": daemonIdentity.ed25519PublicB64,
                "daemon_ephemeral_x25519_pub_b64": daemonEphPubB64,
                "timestamp_ms": ackTsMs,
                "signature_b64": ackSigB64,
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
                  let op = req["op"] as? String
            else {
                conn.cancel()
                return
            }

            if op == "pty_resize" {
                precondition((req["cols"] as? Int) == 98 && (req["rows"] as? Int) == 34)
                let updatedSess = nextSess
                servePacketLoop(conn: conn, daemonId: daemonId, sess: updatedSess)
                return
            }

            guard let reqId = req["req_id"] as? String else {
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
            case "git_diff_request":
                precondition((req["cwd"] as? String) == "/Users/dev/coda")
                replyObj = [
                    "op": "git_diff_response",
                    "req_id": reqId,
                    "branch": "feat/v0.9",
                    "status_short": " M src/main.rs\n?? docs/notes.md",
                    "diff_unified": "diff --git a/src/main.rs b/src/main.rs\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1,1 +1,2 @@\n fn main() {}\n+// v0.9",
                    "is_git_repo": true,
                    "repo_root": "/Users/dev/coda",
                ]
            case "transcribe_audio":
                precondition((req["mime_type"] as? String) == "audio/m4a")
                precondition((req["filename"] as? String) == "dictation.m4a")
                precondition((req["language"] as? String) == "en")
                precondition(Data(base64Encoded: (req["audio_b64"] as? String) ?? "")?.count == 128)
                replyObj = [
                    "op": "transcribe_audio_result",
                    "req_id": reqId,
                    "ok": true,
                    "text": "run cargo test --workspace",
                ]
            case "bootstrap_workspace":
                precondition((req["parent_dir"] as? String) == "~/workspace")
                precondition((req["folder_name"] as? String) == "demo-repo")
                precondition((req["git_clone_url"] as? String) == "https://github.com/org/demo-repo.git")
                replyObj = [
                    "op": "bootstrap_workspace_result",
                    "req_id": reqId,
                    "ok": true,
                    "workspace_path": "/Users/dev/workspace/demo-repo",
                    "detail": "cloned",
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

    static func runSecurityFFIAndVTAssertions() throws {
        // 8a. DirectP2P v2 hybrid static + ephemeral X25519 + transcript binding
        let clientID = UUID()
        let daemonID = UUID()
        let clientId = CodaDeviceIdentity.generate(name: "SelfTest-Client")
        let daemonId = CodaDeviceIdentity.generate(name: "SelfTest-Daemon")
        let clientEph = Curve25519.KeyAgreement.PrivateKey()
        let daemonEph = Curve25519.KeyAgreement.PrivateKey()

        var p2pInitiator = try CodaE2EESession(
            directP2Pv2Identity: clientId,
            peerStaticX25519PublicB64: daemonId.x25519PublicB64,
            myEphemeralPrivateKey: clientEph,
            peerEphemeralX25519PublicB64: daemonEph.publicKey.rawRepresentation.base64EncodedString(),
            clientDeviceId: clientID,
            daemonDeviceId: daemonID,
            isInitiator: true
        )
        var p2pResponder = try CodaE2EESession(
            directP2Pv2Identity: daemonId,
            peerStaticX25519PublicB64: clientId.x25519PublicB64,
            myEphemeralPrivateKey: daemonEph,
            peerEphemeralX25519PublicB64: clientEph.publicKey.rawRepresentation.base64EncodedString(),
            clientDeviceId: clientID,
            daemonDeviceId: daemonID,
            isInitiator: false
        )
        let p2pSealed = try p2pInitiator.sealJSON(Data("{\"op\":\"ping\",\"seq\":1}".utf8))
        let p2pOpened = try p2pResponder.openJSON(p2pSealed)
        precondition(String(decoding: p2pOpened, as: UTF8.self) == "{\"op\":\"ping\",\"seq\":1}")

        var wrongBoundResponder = try CodaE2EESession(
            directP2Pv2Identity: daemonId,
            peerStaticX25519PublicB64: clientId.x25519PublicB64,
            myEphemeralPrivateKey: daemonEph,
            peerEphemeralX25519PublicB64: clientEph.publicKey.rawRepresentation.base64EncodedString(),
            clientDeviceId: clientID,
            daemonDeviceId: UUID(),
            isInitiator: false
        )
        let p2pSealedBad = try p2pInitiator.sealJSON(Data("{}".utf8))
        var mismatchRejected = false
        do { _ = try wrongBoundResponder.openJSON(p2pSealedBad) } catch { mismatchRejected = true }
        precondition(mismatchRejected, "Expected DirectP2P v2 transcript mismatch to fail AEAD")

        // 8b. Relay v2 Swift <-> Rust FFI interop + native zstd + binary interior NUL preservation
        precondition(CodaE2EESession.hasNativeZstdFFI, "Expected libcoda_apple_ffi native zstd FFI")
        precondition(CodaE2EESession.hasRelayV2FFIBridge, "Expected libcoda_apple_ffi Relay v2 FFI")
        let nonceB64 = CodaE2EESession.randomSessionNonceB64()
        var swiftRelay = try CodaE2EESession(
            relayV2Identity: clientId,
            peerStaticX25519PublicB64: daemonId.x25519PublicB64,
            sessionNonceB64: nonceB64,
            clientDeviceId: clientID,
            daemonDeviceId: daemonID,
            isInitiator: true
        )
        guard let rustRelay = CodaRustFFISessionBridge(
            identity: daemonId,
            peerX25519PublicB64: clientId.x25519PublicB64,
            sessionNonceB64: nonceB64,
            clientDeviceId: clientID,
            daemonDeviceId: daemonID,
            isInitiator: false,
            peerSupportsZstd: true
        ) else {
            preconditionFailure("Failed initializing CodaRustFFISessionBridge")
        }

        let s2rPayload = try swiftRelay.sealJSON(Data("{\"op\":\"ping\",\"req_id\":\"selftest-r1\",\"sent_ms\":1234}".utf8))
        let s2rJSON = String(decoding: try JSONEncoder().encode(s2rPayload), as: UTF8.self)
        guard let rOpened = rustRelay.openEncryptedPayloadJSON(s2rJSON), rOpened.contains("\"selftest-r1\"") else {
            preconditionFailure("Rust FFI failed opening Swift Relay v2 payload")
        }

        let bigText = String(repeating: "coda-zstd-ffi-line\n", count: 80)
        let bigMsgData = try JSONSerialization.data(withJSONObject: [
            "op": "pong",
            "req_id": "selftest-r1",
            "sent_ms": 9876,
            "daemon_hostname": bigText,
            "herdr_version": "0.1.20",
        ])
        guard let sealedFromRust = rustRelay.sealPeerMessageJSON(String(decoding: bigMsgData, as: UTF8.self)) else {
            preconditionFailure("Rust FFI failed sealing PeerMessage")
        }
        let encFromRust = try JSONDecoder().decode(CodaEncryptedPayload.self, from: Data(sealedFromRust.utf8))
        precondition(encFromRust.compressed, "Expected Rust FFI to zstd-compress large PeerMessage")
        let swiftOpenedBig = try swiftRelay.openJSON(encFromRust)
        let parsedBig = try JSONSerialization.jsonObject(with: swiftOpenedBig) as? [String: Any]
        precondition((parsedBig?["daemon_hostname"] as? String) == bigText)

        // Binary zstd frame with interior NUL bytes: "A\0B\0C" (RFC 8878 single-segment raw block)
        let nulZstdFrame = Data([0x28, 0xB5, 0x2F, 0xFD, 0x20, 0x05, 0x29, 0x00, 0x00, 0x41, 0x00, 0x42, 0x00, 0x43])
        let decompressedNul = try CodaE2EESession.decompressZstdB64ToData(nulZstdFrame.base64EncodedString())
        precondition(decompressedNul == Data([0x41, 0x00, 0x42, 0x00, 0x43]), "Binary interior NUL zstd decompression truncated")

        // 8c. Post-AEAD ReplayWindow resilience (forged high-seq packet must not poison window)
        let rwNonce = CodaE2EESession.randomSessionNonceB64()
        var rwSender = try CodaE2EESession(
            relayV2Identity: clientId,
            peerStaticX25519PublicB64: daemonId.x25519PublicB64,
            sessionNonceB64: rwNonce,
            clientDeviceId: clientID,
            daemonDeviceId: daemonID,
            isInitiator: true
        )
        var rwReceiver = try CodaE2EESession(
            relayV2Identity: daemonId,
            peerStaticX25519PublicB64: clientId.x25519PublicB64,
            sessionNonceB64: rwNonce,
            clientDeviceId: clientID,
            daemonDeviceId: daemonID,
            isInitiator: false
        )
        let rw1 = try rwSender.sealJSON(Data("{\"n\":1}".utf8))
        let rw2 = try rwSender.sealJSON(Data("{\"n\":2}".utf8))
        let rw3 = try rwSender.sealJSON(Data("{\"n\":3}".utf8))
        let forgedHigh = CodaEncryptedPayload(
            seq: 10_000,
            compressed: false,
            ciphertextB64: Data("forged".utf8).base64EncodedString(),
            sessionNonceB64: rwNonce
        )
        var forgedFailed = false
        do { _ = try rwReceiver.openJSON(forgedHigh) } catch { forgedFailed = true }
        precondition(forgedFailed, "Forged high-seq packet must fail AEAD")
        let openedRw3 = try rwReceiver.openJSON(rw3)
        let openedRw1 = try rwReceiver.openJSON(rw1)
        let openedRw2 = try rwReceiver.openJSON(rw2)
        precondition(openedRw3 == Data("{\"n\":3}".utf8))
        precondition(openedRw1 == Data("{\"n\":1}".utf8))
        precondition(openedRw2 == Data("{\"n\":2}".utf8))
        var replayFailed = false
        do { _ = try rwReceiver.openJSON(rw2) } catch { replayFailed = true }
        precondition(replayFailed, "Replayed seq=2 must fail")

        // 8d. CodaSecretStore 0600 file permissions & CodaPersistedSettings UserDefaults secret scrubbing
        let tmpDir = FileManager.default.temporaryDirectory
            .appendingPathComponent("coda-selftest-secrets-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: tmpDir, withIntermediateDirectories: true)
        let prevSecretsDir = ProcessInfo.processInfo.environment["CODA_APPLE_SECRETS_DIR"]
        let defaultsKey = "dev.codahub.client.settings.v2"
        let prevDefaultsData = UserDefaults.standard.data(forKey: defaultsKey)
        setenv("CODA_APPLE_SECRETS_DIR", tmpDir.path, 1)
        defer {
            if let prevSecretsDir {
                setenv("CODA_APPLE_SECRETS_DIR", prevSecretsDir, 1)
            } else {
                unsetenv("CODA_APPLE_SECRETS_DIR")
            }
            if let prevDefaultsData {
                UserDefaults.standard.set(prevDefaultsData, forKey: defaultsKey)
            } else {
                UserDefaults.standard.removeObject(forKey: defaultsKey)
            }
            try? FileManager.default.removeItem(at: tmpDir)
        }
        let secSettings = CodaPersistedSettings(
            serverBaseURLString: "https://codahub.dev",
            sessionToken: "tok_secret_value_123",
            ownerHandle: "alice",
            voiceConfig: CodaVoiceModelConfig(apiKey: "sk-voice-secret-999")
        )
        secSettings.save()
        if let rawDefaults = UserDefaults.standard.data(forKey: defaultsKey),
           let decodedRaw = try? JSONDecoder().decode(CodaPersistedSettings.self, from: rawDefaults) {
            precondition(decodedRaw.sessionToken.isEmpty, "UserDefaults must NOT store plaintext sessionToken")
            precondition(decodedRaw.voiceConfig.apiKey.isEmpty, "UserDefaults must NOT store plaintext apiKey")
        } else {
            preconditionFailure("Expected scrubbed settings in UserDefaults")
        }
        let reloaded = CodaPersistedSettings.load()
        precondition(reloaded.sessionToken == "tok_secret_value_123")
        precondition(reloaded.voiceConfig.apiKey == "sk-voice-secret-999")
        let secretFiles = try FileManager.default.contentsOfDirectory(at: tmpDir, includingPropertiesForKeys: nil)
        precondition(secretFiles.count >= 2)
        for f in secretFiles {
            let attrs = try FileManager.default.attributesOfItem(atPath: f.path)
            let perms = (attrs[.posixPermissions] as? NSNumber)?.intValue
            precondition(perms == 0o600, "Expected 0600 permissions on secret file, got \(String(describing: perms))")
        }

        // 8e. GhosttyReflowEngine stateful VT100 UTF-8 split chunks, \r/BS, alt-screen, split OSC 7501 & reflow
        var vt = GhosttyReflowEngine(columns: 20, rows: 6)
        let splitUTF8 = Data("Hi 你🚀\r\n".utf8)
        vt.feed(splitUTF8.subdata(in: 0..<5))
        vt.feed(splitUTF8.subdata(in: 5..<splitUTF8.count))
        precondition(vt.visibleLines.first == "Hi 你🚀", "Split UTF-8 failed: \(vt.visibleLines)")
        vt.feed(Data("FAIL\rPASS!\u{08} \r\n".utf8))
        precondition(vt.visibleLines.count == 2 && vt.visibleLines[1] == "PASS")
        vt.feed(Data("\u{1B}[?1049hALT SCREEN\u{1B}[?1049l".utf8))
        precondition(vt.visibleLines[0] == "Hi 你🚀" && vt.visibleLines[1] == "PASS")
        let splitOSC = Data("\u{1B}]7501;state=blocked;kind=permission;msg=Proceed?\u{07}".utf8)
        vt.feed(splitOSC.subdata(in: 0..<11))
        vt.feed(splitOSC.subdata(in: 11..<splitOSC.count))
        precondition(vt.latestHerdrStatus?.state == .needsYou && vt.latestHerdrStatus?.message == "Proceed?")
        precondition(!vt.plainVisibleText.contains("7501"))

        var vtReflow = GhosttyReflowEngine(columns: 40, rows: 8)
        vtReflow.feed(Data("123456789012345678901234567890".utf8))
        vtReflow.resizeReflow(columns: 20, rows: 8)
        precondition(vtReflow.visibleLines.count == 2)

        // 8f. Path boundary helper
        precondition(codaPathWithinRoot("/Users/dev/coda", root: "/Users/dev/coda"))
        precondition(codaPathWithinRoot("/Users/dev/coda/crates/coda-server", root: "/Users/dev/coda"))
        precondition(!codaPathWithinRoot("/Users/dev/coda-other", root: "/Users/dev/coda"))
        precondition(!codaPathWithinRoot("/Users/dev/coda/../coda-other", root: "/Users/dev/coda"))

        // 8g. Ghostty BG/Inverse, strict CSI prefix filtering, JSON OSC 7501 & Markdown bullet guard
        var vtRich = GhosttyReflowEngine(columns: 80, rows: 10, treatLFAsCRLF: true)
        vtRich.feed(Data(
            "\u{1B}[>4;2mplain\u{1B}[38;2;250;200;100;48;2;10;20;30;7mHI\u{1B}[0m\n- Markdown bullet item\n-removedDiffLine\n\u{1B}]7501;{\"state\":\"blocked\",\"kind\":\"question\",\"title\":\"Confirm\",\"msg\":\"Ship?\"}\u{07}".utf8
        ))
        let richLines = vtRich.styledLines
        precondition(richLines.count == 3, "Expected 3 styled lines, got \(richLines.count)")
        precondition(richLines[0].spans.count == 2 && richLines[0].spans[0].text == "plain")
        precondition(richLines[0].spans[1].bg == GhosttyRGBColor(r: 10, g: 20, b: 30) && richLines[0].spans[1].inverse)
        precondition(richLines[1].kind == .normal, "Markdown bullet should stay .normal")
        precondition(richLines[2].kind == .diffRemoved, "Unified diff deletion should be .diffRemoved")
        precondition(vtRich.latestHerdrStatus?.state == .needsYou && vtRich.latestHerdrStatus?.message == "Ship?")

        // 8h. Smart-Path hot-switch policy & edge-triggered OSC 7501 notification tracker
        let spPeer = UUID()
        var spPolicy = CodaSmartPathPolicy(baseInterval: 10.0, maxInterval: 60.0, dropCooldown: 5.0)
        let spNow = Date(timeIntervalSince1970: 10_000)
        precondition(spPolicy.canProbeNow(peerId: spPeer, isDirect: false, now: spNow))
        precondition(!spPolicy.canProbeNow(peerId: spPeer, isDirect: true, now: spNow))
        precondition(spPolicy.recordProbeFailure(peerId: spPeer) == 10.0)
        precondition(spPolicy.recordProbeFailure(peerId: spPeer) == 20.0)
        precondition(spPolicy.recordProbeFailure(peerId: spPeer) == 40.0)
        precondition(spPolicy.recordProbeFailure(peerId: spPeer) == 60.0)
        spPolicy.recordDirectDrop(peerId: spPeer, now: spNow)
        precondition(!spPolicy.canProbeNow(peerId: spPeer, isDirect: false, now: spNow.addingTimeInterval(2.0)))
        precondition(spPolicy.canProbeNow(peerId: spPeer, isDirect: false, now: spNow.addingTimeInterval(5.0)))
        let rankedCands = CodaSmartPathPolicy.buildDirectCandidates(
            udpFound: (host: "192.168.1.10", port: 24444),
            endpoints: [
                CodaEndpointCandidate(kind: "cn_public_ipv6", addr: "[2409::1]:24444"),
                CodaEndpointCandidate(kind: "lan_ipv4", addr: "192.168.1.10:24444"),
                CodaEndpointCandidate(kind: "lan_ipv4", addr: "192.168.1.20:24444"),
            ]
        )
        precondition(rankedCands.count == 3 && rankedCands[0].host == "192.168.1.10" && rankedCands[2].host == "2409::1")

        var nt = CodaNotificationTracker(cooldown: 20.0)
        let nt0 = Date(timeIntervalSince1970: 50_000)
        precondition(nt.observe(sessionId: "s1", machineName: "Mac-mini", workspaceName: "coda", taskTitle: "claude", statusMessage: nil, state: .working, activeSessionId: "s2", locale: .zh, now: nt0) == nil)
        let ntAlert = nt.observe(sessionId: "s1", machineName: "Mac-mini", workspaceName: "coda", taskTitle: "claude", statusMessage: "Approve?", state: .needsYou, activeSessionId: "s2", locale: .zh, now: nt0.addingTimeInterval(2))
        precondition(ntAlert?.kind == .needsYou && ntAlert?.body == "Approve?")
        _ = nt.observe(sessionId: "s1", machineName: "Mac-mini", workspaceName: "coda", taskTitle: "claude", statusMessage: nil, state: .working, activeSessionId: "s2", locale: .en, now: nt0.addingTimeInterval(4))
        precondition(nt.observe(sessionId: "s1", machineName: "Mac-mini", workspaceName: "coda", taskTitle: "claude", statusMessage: "Done", state: .done, activeSessionId: "s2", locale: .en, now: nt0.addingTimeInterval(8)) == nil)
        _ = nt.observe(sessionId: "s1", machineName: "Mac-mini", workspaceName: "coda", taskTitle: "claude", statusMessage: nil, state: .working, activeSessionId: "s2", locale: .en, now: nt0.addingTimeInterval(24))
        let ntDone = nt.observe(sessionId: "s1", machineName: "Mac-mini", workspaceName: "coda", taskTitle: "claude", statusMessage: "Done", state: .done, activeSessionId: "s2", locale: .en, now: nt0.addingTimeInterval(25))
        precondition(ntDone?.kind == .done && ntDone?.body == "Done")

        // 8i. V3 Regression Guards: scrollUp full-region CSI M / CSI 99 S no-crash, oscStatusGeneration,
        // treatLFAsCRLF snapshot->live transition, and alt-screen resizeReflow primaryScrollback preservation
        var vtCrash = GhosttyReflowEngine(columns: 30, rows: 6)
        vtCrash.feed(Data("L1\r\nL2\r\nL3\r\nL4\r\nL5\r\nL6".utf8))
        // Cursor is on bottom row (row 6); CSI M deletes 1 line from scrollTop==scrollBottom (n == bottom-top+1)
        vtCrash.feed(Data("\u{1B}[6;1H\u{1B}[M".utf8))
        // Full-screen scrollUp CSI 99 S and huge CSI parameter must not trap
        vtCrash.feed(Data("\u{1B}[99S\u{1B}[999999999999BOK".utf8))
        precondition(vtCrash.plainVisibleText.contains("OK"))

        var vtOsc = GhosttyReflowEngine(columns: 40, rows: 6)
        let gen0 = vtOsc.oscStatusGeneration
        vtOsc.feed(Data("\u{1B}]7501;state=blocked;msg=Approve?\u{07}".utf8))
        let gen1 = vtOsc.oscStatusGeneration
        precondition(gen1 == gen0 + 1 && vtOsc.latestHerdrStatus?.state == .needsYou)
        vtOsc.feed(Data("plain stdout after approval\r\n".utf8))
        precondition(vtOsc.oscStatusGeneration == gen1, "Plain stdout must not bump oscStatusGeneration")
        vtOsc.feed(Data("\u{1B}]7501;state=working;msg=Running\u{07}".utf8))
        precondition(vtOsc.oscStatusGeneration == gen1 + 1 && vtOsc.latestHerdrStatus?.state == .working)

        var vtLf = GhosttyReflowEngine(columns: 40, rows: 6, treatLFAsCRLF: true)
        vtLf.feed(Data("snap1\nsnap2\n".utf8))
        vtLf.treatLFAsCRLF = false
        vtLf.feed(Data("AB\nC".utf8))
        precondition(vtLf.visibleLines.last == "  C", "Live bare LF with treatLFAsCRLF=false must preserve cursor col, got: \(vtLf.visibleLines)")

        var vtAlt = GhosttyReflowEngine(columns: 24, rows: 6, maxScrollback: 100)
        for i in 1...10 {
            vtAlt.feed(Data("hist-\(i)\r\n".utf8))
        }
        vtAlt.feed(Data("\u{1B}[?1049hFULLSCREEN TUI".utf8))
        vtAlt.resizeReflow(columns: 40, rows: 6)
        vtAlt.feed(Data("\u{1B}[?1049l".utf8))
        precondition(vtAlt.plainVisibleText.contains("hist-1") && vtAlt.plainVisibleText.contains("hist-10"),
                     "Alt-screen resizeReflow must preserve primaryScrollback")

        // 8j. v0.7.0 HostHealthSnapshot wire decoding & quick terminal commands
        let healthRaw = Data("""
        {"daemon_version":"0.7.0","os":"linux","arch":"x86_64","cpu_cores":8,"load_avg_1m":0.42,"mem_used_mb":6144,"mem_total_mb":16384,"daemon_uptime_secs":120,"ota_supported":true}
        """.utf8)
        let healthSnap = try JSONDecoder().decode(CodaHostHealthSnapshot.self, from: healthRaw)
        precondition(healthSnap.compactSummary == "v0.7.0 · 8c · load 0.42 · 6.0/16.0G",
                     "Unexpected compactSummary: \(healthSnap.compactSummary)")
        precondition(CodaHostHealthSnapshot.quickTerminalCommands.contains("/compact"))
        precondition(CodaHostHealthSnapshot.quickTerminalCommands.contains("git status"))

        // 8k. v0.8.0 Apple parity helpers: terminal search, Markdown export & remote file paths
        let hits = CodaTerminalTranscriptHelper.findMatchingLineIndices(
            in: ["$ cargo test", "ERROR: failed", "test error ok", ""],
            query: "error"
        )
        precondition(hits == [1, 2], "Unexpected search hits: \(hits)")
        let md = CodaTerminalTranscriptHelper.buildMarkdown(
            title: "coda · agy",
            cwd: "/opt/coda",
            hostName: "linux-box",
            lines: ["$ cargo test", "ok", "   "],
            exportedISO: "2026-11-21T00:00:00Z"
        )
        precondition(md.contains("# Coda Session Transcript — coda · agy") && md.contains("$ cargo test\nok\n```"))
        precondition(CodaTerminalTranscriptHelper.parentSubPath("crates/coda-daemon/src") == "crates/coda-daemon")
        precondition(CodaTerminalTranscriptHelper.joinRootAndRel(rootPath: "/opt/coda/", relPath: "/src/lib.rs") == "/opt/coda/src/lib.rs")

        // 8l. v0.9.0 Git Diff Parser, File Filter, Shell-Safe Path Quoting, Bracketed Paste, Bootstrap RCE Guards & Webhook Codables
        let rawStatusShort = """
         M crates/coda-server/src/lib.rs
        A  apple/Sources/CodaUI/NewView.swift
        D  legacy/old.txt
        R  src/before.rs -> src/after.rs
        ?? "docs/my spec.md"
        """
        let parsedFiles = CodaGitDiffResult.parseStatusShort(rawStatusShort)
        precondition(parsedFiles.count == 5, "Expected 5 parsed git changed files, got \(parsedFiles.count)")
        precondition(parsedFiles[0] == CodaGitChangedFile(status: "M", path: "crates/coda-server/src/lib.rs"))
        precondition(parsedFiles[1] == CodaGitChangedFile(status: "A", path: "apple/Sources/CodaUI/NewView.swift"))
        precondition(parsedFiles[2] == CodaGitChangedFile(status: "D", path: "legacy/old.txt"))
        precondition(parsedFiles[3] == CodaGitChangedFile(status: "R", path: "src/after.rs", previousPath: "src/before.rs"))
        precondition(parsedFiles[4] == CodaGitChangedFile(status: "??", path: "docs/my spec.md"))

        let sampleUnifiedDiff = """
        diff --git a/crates/coda-server/src/lib.rs b/crates/coda-server/src/lib.rs
        index 1111111..2222222 100644
        --- a/crates/coda-server/src/lib.rs
        +++ b/crates/coda-server/src/lib.rs
        @@ -10,2 +10,3 @@
         pub fn run() {}
        +pub fn notify() {}
        diff --git a/src/before.rs b/src/after.rs
        similarity index 92%
        rename from src/before.rs
        rename to src/after.rs
        --- a/src/before.rs
        +++ b/src/after.rs
        @@ -1,2 +1,2 @@
        -let v = 8;
        +let v = 9;
        """
        precondition(CodaGitDiffResult.filterDiffByFile(sampleUnifiedDiff, filePath: nil) == sampleUnifiedDiff)
        let serverOnlyDiff = CodaGitDiffResult.filterDiffByFile(sampleUnifiedDiff, filePath: "crates/coda-server/src/lib.rs")
        precondition(serverOnlyDiff.contains("+pub fn notify() {}") && !serverOnlyDiff.contains("let v = 9;"))
        let renamedDiff = CodaGitDiffResult.filterDiffByFile(sampleUnifiedDiff, filePath: "src/after.rs")
        precondition(renamedDiff.contains("+let v = 9;") && !renamedDiff.contains("pub fn notify()"))
        let untrackedDiff = CodaGitDiffResult.filterDiffByFile(sampleUnifiedDiff, filePath: "docs/my spec.md")
        precondition(untrackedDiff.isEmpty)

        // Shell-safe path quoting & Bracketed Paste escape injection guard
        precondition(codaShellQuotePath("/tmp/coda/paste-01.png") == "/tmp/coda/paste-01.png")
        precondition(codaShellQuotePath("/tmp/my shot's $(id).png") == "'/tmp/my shot'\\''s $(id).png'")
        precondition(codaShellQuotePath("-rf") == "'-rf'")
        precondition(codaShellQuotePath("/tmp/a\0b.png") == "/tmp/ab.png")
        precondition(codaShellQuotePath("") == "''")

        let bpBytes = CodaTerminalTranscriptHelper.encodeBracketedPaste("step 1\nstep 2\u{1B}[201~evil")
        precondition(bpBytes == Data("\u{1B}[200~step 1\nstep 2evil\u{1B}[201~\r".utf8))

        // Workspace bootstrapper RCE validation & repo folder suggestion
        precondition(CodaWorkspaceBootstrapValidator.isValidFolderName("coda-v0.9"))
        precondition(!CodaWorkspaceBootstrapValidator.isValidFolderName("../escape"))
        precondition(!CodaWorkspaceBootstrapValidator.isValidFolderName("sub/dir"))
        precondition(!CodaWorkspaceBootstrapValidator.isValidFolderName("-upload-pack"))
        precondition(CodaWorkspaceBootstrapValidator.isAllowedGitCloneURL(""))
        precondition(CodaWorkspaceBootstrapValidator.isAllowedGitCloneURL("https://github.com/org/coda.git"))
        precondition(CodaWorkspaceBootstrapValidator.isAllowedGitCloneURL("ssh://git@github.com/org/coda.git"))
        precondition(CodaWorkspaceBootstrapValidator.isAllowedGitCloneURL("git@github.com:org/coda.git"))
        precondition(!CodaWorkspaceBootstrapValidator.isAllowedGitCloneURL("ext::sh -c id"))
        precondition(!CodaWorkspaceBootstrapValidator.isAllowedGitCloneURL("file:///etc/passwd"))
        precondition(!CodaWorkspaceBootstrapValidator.isAllowedGitCloneURL("--upload-pack=evil"))
        precondition(!CodaWorkspaceBootstrapValidator.isAllowedGitCloneURL("http://example.com/repo.git"))
        precondition(CodaWorkspaceBootstrapValidator.suggestedFolderName(fromGitCloneURL: "https://github.com/org/coda-apple.git") == "coda-apple")
        precondition(CodaWorkspaceBootstrapValidator.suggestedFolderName(fromGitCloneURL: "git@github.com:org/herdr.git") == "herdr")

        // Webhook & Prune Stale Devices wire JSON field alignment
        let wh = CodaNotificationWebhookSettings(
            barkURL: "https://api.day.app/demo",
            telegramBotToken: "123:ABC",
            telegramChatId: "999",
            feishuWebhookURL: "https://open.feishu.cn/open-apis/bot/v2/hook/xyz",
            discordWebhookURL: "https://discord.com/api/webhooks/1/2",
            genericWebhookURL: "https://hooks.example.com/coda",
            notifyOnNeedsYou: true,
            notifyOnFinished: false
        )
        let whData = try JSONEncoder().encode(wh)
        let whDict = try JSONSerialization.jsonObject(with: whData) as? [String: Any]
        precondition((whDict?["bark_url"] as? String) == "https://api.day.app/demo")
        precondition((whDict?["telegram_bot_token"] as? String) == "123:ABC")
        precondition((whDict?["telegram_chat_id"] as? String) == "999")
        precondition((whDict?["feishu_webhook_url"] as? String) == "https://open.feishu.cn/open-apis/bot/v2/hook/xyz")
        precondition((whDict?["discord_webhook_url"] as? String) == "https://discord.com/api/webhooks/1/2")
        precondition((whDict?["generic_webhook_url"] as? String) == "https://hooks.example.com/coda")
        precondition((whDict?["notify_on_needs_you"] as? Bool) == true)
        precondition((whDict?["notify_on_finished"] as? Bool) == false)
        let whDecoded = try JSONDecoder().decode(CodaNotificationWebhookSettings.self, from: whData)
        precondition(whDecoded == wh)
        try whDecoded.validateWebhookURLs()
        var badWebhookRejected = false
        do {
            try CodaNotificationWebhookSettings(barkURL: "http://169.254.169.254/latest/meta-data").validateWebhookURLs()
        } catch {
            badWebhookRejected = true
        }
        precondition(badWebhookRejected, "Expected SSRF metadata webhook URL to be rejected")

        let testPushDecoded = try JSONDecoder().decode(
            CodaNotificationTestResult.self,
            from: Data(#"{"ok":true,"delivered_channels":["bark","discord"],"errors":[]}"#.utf8)
        )
        precondition(testPushDecoded.ok && testPushDecoded.deliveredChannels == ["bark", "discord"])
        let pruneDecoded = try JSONDecoder().decode(
            CodaPruneStaleDevicesResult.self,
            from: Data(#"{"ok":true,"pruned_count":4}"#.utf8)
        )
        precondition(pruneDecoded.ok && pruneDecoded.prunedCount == 4)
        let statusCheckSession = CodaAgentSession(
            id: "s-status",
            terminalAddress: "t9",
            workspaceName: "coda",
            hostName: "mac",
            agentName: "claude",
            taskTitle: "Verify status alias",
            state: .needsYou
        )
        precondition(statusCheckSession.status == .needsYou)

        // 8m. v1.0.0 Adversarial Regression Guards:
        // (1) Iterative fixed-point bracketed paste sanitizer + CRLF normalization + DECSET ?2004h/?2004l tracking
        let nestedPasteAttack = "line1\r\nline2\u{1B}[\u{1B}[201~201~\u{1B}[\u{1B}[200~200~\u{9B}201~safe\0end"
        let encodedBracketed = CodaTerminalTranscriptHelper.encodeBracketedPaste(
            nestedPasteAttack,
            appendCarriageReturn: true,
            bracketedModeEnabled: true
        )
        precondition(
            encodedBracketed == Data("\u{1B}[200~line1\nline2safeend\u{1B}[201~\r".utf8),
            "Nested bracketed paste bypass was not iteratively stripped!"
        )
        let encodedPlainShell = CodaTerminalTranscriptHelper.encodeBracketedPaste(
            nestedPasteAttack,
            appendCarriageReturn: true,
            bracketedModeEnabled: false
        )
        precondition(
            encodedPlainShell == Data("line1\nline2safeend\r".utf8),
            "Plain shell paste without ?2004h must not inject CSI 200~/201~"
        )

        var vtPasteMode = GhosttyReflowEngine(columns: 40, rows: 10)
        precondition(!vtPasteMode.bracketedPasteEnabled && !vtPasteMode.bracketedPasteExplicit)
        vtPasteMode.feed(Data("\u{1B}[?2004h\u{1B}[?1000h\u{1B}[?1006h".utf8))
        precondition(
            vtPasteMode.bracketedPasteEnabled
                && vtPasteMode.bracketedPasteExplicit
                && vtPasteMode.mouseTrackingMode == 1000
                && vtPasteMode.sgrMouseEncoding
        )

        // Plain-text snapshot reload (`restoreModeFlagsIfUnset`) must preserve live `?2004h` & mouse modes
        var snapshotReloaded = GhosttyReflowEngine(columns: 40, rows: 10, treatLFAsCRLF: true)
        snapshotReloaded.feed(Data("plain snapshot without escape sequences\n".utf8))
        snapshotReloaded.treatLFAsCRLF = false
        snapshotReloaded.restoreModeFlagsIfUnset(from: vtPasteMode)
        precondition(
            snapshotReloaded.bracketedPasteEnabled
                && snapshotReloaded.bracketedPasteExplicit
                && snapshotReloaded.mouseTrackingMode == 1000
                && snapshotReloaded.sgrMouseEncoding,
            "Plain-text snapshot reload must preserve prior ?2004h & mouse mode flags"
        )

        // If snapshot itself explicitly emitted `?2004l`, `restoreModeFlagsIfUnset` must NOT override it
        var explicitOffSnap = GhosttyReflowEngine(columns: 40, rows: 10, treatLFAsCRLF: true)
        explicitOffSnap.feed(Data("\u{1B}[?2004lshell prompt $ ".utf8))
        explicitOffSnap.restoreModeFlagsIfUnset(from: vtPasteMode)
        precondition(!explicitOffSnap.bracketedPasteEnabled && explicitOffSnap.bracketedPasteExplicit)

        precondition(GhosttyReflowEngine.encodeMouseWheel(up: true, column: 20, row: 5, sgrEncoding: true) == Data("\u{1B}[<64;20;5M".utf8))
        precondition(GhosttyReflowEngine.encodeMouseWheel(up: false, column: 20, row: 5, sgrEncoding: true) == Data("\u{1B}[<65;20;5M".utf8))
        vtPasteMode.feed(Data("\u{1B}[?2004l".utf8))
        precondition(!vtPasteMode.bracketedPasteEnabled && vtPasteMode.bracketedPasteExplicit)
        vtPasteMode.feed(Data("\u{1B}[?2004h\u{1B}cAFTER_RIS".utf8))
        precondition(
            !vtPasteMode.bracketedPasteEnabled
                && !vtPasteMode.bracketedPasteExplicit
                && vtPasteMode.mouseTrackingMode == 0
                && !vtPasteMode.sgrMouseEncoding
        )
        precondition(vtPasteMode.plainVisibleText.contains("AFTER_RIS"))

        // Effective bracketed paste resolution for already-running idle TUIs/shells vs plain shells/REPLs
        precondition(CodaTerminalTranscriptHelper.resolveEffectiveBracketedPaste(explicitSeen: false, engineEnabled: false, agentName: "claude"))
        precondition(CodaTerminalTranscriptHelper.resolveEffectiveBracketedPaste(explicitSeen: false, engineEnabled: false, agentName: "/opt/homebrew/bin/codex"))
        precondition(CodaTerminalTranscriptHelper.resolveEffectiveBracketedPaste(explicitSeen: false, engineEnabled: false, agentName: "gemini"))
        precondition(CodaTerminalTranscriptHelper.resolveEffectiveBracketedPaste(explicitSeen: false, engineEnabled: false, agentName: "-zsh"))
        precondition(CodaTerminalTranscriptHelper.resolveEffectiveBracketedPaste(explicitSeen: false, engineEnabled: false, agentName: "/usr/local/bin/fish"))
        precondition(!CodaTerminalTranscriptHelper.resolveEffectiveBracketedPaste(explicitSeen: false, engineEnabled: false, agentName: "/bin/sh"))
        precondition(!CodaTerminalTranscriptHelper.resolveEffectiveBracketedPaste(explicitSeen: false, engineEnabled: false, agentName: "dash"))
        precondition(!CodaTerminalTranscriptHelper.resolveEffectiveBracketedPaste(explicitSeen: false, engineEnabled: false, agentName: "bash"))
        precondition(!CodaTerminalTranscriptHelper.resolveEffectiveBracketedPaste(explicitSeen: false, engineEnabled: false, agentName: "python3"))
        precondition(!CodaTerminalTranscriptHelper.resolveEffectiveBracketedPaste(explicitSeen: false, engineEnabled: false, agentName: "psql"))
        precondition(!CodaTerminalTranscriptHelper.resolveEffectiveBracketedPaste(explicitSeen: false, engineEnabled: false, agentName: "ssh"))
        // Explicit `?2004l` (`explicitSeen: true, engineEnabled: false`) must override even known TUI default
        precondition(!CodaTerminalTranscriptHelper.resolveEffectiveBracketedPaste(explicitSeen: true, engineEnabled: false, agentName: "claude"))
        precondition(!CodaTerminalTranscriptHelper.resolveEffectiveBracketedPaste(explicitSeen: true, engineEnabled: false, agentName: "zsh"))
        // Explicit `?2004h` (`explicitSeen: true, engineEnabled: true`) must enable even on bash/python
        precondition(CodaTerminalTranscriptHelper.resolveEffectiveBracketedPaste(explicitSeen: true, engineEnabled: true, agentName: "bash"))

        // (2) GitDiffResponse v1.0 wire decoding (`is_git_repo`, `repo_root`, `error`), non-rename arrow guard & split rename filter
        let legacyDiffWire = Data(#"{"branch":"main","status_short":"","diff_unified":""}"#.utf8)
        let decodedLegacyDiff = try JSONDecoder().decode(CodaGitDiffResult.self, from: legacyDiffWire)
        precondition(decodedLegacyDiff.isGitRepo == true && decodedLegacyDiff.repoRoot == nil && decodedLegacyDiff.error == nil)

        let v1DiffWire = Data(#"{"branch":"feat/v1","status_short":" M apple/Sources/App.swift\nD  old.swift\n?? new_dir/\n?? \"a -> b.txt\"\nR  src/a.rs -> src/b.rs","diff_unified":"","is_git_repo":true,"repo_root":"/Users/dev/coda"}"#.utf8)
        let decodedV1Diff = try JSONDecoder().decode(CodaGitDiffResult.self, from: v1DiffWire)
        precondition(decodedV1Diff.isGitRepo == true)
        precondition(decodedV1Diff.repoRoot == "/Users/dev/coda")
        let v1Files = decodedV1Diff.changedFiles
        precondition(v1Files.count == 5, "Expected 5 v1 diff files, got \(v1Files.count)")
        precondition(CodaTerminalTranscriptHelper.joinRootAndRel(rootPath: decodedV1Diff.repoRoot ?? "", relPath: v1Files[0].path) == "/Users/dev/coda/apple/Sources/App.swift")
        precondition(v1Files[1].isDeleted && !v1Files[0].isDeleted)
        precondition(v1Files[2].isDirectoryEntry && !v1Files[0].isDirectoryEntry)
        precondition(v1Files[3].path == "a -> b.txt" && v1Files[3].previousPath == nil, "Untracked file with ' -> ' must not split as rename")
        precondition(v1Files[4].path == "src/b.rs" && v1Files[4].previousPath == "src/a.rs")

        let nonGitWire = Data(#"{"branch":"","status_short":"","diff_unified":"","is_git_repo":false,"error":"not a git repository"}"#.utf8)
        let decodedNonGit = try JSONDecoder().decode(CodaGitDiffResult.self, from: nonGitWire)
        precondition(!decodedNonGit.isGitRepo && decodedNonGit.error == "not a git repository")

        let splitRenameUnified = """
        diff --git a/src/a.rs b/src/a.rs
        deleted file mode 100644
        --- a/src/a.rs
        +++ /dev/null
        @@ -1 +0,0 @@
        -fn old_fn() {}
        diff --git a/src/b.rs b/src/b.rs
        new file mode 100644
        --- /dev/null
        +++ b/src/b.rs
        @@ -0,0 +1 @@
        +fn new_fn() {}
        """
        let splitFiltered = CodaGitDiffResult.filterDiffByFile(splitRenameUnified, filePath: "src/b.rs", previousPath: "src/a.rs")
        precondition(splitFiltered.contains("-fn old_fn()") && splitFiltered.contains("+fn new_fn()"))

        // (3) Voice endpoint URL validation (empty-host bypass, query preservation) & macOS TCC bundle guard
        var emptyHostRejected = false
        do {
            _ = try CodaVoiceEndpointValidator.validatedTranscriptionURL(from: "https:///foo")
        } catch {
            emptyHostRejected = true
        }
        precondition(emptyHostRejected, "https:///foo empty host must be rejected")

        var insecureHttpRejected = false
        do {
            _ = try CodaVoiceEndpointValidator.validatedTranscriptionURL(from: "http://example.com/v1")
        } catch {
            insecureHttpRejected = true
        }
        precondition(insecureHttpRejected, "Non-loopback http:// must be rejected")

        let azureStyleURL = try CodaVoiceEndpointValidator.validatedTranscriptionURL(
            from: "https://api.example.com/openai/v1/?api-version=2024-06"
        )
        precondition(
            azureStyleURL.absoluteString == "https://api.example.com/openai/v1/audio/transcriptions?api-version=2024-06",
            "Query params must stay after /audio/transcriptions, got \(azureStyleURL.absoluteString)"
        )
        let localWhisperURL = try CodaVoiceEndpointValidator.validatedTranscriptionURL(from: "http://127.0.0.1:8080/v1")
        precondition(localWhisperURL.absoluteString == "http://127.0.0.1:8080/v1/audio/transcriptions")

        precondition(!CodaVoiceEndpointValidator.isBundledWithUsageDescription(bundlePathExtension: "", usageDescription: "Mic desc"))
        precondition(!CodaVoiceEndpointValidator.isBundledWithUsageDescription(bundlePathExtension: "app", usageDescription: nil))
        precondition(!CodaVoiceEndpointValidator.isBundledWithUsageDescription(bundlePathExtension: "app", usageDescription: "   "))
        precondition(CodaVoiceEndpointValidator.isBundledWithUsageDescription(bundlePathExtension: "app", usageDescription: "Record voice"))

        let maskedCfg = CodaVoiceModelConfig(apiKey: "••••••••")
        precondition(maskedCfg.hasMaskedDaemonKey && !maskedCfg.hasLocalPlaintextApiKey)
        let transcribeDecoded = try JSONDecoder().decode(
            CodaTranscribeAudioResult.self,
            from: Data(#"{"ok":true,"text":"ship v1.0.0"}"#.utf8)
        )
        precondition(transcribeDecoded.ok && transcribeDecoded.text == "ship v1.0.0")

        // (4) Form-Factor Detection Guards: iPad Stage Manager / Split View, Zoomed iPad mini, Mac window & Duo Cover keyboard shrink
        precondition(
            CodaFormFactor.detect(width: 620, height: 834, isHorizontalRegular: true, isVerticalRegular: true) == .ipadOrDesktop,
            "iPad Split View / Stage Manager 620x834 must NOT misclassify as Duo Inner"
        )
        precondition(
            CodaFormFactor.detect(width: 900, height: 640, isHorizontalRegular: true, isVerticalRegular: true) == .ipadOrDesktop,
            "iPad Stage Manager / Mac 900x640 must NOT misclassify as Duo Inner"
        )
        precondition(
            CodaFormFactor.detect(width: 640, height: 960, isHorizontalRegular: true, isVerticalRegular: true) == .ipadOrDesktop,
            "Display-Zoomed iPad mini 640x960 must NOT misclassify as Duo Inner"
        )
        precondition(
            CodaFormFactor.detect(
                width: 626,
                height: 890,
                isHorizontalRegular: true,
                isVerticalRegular: true,
                isPadOrMacHardware: true
            ) == .ipadOrDesktop,
            "Mac/iPad hardware window at 626x890 must stay .ipadOrDesktop"
        )
        precondition(
            CodaFormFactor.detect(
                width: 466,
                height: 421,
                isHorizontalRegular: false,
                isVerticalRegular: true,
                hardwareScreenSize: CGSize(width: 466, height: 678)
            ) == .iphoneDuoCoverPortrait,
            "Duo Cover 466x678 with keyboard open must stay .iphoneDuoCoverPortrait instead of flipping to .iphoneLandscape"
        )
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
