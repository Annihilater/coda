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
    @Published public var routeMode: SmartPathRouteMode = .cnIPv6P2P

    public init(initialSessions: [CodaAgentSession] = CodaAdaptiveRootView.sampleSessions) {
        self.sessions = initialSessions
        self.selectedSessionId = initialSessions.first?.id ?? ""
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
            machineName: "Home",
            machinePlatform: "macos",
            agentApp: "Opus 5.5",
            title: "语音模型 provider + baseurl + key 配置",
            statusMessage: "4 个终端运行中",
            cwd: "coda · main",
            state: .working,
            progress: 100,
            diffSummary: "+1,233 -116",
            screenPreview: """
                里，一个是我这次用来合 #135 的临时目录。
              - 本地分支删了 8 个：2.25 和 2.26
                发版留下的、内容已在 main 里的分支，加上
                #113 的分支。

              还剩
              - .coda/worktrees/a46dbd6c... : #135
                的工作区，分支已经合并了。它是 Herdr
                管理的工作区，你在 Coda 里把这个工作区删掉就行。

            * Crunched for 8m 41s · done 12:37 PM

            ● Background command "Wait for main CI at the integrated tip" completed (exit code 0)

              Read 1 file

            ● main 上的 CI 也通过了：最新提交 974dce2e，合入了 #135 和 5 个依赖升级，CI 结果为 success。整合到此全部完成。

            * Churned for 22s · done 12:43 PM
              new task? /clear... · ✓ Update installed
            """
        ),
        CodaAgentSession(
            id: "coda/pixel",
            address: "t2",
            machineName: "Home",
            machinePlatform: "macos",
            agentApp: "Fable 5",
            title: "Pixel app 项目进展",
            statusMessage: "本轮完成",
            cwd: "cc-host · main",
            state: .done,
            progress: 100,
            diffSummary: "+2,687 -306",
            screenPreview: """
              - 管理员门生效：/accounts/new 自托管确认步的「设备」分叉和 /pixel-setup 页都只对 isAdmin 渲染；
              - assetlinks.json 返回 404 —— 预期状态（等 keystore 生成后回填）。

            * Baked for 54s · done 4:33 PM
            """
        ),
    ]
}

// MARK: - 1. Left Repo & Branch/Worktree Sidebar (Image 2 Left Column)

struct CodaRepoBranchSidebar: View {
    @ObservedObject var store: CodaWorkspaceStore

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                VStack(alignment: .leading, spacing: 2) {
                    Text("vampire@coda.dev")
                        .font(.system(size: 14, weight: .medium))
                    Text("已连接")
                        .font(.system(size: 12, weight: .medium))
                        .foregroundStyle(CodaPalette.workingEmerald)
                }
                Spacer()
                HStack(spacing: 8) {
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
                        Text("5G")
                            .font(.system(size: 9, design: .monospaced))
                            .foregroundStyle(.secondary)
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
