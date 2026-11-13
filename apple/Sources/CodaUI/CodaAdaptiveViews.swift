import Combine
import CodaGhosttyKit
import CodaKit
import SwiftUI

// MARK: - OLED Terminal Palette (Matches Target Screenshots 1:1)

public enum CodaPalette {
    public static let pureBlack = Color.black
    public static let sidebarBg = Color(red: 0.02, green: 0.02, blue: 0.028)
    public static let keySurface = Color(red: 0.09, green: 0.09, blue: 0.10)
    public static let keyBorder = Color(red: 0.15, green: 0.15, blue: 0.17)
    public static let workingEmerald = Color(red: 0.133, green: 0.773, blue: 0.369)
    public static let completedSky = Color(red: 0.220, green: 0.741, blue: 0.973)
    public static let coralAgent = Color(red: 0.878, green: 0.424, blue: 0.314)
}

@MainActor
public final class CodaWorkspaceStore: ObservableObject {
    @Published public var sessions: [CodaAgentSession]
    @Published public var selectedSessionId: String
    @Published public var showSidebar: Bool = true
    @Published public var draftCommand: String = "发个 iOS 版"
    @Published public var bypassPermissions: Bool = true
    @Published public var voiceConfig: CodaVoiceModelConfig = CodaVoiceModelConfig()
    @Published public var showVoiceConfigSheet: Bool = false
    @Published public var routeMode: SmartPathRouteMode = .lanDirect
    @Published public var serverURLString: String = "https://codahub.dev"
    @Published public var accountLabel: String = "vampire@codahub.dev"
    @Published public var remoteHosts: [CodaRemoteDevice] = []
    @Published public var selectedHostId: UUID? = nil
    @Published public var showAccountSheet: Bool = false
    @Published public var pendingVerificationURL: String? = nil
    @Published public var pendingUserCode: String? = nil
    @Published public var authStatusMessage: String? = nil

    private var networkClient: CodaNetworkClient?
    private var localIdentity: CodaDeviceIdentity = CodaDeviceIdentity.generate(name: "Apple-Device")

    public init(initialSessions: [CodaAgentSession] = CodaAdaptiveRootView.sampleSessions) {
        self.sessions = initialSessions
        self.selectedSessionId = initialSessions.first?.id ?? ""
    }

    public func startBrowserDeviceLogin() async {
        guard let url = URL(string: serverURLString.trimmingCharacters(in: .whitespacesAndNewlines)) else {
            authStatusMessage = "服务器地址格式无效"
            return
        }
        let client = CodaNetworkClient(serverBaseURL: url)
        self.networkClient = client
        do {
            #if os(iOS)
            let platform = "ios"
            #else
            let platform = "macos"
            #endif
            let started = try await client.startDeviceAuth(
                deviceName: localIdentity.name,
                role: "client",
                platform: platform,
                identity: localIdentity
            )
            self.pendingUserCode = started.userCode
            self.pendingVerificationURL = started.verificationURIComplete
            self.authStatusMessage = "请在浏览器打开链接确认授权码：\(started.userCode)"

            for _ in 0..<150 {
                try await Task.sleep(nanoseconds: UInt64(max(1, started.intervalSecs)) * 1_000_000_000)
                let polled = try await client.pollDeviceAuth(deviceCode: started.deviceCode)
                if polled.state == "approved" {
                    self.accountLabel = "@\(polled.ownerHandle ?? "owner")"
                    self.pendingUserCode = nil
                    self.pendingVerificationURL = nil
                    self.authStatusMessage = "授权成功！正在加载云端主机列表..."
                    await refreshAccountHosts()
                    return
                } else if polled.state == "expired" {
                    self.authStatusMessage = "授权码已过期，请重新生成"
                    return
                }
            }
        } catch {
            self.authStatusMessage = "连接失败：\(error.localizedDescription)"
        }
    }

    public func refreshAccountHosts() async {
        guard let client = networkClient else { return }
        do {
            let all = try await client.listDevices()
            self.remoteHosts = all.filter { $0.role == "daemon" || $0.role == "hybrid" }
            if selectedHostId == nil {
                selectedHostId = remoteHosts.first(where: { $0.isOnline })?.id ?? remoteHosts.first?.id
            }
        } catch {
            self.authStatusMessage = "拉取设备失败：\(error.localizedDescription)"
        }
    }

    public var activeSession: CodaAgentSession {
        sessions.first(where: { $0.id == selectedSessionId }) ?? sessions[0]
    }

    public func simulateAttachmentUpload(filename: String = "screenshot_duo.png") {
        let remotePath = "/tmp/herdr-uploads/\(filename)"
        if draftCommand.isEmpty {
            draftCommand = remotePath
        } else {
            draftCommand += " \(remotePath)"
        }
    }
}

/// Unified Terminal-Centric SwiftUI Workspace matching both:
/// - **iPhone / iPhone Duo 5.4" Folded Portrait** (Full-bleed terminal + 2-row keypad + right thumb rail)
/// - **iPhone Duo 7.6" Unfolded / iPad / Mac** (Left Repo/Branch/Worktree sidebar + Center live terminal + 2-row keypad + right thumb rail)
public struct CodaAdaptiveRootView: View {
    @ObservedObject private var store: CodaWorkspaceStore

    public init(store: CodaWorkspaceStore = CodaWorkspaceStore()) {
        self.store = store
    }

    public var body: some View {
        GeometryReader { proxy in
            let isWide = proxy.size.width >= 680
            HStack(spacing: 0) {
                if isWide && store.showSidebar {
                    CodaRepoBranchSidebar(store: store)
                        .frame(width: 310)
                    Divider().background(Color.white.opacity(0.08))
                }

                CodaMainTerminalPane(store: store)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)

                Divider().background(Color.white.opacity(0.08))

                CodaRightVerticalThumbRail(store: store, isWide: isWide)
                    .frame(width: 56)
            }
            .background(CodaPalette.pureBlack.ignoresSafeArea())
        }
        .preferredColorScheme(.dark)
    }

    public static let sampleSessions: [CodaAgentSession] = [
        CodaAgentSession(
            id: "coda/main",
            address: "t1",
            machineName: "MacBook-Pro",
            machinePlatform: "macos",
            agentApp: "Opus 4.6",
            title: "语音模型 provider + baseurl + key 配置",
            statusMessage: "4 个终端运行中",
            cwd: "coda · main",
            state: .working,
            progress: 100,
            diffSummary: "+1,233 -116",
            screenPreview: """
              - 本地 Worktree 已整理：移除已合入 main 的临时分支并保持活跃工作区在线。
              - Herdr Core (`hc 0.1.20`) 全双工 PTY 流与 `hc doc.set` 语音配置已就绪。

            * Crunched for 4m 18s · done 12:37 PM

            ● Background command "cargo test -p coda-protocol -p coda-herdr" completed (exit code 0)

              Read 1 file

            ● main 分支测试与 CI 验证通过：X25519 + ChaCha20-Poly1305 零知识隧道与 GPUI 桌面端均已就绪。

            * Churned for 22s · done 12:43 PM
              new task? /clear... · ✓ E2EE Mesh Connected
            """
        ),
        CodaAgentSession(
            id: "coda/relay",
            address: "t2",
            machineName: "green-jp-02",
            machinePlatform: "linux",
            agentApp: "Sonnet 4.5",
            title: "Cloudflare Tunnel WSS 与内网 P2P 自动切换",
            statusMessage: "本轮完成",
            cwd: "coda-server · main",
            state: .done,
            progress: 100,
            diffSummary: "+842 -94",
            screenPreview: """
              - 主控面板绑定 127.0.0.1:8080，由 Cloudflare Tunnel 零公网端口反代至 https://codahub.dev。
              - SmartPeerClient 同 Wi-Fi 下自动协商 :24444 LAN P2P 直连（< 1ms），跨网无缝切回 WSS 443。

            * Baked for 38s · done 4:33 PM

            ● recap: 端到端加密通道与 RFC 8628 浏览器设备码授权全链验证完成。
            """
        ),
    ]
}

struct CodaBrandMarkView: View {
    var size: CGFloat = 26

    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: size * 0.24, style: .continuous)
                .fill(Color(red: 0.08, green: 0.09, blue: 0.12))
                .overlay(
                    RoundedRectangle(cornerRadius: size * 0.24, style: .continuous)
                        .stroke(Color.white.opacity(0.14), lineWidth: 1)
                )

            Circle()
                .trim(from: 0.12, to: 0.88)
                .stroke(
                    LinearGradient(
                        colors: [Color(red: 0.96, green: 0.62, blue: 0.04), CodaPalette.coralAgent],
                        startPoint: .topLeading,
                        endPoint: .bottomTrailing
                    ),
                    style: StrokeStyle(lineWidth: size * 0.1, lineCap: .round)
                )
                .rotationEffect(.degrees(18))
                .padding(size * 0.22)

            Image(systemName: "chevron.right")
                .font(.system(size: size * 0.28, weight: .bold, design: .monospaced))
                .foregroundStyle(.white)
                .offset(x: -size * 0.02)

            Circle()
                .fill(CodaPalette.workingEmerald)
                .frame(width: size * 0.16, height: size * 0.16)
                .offset(x: size * 0.21, y: -size * 0.18)
        }
        .frame(width: size, height: size)
    }
}

// MARK: - 1. Left Repo & Branch/Worktree Sidebar

struct CodaRepoBranchSidebar: View {
    @ObservedObject var store: CodaWorkspaceStore

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 10) {
                CodaBrandMarkView(size: 28)
                VStack(alignment: .leading, spacing: 2) {
                    Text(store.accountLabel)
                        .font(.system(size: 14, weight: .medium))
                    Text(store.routeMode == .lanDirect ? "已连接 · ⚡ 局域网直连" : "已连接 · \(store.routeMode.rawValue)")
                        .font(.system(size: 12, weight: .medium))
                        .foregroundStyle(CodaPalette.workingEmerald)
                }
                Spacer()
                HStack(spacing: 8) {
                    CircleIconButton(symbol: "person.badge.key") {
                        store.showAccountSheet.toggle()
                    }
                    CircleIconButton(symbol: "gearshape") {
                        store.showVoiceConfigSheet.toggle()
                    }
                    CircleIconButton(symbol: "sidebar.left") {
                        store.showSidebar.toggle()
                    }
                }
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 12)
            .sheet(isPresented: $store.showAccountSheet) {
                CodaAccountMeshSheet(store: store)
            }

            if !store.remoteHosts.isEmpty {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 6) {
                        ForEach(store.remoteHosts) { host in
                            Button {
                                store.selectedHostId = host.id
                            } label: {
                                HStack(spacing: 4) {
                                    Circle()
                                        .fill(host.isOnline ? CodaPalette.workingEmerald : Color.gray)
                                        .frame(width: 6, height: 6)
                                    Text(host.name)
                                        .font(.system(size: 11, weight: .medium))
                                }
                                .padding(.horizontal, 8)
                                .padding(.vertical, 5)
                                .background(
                                    store.selectedHostId == host.id
                                        ? CodaPalette.keyBorder
                                        : CodaPalette.keySurface
                                )
                                .clipShape(Capsule())
                            }
                            .buttonStyle(.plain)
                        }
                    }
                    .padding(.horizontal, 12)
                    .padding(.bottom, 8)
                }
            }

            Divider().background(Color.white.opacity(0.06))

            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    HStack(spacing: 6) {
                        Circle()
                            .fill(CodaPalette.workingEmerald)
                            .frame(width: 7, height: 7)
                        Text("coda")
                            .font(.system(size: 12, weight: .semibold))
                        Text("· Home")
                            .font(.system(size: 12))
                            .foregroundStyle(.secondary)
                    }
                    .padding(.horizontal, 6)

                    ForEach(store.sessions) { item in
                        Button {
                            store.selectedSessionId = item.id
                        } label: {
                            HStack(spacing: 10) {
                                RoundedRectangle(cornerRadius: 4)
                                    .fill(CodaPalette.coralAgent)
                                    .frame(width: 16, height: 16)
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(item.id)
                                        .font(.system(size: 13, weight: .medium, design: .monospaced))
                                        .lineLimit(1)
                                    Text(item.statusMessage)
                                        .font(.system(size: 11))
                                        .foregroundStyle(
                                            item.state == .done ? CodaPalette.completedSky : CodaPalette.workingEmerald
                                        )
                                }
                                Spacer()
                                if let diff = item.diffSummary {
                                    Text(diff)
                                        .font(.system(size: 11, design: .monospaced))
                                        .foregroundStyle(CodaPalette.workingEmerald)
                                }
                            }
                            .padding(.horizontal, 10)
                            .padding(.vertical, 8)
                            .background(
                                store.selectedSessionId == item.id
                                    ? Color.white.opacity(0.08)
                                    : Color.clear
                            )
                            .clipShape(RoundedRectangle(cornerRadius: 10))
                        }
                        .buttonStyle(.plain)
                    }
                }
                .padding(12)
            }
        }
        .background(CodaPalette.sidebarBg)
    }
}

// MARK: - 2. Center Full-Bleed Terminal + Overlapping Agent Switcher + 2-Row Keypad

struct CodaMainTerminalPane: View {
    @ObservedObject var store: CodaWorkspaceStore

    var body: some View {
        let session = store.activeSession
        VStack(alignment: .leading, spacing: 0) {
            // Top Header with Overlapping Agent Avatars
            HStack(alignment: .top) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(session.title)
                        .font(.system(size: 15, weight: .bold))
                        .lineLimit(1)
                    Text("\(session.cwd) · \(session.machineName)")
                        .font(.system(size: 12, design: .monospaced))
                        .foregroundStyle(.secondary)
                }
                Spacer()
                HStack(spacing: -8) {
                    ForEach(store.sessions) { s in
                        Button {
                            store.selectedSessionId = s.id
                        } label: {
                            ZStack {
                                Circle()
                                    .fill(CodaPalette.keySurface)
                                    .frame(width: 34, height: 34)
                                Circle()
                                    .stroke(
                                        store.selectedSessionId == s.id ? Color.white : CodaPalette.keyBorder,
                                        lineWidth: store.selectedSessionId == s.id ? 2 : 1
                                    )
                                    .frame(width: 34, height: 34)
                                Image(systemName: "terminal.fill")
                                    .font(.system(size: 13))
                                    .foregroundStyle(CodaPalette.coralAgent)
                            }
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
            .padding(.bottom, 8)

            // Full-Bleed Monospace Terminal Viewport
            ScrollView {
                VStack(alignment: .leading, spacing: 8) {
                    Text(session.screenPreview)
                        .font(.system(size: 13, design: .monospaced))
                        .foregroundStyle(Color(white: 0.92))
                        .frame(maxWidth: .infinity, alignment: .leading)

                    Divider().background(Color.white.opacity(0.15))

                    HStack(spacing: 6) {
                        Text(">")
                            .font(.system(size: 13, weight: .bold, design: .monospaced))
                            .foregroundStyle(.secondary)
                        Text(store.draftCommand.isEmpty ? "▯" : "[\(String(store.draftCommand.prefix(1)))]\(String(store.draftCommand.dropFirst()))")
                            .font(.system(size: 13, design: .monospaced))
                    }
                    .padding(.vertical, 2)

                    Divider().background(Color.white.opacity(0.15))

                    VStack(alignment: .leading, spacing: 2) {
                        Text("\(session.agentApp) | 5h 100% | 7d 总 77% | ⚠ 156239m...")
                            .font(.system(size: 11, design: .monospaced))
                            .foregroundStyle(.secondary)
                        Text("▶▶ \(store.bypassPermissions ? "bypass permissions on (shift+tab to cycle)" : "plan mode on") · ← 5 agents · PR")
                            .font(.system(size: 11, design: .monospaced))
                            .foregroundStyle(CodaPalette.coralAgent)
                    }
                }
                .padding(.vertical, 6)
            }

            // Bottom Dedicated 2-Row Tactile Terminal Keypad + Tall White Enter Button
            CodaTwoRowTerminalKeypad(store: store)
                .padding(.top, 6)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .background(CodaPalette.pureBlack)
    }
}

struct CodaTwoRowTerminalKeypad: View {
    @ObservedObject var store: CodaWorkspaceStore

    var body: some View {
        HStack(spacing: 6) {
            VStack(spacing: 6) {
                // Row 1: esc | tab | [⌨️ 🎙️] | ↑ | ⌫
                HStack(spacing: 6) {
                    KeypadButton(label: "esc") { store.draftCommand = "" }
                    KeypadButton(label: "tab") { store.bypassPermissions.toggle() }
                    KeypadButton(label: "⌨  |  🎙") { store.showVoiceConfigSheet.toggle() }
                    KeypadButton(label: "↑") {}
                    KeypadButton(label: "⌫") {
                        if !store.draftCommand.isEmpty {
                            store.draftCommand.removeLast()
                        }
                    }
                }
                // Row 2: ^C | ^D | / | ← | ↓ | →
                HStack(spacing: 6) {
                    KeypadButton(label: "^C", fg: CodaPalette.coralAgent) { store.draftCommand = "" }
                    KeypadButton(label: "^D") {}
                    KeypadButton(label: "/") { store.draftCommand = "/clear" }
                    KeypadButton(label: "←") {}
                    KeypadButton(label: "↓") {}
                    KeypadButton(label: "→") {}
                }
            }

            // Tall White Enter Key spanning both rows
            Button {
                store.draftCommand = ""
            } label: {
                Text("↵")
                    .font(.system(size: 20, weight: .bold, design: .monospaced))
                    .foregroundStyle(Color.black)
                    .frame(width: 58, height: 92)
                    .background(Color(white: 0.96))
                    .clipShape(RoundedRectangle(cornerRadius: 16))
            }
            .buttonStyle(.plain)
        }
        .frame(height: 92)
    }
}

struct KeypadButton: View {
    let label: String
    var fg: Color = .white
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Text(label)
                .font(.system(size: 13, weight: .medium, design: .monospaced))
                .foregroundStyle(fg)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .background(CodaPalette.keySurface)
                .overlay(
                    RoundedRectangle(cornerRadius: 12)
                        .stroke(CodaPalette.keyBorder, lineWidth: 1)
                )
                .clipShape(RoundedRectangle(cornerRadius: 12))
        }
        .buttonStyle(.plain)
    }
}

// MARK: - 3. Right Vertical Thumb Rail

struct CodaRightVerticalThumbRail: View {
    @ObservedObject var store: CodaWorkspaceStore
    let isWide: Bool

    var body: some View {
        VStack {
            // Top Status Capsule
            ZStack(alignment: .bottom) {
                Capsule()
                    .stroke(Color.red.opacity(0.35), lineWidth: 1)
                    .frame(width: 32, height: 54)
                Circle()
                    .fill(Color.red)
                    .frame(width: 10, height: 10)
                    .padding(.bottom, 8)
            }

            Spacer()

            VStack(spacing: 4) {
                Text("21:50")
                    .font(.system(size: 12, weight: .bold, design: .monospaced))
                ZStack {
                    Circle()
                        .stroke(CodaPalette.keyBorder, lineWidth: 2)
                        .frame(width: 40, height: 40)
                    VStack(spacing: 0) {
                        Text("68")
                            .font(.system(size: 10, weight: .bold, design: .monospaced))
                        Text(store.routeMode == .lanDirect ? "LAN" : "5G")
                            .font(.system(size: 9, design: .monospaced))
                            .foregroundStyle(store.routeMode == .lanDirect ? CodaPalette.workingEmerald : .secondary)
                    }
                }
            }

            Spacer()

            CircleIconButton(symbol: isWide && store.showSidebar ? "chevron.left" : "chevron.right") {
                store.showSidebar.toggle()
            }

            Spacer()

            VStack(spacing: 12) {
                Text("+").font(.system(size: 16, weight: .medium))
                Divider().frame(width: 16)
                Text("•••").font(.system(size: 10, weight: .bold))
            }
            .padding(.vertical, 10)
            .frame(width: 38)
            .background(CodaPalette.keySurface)
            .clipShape(Capsule())

            Spacer()

            VStack(spacing: 12) {
                Button {
                    store.simulateAttachmentUpload()
                } label: {
                    Image(systemName: "paperclip").font(.system(size: 13))
                }
                .buttonStyle(.plain)
                Divider().frame(width: 16)
                Image(systemName: "doc.on.clipboard").font(.system(size: 13))
            }
            .padding(.vertical, 10)
            .frame(width: 38)
            .background(CodaPalette.keySurface)
            .clipShape(Capsule())
        }
        .padding(.vertical, 12)
        .background(CodaPalette.sidebarBg)
    }
}

struct CircleIconButton: View {
    let symbol: String
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: symbol)
                .font(.system(size: 13, weight: .medium))
                .frame(width: 36, height: 36)
                .background(CodaPalette.keySurface)
                .overlay(Circle().stroke(CodaPalette.keyBorder, lineWidth: 1))
                .clipShape(Circle())
        }
        .buttonStyle(.plain)
    }
}

struct CodaAccountMeshSheet: View {
    @ObservedObject var store: CodaWorkspaceStore

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 10) {
                CodaBrandMarkView(size: 24)
                Text("连接 Coda 云端主控网络")
                    .font(.headline)
                Spacer()
                Button("关闭") {
                    store.showAccountSheet = false
                }
            }

            Text("Cloudflare Tunnel / 主控域名")
                .font(.caption)
                .foregroundStyle(.secondary)
            TextField("https://codahub.dev", text: $store.serverURLString)
                .textFieldStyle(.roundedBorder)

            Button {
                Task {
                    await store.startBrowserDeviceLogin()
                }
            } label: {
                Text("一键浏览器设备码授权 (RFC 8628)")
                    .frame(maxWidth: .infinity)
                    .padding(.vertical, 8)
            }
            .buttonStyle(.borderedProminent)

            if let userCode = store.pendingUserCode {
                VStack(alignment: .leading, spacing: 6) {
                    Text("设备授权码：\(userCode)")
                        .font(.system(size: 18, weight: .bold, design: .monospaced))
                    if let urlStr = store.pendingVerificationURL, let linkURL = URL(string: urlStr) {
                        Link("在 Safari 打开确认页面 →", destination: linkURL)
                            .font(.caption)
                    }
                }
                .padding(10)
                .background(Color.white.opacity(0.06))
                .clipShape(RoundedRectangle(cornerRadius: 10))
            }

            if let msg = store.authStatusMessage {
                Text(msg)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            Spacer()
        }
        .padding(20)
        .frame(minWidth: 340, minHeight: 280)
        .preferredColorScheme(.dark)
    }
}
