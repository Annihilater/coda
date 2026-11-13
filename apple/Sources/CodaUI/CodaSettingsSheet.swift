import Combine
import CodaKit
import SwiftUI

// MARK: - Settings, Account Auth & Machine Pairing Sheet

private final class SettingsFormModel: ObservableObject {
    @Published var authMode: Int = 0 // 0: Browser Device Auth, 1: Password, 2: Register
    @Published var emailInput: String = ""
    @Published var handleInput: String = ""
    @Published var passwordInput: String = ""
    @Published var suggestedHostName: String = ""
    @Published var copiedFlag: Bool = false
}

public struct CodaSettingsAndPairingSheet: View {
    @ObservedObject public var store: CodaWorkspaceStore
    @Environment(\.dismiss) private var dismiss
    @Environment(\.openURL) private var openURL
    @StateObject private var form = SettingsFormModel()

    public init(store: CodaWorkspaceStore) {
        self.store = store
    }

    public var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 18) {
                    languageAndServerSection
                    accountAuthSection
                    if store.isSignedIn {
                        machinePairingSection
                    }
                    voiceModelSection
                }
                .padding(18)
            }
            .background(CodaHerdrTheme.canvasBG)
            .navigationTitle(tr("Mesh Settings", "工作网设置", locale: store.locale))
            #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
            #endif
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button(tr("Done", "完成", locale: store.locale)) {
                        store.settings.save()
                        dismiss()
                    }
                    .foregroundStyle(CodaHerdrTheme.textPrimary)
                }
            }
        }
        .preferredColorScheme(.dark)
    }

    // MARK: - Language & Control Plane Server

    private var languageAndServerSection: some View {
        sectionCard(title: tr("General", "通用", locale: store.locale)) {
            VStack(alignment: .leading, spacing: 12) {
                HStack {
                    Text(tr("Language", "界面语言", locale: store.locale))
                        .font(.system(size: 13))
                        .foregroundStyle(CodaHerdrTheme.textSecondary)
                    Spacer()
                    Picker("", selection: Binding(
                        get: { store.settings.locale },
                        set: { store.setLocale($0) }
                    )) {
                        Text("English").tag(CodaUILocale.en)
                        Text("简体中文").tag(CodaUILocale.zh)
                    }
                    .pickerStyle(.segmented)
                    .frame(width: 170)
                }

                VStack(alignment: .leading, spacing: 6) {
                    Text(tr("Control Plane Server", "控制面服务器", locale: store.locale))
                        .font(.system(size: 12, weight: .medium))
                        .foregroundStyle(CodaHerdrTheme.textSecondary)
                    HStack(spacing: 8) {
                        TextField("https://codahub.dev", text: $store.settings.serverBaseURLString)
                            .font(.system(size: 13, design: .monospaced))
                            .foregroundStyle(CodaHerdrTheme.textPrimary)
                            .padding(.horizontal, 10)
                            .padding(.vertical, 8)
                            .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.canvasBG))
                            .codaNoAutocapAndSpelling()

                        Button(tr("Apply", "应用", locale: store.locale)) {
                            store.updateServerURL(store.settings.serverBaseURLString)
                        }
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(CodaHerdrTheme.textPrimary)
                        .padding(.horizontal, 12)
                        .padding(.vertical, 8)
                        .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.pillActiveBG))
                        .buttonStyle(.plain)
                    }
                }
            }
        }
    }

    // MARK: - Authentication (Browser Device Code & Password)

    private var accountAuthSection: some View {
        sectionCard(title: tr("Account", "账户认证", locale: store.locale)) {
            if store.isSignedIn {
                VStack(alignment: .leading, spacing: 12) {
                    HStack(spacing: 10) {
                        Image(systemName: "checkmark.shield.fill")
                            .foregroundStyle(CodaHerdrTheme.greenDiff)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(store.settings.ownerHandle.isEmpty ? tr("Signed In", "已登录", locale: store.locale) : "@\(store.settings.ownerHandle)")
                                .font(.system(size: 14, weight: .semibold))
                                .foregroundStyle(CodaHerdrTheme.textPrimary)
                            Text(store.settings.serverBaseURLString)
                                .font(.system(size: 11, design: .monospaced))
                                .foregroundStyle(CodaHerdrTheme.textSecondary)
                        }
                        Spacer()
                    }

                    HStack(spacing: 10) {
                        Button {
                            Task { await store.refreshMachinesAndSessions() }
                        } label: {
                            Label(tr("Sync Now", "立即同步", locale: store.locale), systemImage: "arrow.clockwise")
                                .font(.system(size: 12, weight: .semibold))
                                .foregroundStyle(CodaHerdrTheme.textPrimary)
                                .padding(.horizontal, 12)
                                .padding(.vertical, 8)
                                .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.pillActiveBG))
                        }
                        .buttonStyle(.plain)

                        Spacer()

                        Button(role: .destructive) {
                            store.signOut()
                        } label: {
                            Text(tr("Sign Out", "退出登录", locale: store.locale))
                                .font(.system(size: 12, weight: .semibold))
                                .foregroundStyle(CodaHerdrTheme.redDiff)
                                .padding(.horizontal, 12)
                                .padding(.vertical, 8)
                                .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.redDiff.opacity(0.14)))
                        }
                        .buttonStyle(.plain)
                    }
                }
            } else {
                VStack(alignment: .leading, spacing: 12) {
                    Picker("", selection: $form.authMode) {
                        Text(tr("Browser", "浏览器授权", locale: store.locale)).tag(0)
                        Text(tr("Sign In", "密码登录", locale: store.locale)).tag(1)
                        Text(tr("Register", "注册", locale: store.locale)).tag(2)
                    }
                    .pickerStyle(.segmented)

                    if form.authMode == 0 {
                        if let auth = store.pendingDeviceAuth {
                            VStack(alignment: .leading, spacing: 10) {
                                Text(tr("Confirm this code in your browser:", "请在浏览器中确认此授权码：", locale: store.locale))
                                    .font(.system(size: 12))
                                    .foregroundStyle(CodaHerdrTheme.textSecondary)

                                Text(auth.userCode)
                                    .font(.system(size: 24, weight: .bold, design: .monospaced))
                                    .foregroundStyle(CodaHerdrTheme.amberNeedsYou)
                                    .frame(maxWidth: .infinity)
                                    .padding(.vertical, 10)
                                    .background(RoundedRectangle(cornerRadius: 10).fill(CodaHerdrTheme.canvasBG))

                                HStack(spacing: 10) {
                                    if let url = URL(string: auth.verificationURIComplete) {
                                        Button {
                                            openURL(url)
                                        } label: {
                                            Label(tr("Open Browser", "打开浏览器", locale: store.locale), systemImage: "safari")
                                                .font(.system(size: 13, weight: .semibold))
                                                .foregroundStyle(.black)
                                                .frame(maxWidth: .infinity)
                                                .padding(.vertical, 9)
                                                .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.textPrimary))
                                        }
                                        .buttonStyle(.plain)
                                    }

                                    Button {
                                        store.cancelBrowserDeviceAuth()
                                    } label: {
                                        Text(tr("Cancel", "取消", locale: store.locale))
                                            .font(.system(size: 13))
                                            .foregroundStyle(CodaHerdrTheme.textSecondary)
                                            .padding(.horizontal, 14)
                                            .padding(.vertical, 9)
                                            .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.pillActiveBG))
                                    }
                                    .buttonStyle(.plain)
                                }

                                if store.isPollingDeviceAuth {
                                    HStack(spacing: 8) {
                                        ProgressView().controlSize(.small)
                                        Text(tr("Waiting for approval...", "等待浏览器确认...", locale: store.locale))
                                            .font(.system(size: 12))
                                            .foregroundStyle(CodaHerdrTheme.textSecondary)
                                    }
                                }
                            }
                        } else {
                            Button {
                                Task { await store.beginBrowserDeviceAuth() }
                            } label: {
                                Label(tr("Continue with Browser (RFC 8628)", "使用浏览器扫码/授权登录", locale: store.locale), systemImage: "person.badge.key.fill")
                                    .font(.system(size: 13, weight: .semibold))
                                    .foregroundStyle(.black)
                                    .frame(maxWidth: .infinity)
                                    .padding(.vertical, 10)
                                    .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.textPrimary))
                            }
                            .buttonStyle(.plain)
                        }
                    } else {
                        VStack(spacing: 10) {
                            TextField(tr("Email or Handle", "邮箱或用户名", locale: store.locale), text: $form.emailInput)
                                .font(.system(size: 13))
                                .padding(.horizontal, 10)
                                .padding(.vertical, 8)
                                .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.canvasBG))
                                .codaNoAutocapAndSpelling()

                            if form.authMode == 2 {
                                TextField(tr("Handle (e.g. alice)", "用户名（如 alice）", locale: store.locale), text: $form.handleInput)
                                    .font(.system(size: 13))
                                    .padding(.horizontal, 10)
                                    .padding(.vertical, 8)
                                    .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.canvasBG))
                                    .codaNoAutocapAndSpelling()
                            }

                            SecureField(tr("Password", "密码", locale: store.locale), text: $form.passwordInput)
                                .font(.system(size: 13))
                                .padding(.horizontal, 10)
                                .padding(.vertical, 8)
                                .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.canvasBG))

                            Button {
                                Task {
                                    if form.authMode == 2 {
                                        await store.registerAccount(email: form.emailInput, handle: form.handleInput, password: form.passwordInput)
                                    } else {
                                        await store.signInWithPassword(emailOrHandle: form.emailInput, password: form.passwordInput)
                                    }
                                }
                            } label: {
                                Text(form.authMode == 2 ? tr("Create Account", "创建账户", locale: store.locale) : tr("Sign In", "登录", locale: store.locale))
                                    .font(.system(size: 13, weight: .semibold))
                                    .foregroundStyle(.black)
                                    .frame(maxWidth: .infinity)
                                    .padding(.vertical, 10)
                                    .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.textPrimary))
                            }
                            .buttonStyle(.plain)
                        }
                    }

                    if let err = store.authError {
                        Text(err)
                            .font(.system(size: 12))
                            .foregroundStyle(CodaHerdrTheme.redDiff)
                    }
                }
            }
        }
    }

    // MARK: - Machine Pairing & Paired Hosts

    private var machinePairingSection: some View {
        sectionCard(title: tr("Add Machine & Paired Hosts", "添加主机与已配对设备", locale: store.locale)) {
            VStack(alignment: .leading, spacing: 12) {
                HStack(spacing: 8) {
                    TextField(tr("Optional host label (e.g. Studio)", "可选主机名（如 Studio）", locale: store.locale), text: $form.suggestedHostName)
                        .font(.system(size: 13))
                        .padding(.horizontal, 10)
                        .padding(.vertical, 8)
                        .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.canvasBG))

                    Button {
                        Task { await store.generateMachinePairingCommand(suggestedName: form.suggestedHostName) }
                    } label: {
                        Text(tr("Generate Code", "生成配对码", locale: store.locale))
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundStyle(.black)
                            .padding(.horizontal, 12)
                            .padding(.vertical, 8)
                            .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.textPrimary))
                    }
                    .buttonStyle(.plain)
                }

                if let cmd = store.pairingCommand, !cmd.isEmpty {
                    VStack(alignment: .leading, spacing: 6) {
                        if let code = store.pairingCode {
                            Text(tr("Pairing Code: \(code)", "配对码：\(code)", locale: store.locale))
                                .font(.system(size: 12, weight: .semibold, design: .monospaced))
                                .foregroundStyle(CodaHerdrTheme.amberNeedsYou)
                        }
                        Text(cmd)
                            .font(.system(size: 11, design: .monospaced))
                            .foregroundStyle(CodaHerdrTheme.textPrimary)
                            .textSelection(.enabled)
                            .padding(10)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.canvasBG))

                        Button {
                            copyTextToClipboard(cmd)
                            form.copiedFlag = true
                        } label: {
                            Label(
                                form.copiedFlag ? tr("Copied Command", "已复制命令", locale: store.locale) : tr("Copy Command", "复制配对命令", locale: store.locale),
                                systemImage: form.copiedFlag ? "checkmark" : "doc.on.doc"
                            )
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundStyle(CodaHerdrTheme.cyanWorking)
                        }
                        .buttonStyle(.plain)
                    }
                }

                if !store.remoteMachines.isEmpty {
                    Divider().background(CodaHerdrTheme.borderHairline)
                    ForEach(store.remoteMachines) { dev in
                        HStack(spacing: 8) {
                            Circle()
                                .fill(dev.isOnline ? CodaHerdrTheme.greenDiff : CodaHerdrTheme.textDim)
                                .frame(width: 8, height: 8)
                            Text(dev.name)
                                .font(.system(size: 13, weight: .medium))
                                .foregroundStyle(CodaHerdrTheme.textPrimary)
                            Text(dev.platform)
                                .font(.system(size: 11, design: .monospaced))
                                .foregroundStyle(CodaHerdrTheme.textSecondary)
                            Spacer()
                            if let route = store.machineRouteModes[dev.id] {
                                Text(route.rawValue)
                                    .font(.system(size: 10, design: .monospaced))
                                    .foregroundStyle(CodaHerdrTheme.cyanWorking)
                            } else {
                                Text(dev.isOnline ? tr("Online", "在线", locale: store.locale) : tr("Offline", "离线", locale: store.locale))
                                    .font(.system(size: 11))
                                    .foregroundStyle(dev.isOnline ? CodaHerdrTheme.greenDiff : CodaHerdrTheme.textDim)
                            }
                        }
                    }
                }
            }
        }
    }

    // MARK: - Voice Model Configuration

    private var voiceModelSection: some View {
        sectionCard(title: tr("Voice Dictation Model", "语音识别模型", locale: store.locale)) {
            VStack(alignment: .leading, spacing: 10) {
                labeledInput(tr("Provider", "服务商", locale: store.locale), text: $store.settings.voiceConfig.provider)
                labeledInput(tr("Base URL", "接口地址", locale: store.locale), text: $store.settings.voiceConfig.baseURL)
                labeledInput(tr("Model", "模型名称", locale: store.locale), text: $store.settings.voiceConfig.model)
                VStack(alignment: .leading, spacing: 4) {
                    Text("API Key")
                        .font(.system(size: 11))
                        .foregroundStyle(CodaHerdrTheme.textSecondary)
                    SecureField(tr("Enter API Key", "输入 API Key", locale: store.locale), text: $store.settings.voiceConfig.apiKey)
                        .font(.system(size: 12, design: .monospaced))
                        .padding(.horizontal, 10)
                        .padding(.vertical, 8)
                        .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.canvasBG))
                }
            }
        }
    }

    private func labeledInput(_ title: String, text: Binding<String>) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title)
                .font(.system(size: 11))
                .foregroundStyle(CodaHerdrTheme.textSecondary)
            TextField(title, text: text)
                .font(.system(size: 12, design: .monospaced))
                .padding(.horizontal, 10)
                .padding(.vertical, 8)
                .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.canvasBG))
                .codaNoAutocapAndSpelling()
        }
    }

    private func sectionCard<Content: View>(title: String, @ViewBuilder content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(title.uppercased())
                .font(.system(size: 11, weight: .semibold))
                .tracking(0.6)
                .foregroundStyle(CodaHerdrTheme.textSecondary)
            content()
        }
        .padding(14)
        .background(
            RoundedRectangle(cornerRadius: 14, style: .continuous)
                .fill(CodaHerdrTheme.cardBG)
                .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
        )
    }
}
