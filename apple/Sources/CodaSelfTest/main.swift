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
