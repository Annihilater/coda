import Combine
import CodaKit
import SwiftUI

// MARK: - Settings, Account Auth & Machine Pairing Sheet

private final class SettingsFormModel: ObservableObject {
    @Published var serverURLInput: String = ""
    @Published var authMode: Int = 0 // 0: Browser Device Auth, 1: Password, 2: Register, 3: Reset Password
    @Published var emailInput: String = ""
    @Published var handleInput: String = ""
    @Published var passwordInput: String = ""
    @Published var inviteCodeInput: String = ""
    @Published var resetTokenOrURLInput: String = ""
    @Published var resetNewPasswordInput: String = ""
    @Published var resetConfirmPasswordInput: String = ""
    @Published var suggestedHostName: String = ""
    @Published var approveUserCodeInput: String = ""
    @Published var renamingDeviceId: UUID? = nil
    @Published var renameDeviceInput: String = ""
    @Published var showPasswordEditor: Bool = false
    @Published var currentPasswordInput: String = ""
    @Published var newPasswordInput: String = ""
    @Published var showDeleteAccount: Bool = false
    @Published var deletePasswordInput: String = ""
    @Published var copiedFlag: Bool = false
    @Published var newInviteNoteInput: String = ""
    @Published var copiedInviteCode: String? = nil
    @Published var copiedResetUserId: UUID? = nil
    @Published var mintingResetUserId: UUID? = nil
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
            ScrollViewReader { proxy in
                ScrollView {
                    VStack(alignment: .leading, spacing: 18) {
                        languageAndServerSection
                        accountAuthSection
                        if store.isSignedIn {
                            machinePairingSection
                                .id("machine_pairing_section")
                            if store.currentUser?.isAdmin == true {
                                adminFriendsAndInvitesSection
                            }
                            offlineWebhooksSection
                        }
                        voiceModelSection
                    }
                    .padding(18)
                }
                .onAppear {
                    form.serverURLInput = store.settings.serverBaseURLString
                    if let initialMode = store.settingsInitialAuthMode, !store.isSignedIn {
                        form.authMode = initialMode
                        store.settingsInitialAuthMode = nil
                        if initialMode == 3,
                           form.resetTokenOrURLInput.isEmpty,
                           let clip = readTextFromClipboard(),
                           extractResetTokenFromInput(clip) != nil {
                            form.resetTokenOrURLInput = clip
                            if let origin = extractServerOriginFromAuthLink(clip) {
                                form.serverURLInput = origin
                            }
                        } else if initialMode == 2,
                                  form.inviteCodeInput.isEmpty,
                                  let clip = readTextFromClipboard(),
                                  clip.contains("invite=") {
                            form.inviteCodeInput = extractInviteCodeFromInput(clip)
                            if let origin = extractServerOriginFromAuthLink(clip) {
                                form.serverURLInput = origin
                            }
                        }
                    }
                    if store.isSignedIn {
                        Task {
                            await store.loadNotificationWebhookSettings()
                            await store.loadAdminDashboardData()
                        }
                    }
                    if store.settingsInitialFocusPairing && store.isSignedIn {
                        store.settingsInitialFocusPairing = false
                        Task { @MainActor in
                            withAnimation(.easeOut(duration: 0.2)) {
                                proxy.scrollTo("machine_pairing_section", anchor: .top)
                            }
                        }
                    }
                }
                .onChange(of: store.isSignedIn) { _, signedIn in
                    if signedIn {
                        form.passwordInput = ""
                        form.inviteCodeInput = ""
                        Task {
                            await store.loadNotificationWebhookSettings()
                            await store.loadAdminDashboardData()
                        }
                    }
                }
                .onChange(of: store.currentUser?.isAdmin) { _, isAdmin in
                    if isAdmin == true {
                        Task { await store.loadAdminDashboardData() }
                    }
                }
            }
            .background(CodaHerdrTheme.canvasBG)
            .navigationTitle(tr("Mesh Settings", "工作网设置", locale: store.locale))
            #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
            #endif
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button(tr("Done", "完成", locale: store.locale)) {
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
                        TextField("https://codahub.dev", text: $form.serverURLInput)
                            .font(.system(size: 13, design: .monospaced))
                            .foregroundStyle(CodaHerdrTheme.textPrimary)
                            .padding(.horizontal, 10)
                            .padding(.vertical, 8)
                            .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.canvasBG))
                            .codaNoAutocapAndSpelling()

                        Button(tr("Apply", "应用", locale: store.locale)) {
                            store.updateServerURL(form.serverURLInput)
                            form.serverURLInput = store.settings.serverBaseURLString
                        }
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(CodaHerdrTheme.textPrimary)
                        .padding(.horizontal, 12)
                        .padding(.vertical, 8)
                        .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.pillActiveBG))
                        .buttonStyle(.plain)
                    }
                }

                Divider().overlay(CodaHerdrTheme.borderHairline)

                Toggle(isOn: Binding(
                    get: { store.settings.notificationsEnabled },
                    set: { store.setNotificationsEnabled($0) }
                )) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(tr("System Alerts (OSC 7501)", "系统通知 (OSC 7501)", locale: store.locale))
                            .font(.system(size: 13))
                            .foregroundStyle(CodaHerdrTheme.textPrimary)
                        Text(tr("Notify when an AI agent needs approval or finishes", "后台 Agent 需要审批或任务完成时发送系统通知", locale: store.locale))
                            .font(.system(size: 11))
                            .foregroundStyle(CodaHerdrTheme.textSecondary)
                    }
                }
                .tint(CodaHerdrTheme.amberNeedsYou)

                if store.isSignedIn {
                    HStack {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(tr("Smart-Path Auto Hot-Switch", "智能路径热切直连", locale: store.locale))
                                .font(.system(size: 13))
                                .foregroundStyle(CodaHerdrTheme.textPrimary)
                            Text(tr("Auto-upgrades Relay to LAN Direct on Wi-Fi/Ethernet", "进入局域网时自动从智能中继无感热切至直连", locale: store.locale))
                                .font(.system(size: 11))
                                .foregroundStyle(CodaHerdrTheme.textSecondary)
                        }
                        Spacer()
                        Button {
                            Task { await store.probeDirectLanNow() }
                        } label: {
                            Label(tr("Probe LAN", "探测直连", locale: store.locale), systemImage: "bolt.horizontal.fill")
                                .font(.system(size: 11.5, weight: .semibold))
                                .foregroundStyle(CodaHerdrTheme.textPrimary)
                                .padding(.horizontal, 10)
                                .padding(.vertical, 6)
                                .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.pillActiveBG))
                        }
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
                            HStack(spacing: 6) {
                                Text(store.settings.ownerHandle.isEmpty ? tr("Signed In", "已登录", locale: store.locale) : "@\(store.settings.ownerHandle)")
                                    .font(.system(size: 14, weight: .semibold))
                                    .foregroundStyle(CodaHerdrTheme.textPrimary)
                                if store.currentUser?.isAdmin == true {
                                    Text("ADMIN")
                                        .font(.system(size: 9.5, weight: .bold, design: .monospaced))
                                        .foregroundStyle(CodaHerdrTheme.cyanWorking)
                                        .padding(.horizontal, 6)
                                        .padding(.vertical, 2)
                                        .background(Capsule().fill(CodaHerdrTheme.cyanWorking.opacity(0.16)))
                                }
                            }
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

                    if let status = store.accountStatusMessage {
                        Text(status)
                            .font(.system(size: 12))
                            .foregroundStyle(CodaHerdrTheme.greenDiff)
                    }
                    if let err = store.authError {
                        Text(err)
                            .font(.system(size: 12))
                            .foregroundStyle(CodaHerdrTheme.redDiff)
                    }

                    Divider().background(CodaHerdrTheme.borderHairline)

                    HStack(spacing: 8) {
                        Button {
                            form.showPasswordEditor.toggle()
                        } label: {
                            Text(tr("Change Password", "修改密码", locale: store.locale))
                                .font(.system(size: 11.5, weight: .medium))
                                .foregroundStyle(CodaHerdrTheme.textPrimary)
                                .padding(.horizontal, 10)
                                .padding(.vertical, 6)
                                .background(RoundedRectangle(cornerRadius: 7).fill(CodaHerdrTheme.canvasBG))
                        }
                        .buttonStyle(.plain)

                        Button {
                            Task { await store.resetLocalIdentity() }
                        } label: {
                            Text(tr("Rotate Local Key", "重置本机密钥", locale: store.locale))
                                .font(.system(size: 11.5, weight: .medium))
                                .foregroundStyle(CodaHerdrTheme.amberNeedsYou)
                                .padding(.horizontal, 10)
                                .padding(.vertical, 6)
                                .background(RoundedRectangle(cornerRadius: 7).fill(CodaHerdrTheme.amberNeedsYou.opacity(0.12)))
                        }
                        .buttonStyle(.plain)

                        Spacer()

                        Button {
                            form.showDeleteAccount.toggle()
                        } label: {
                            Text(tr("Delete Account", "注销账户", locale: store.locale))
                                .font(.system(size: 11.5, weight: .medium))
                                .foregroundStyle(CodaHerdrTheme.redDiff)
                        }
                        .buttonStyle(.plain)
                    }

                    if form.showPasswordEditor {
                        VStack(spacing: 8) {
                            SecureField(tr("Current password", "当前密码", locale: store.locale), text: $form.currentPasswordInput)
                                .font(.system(size: 12))
                                .padding(.horizontal, 10)
                                .padding(.vertical, 7)
                                .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.canvasBG))
                            SecureField(tr("New password (min 8 chars)", "新密码（至少 8 位）", locale: store.locale), text: $form.newPasswordInput)
                                .font(.system(size: 12))
                                .padding(.horizontal, 10)
                                .padding(.vertical, 7)
                                .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.canvasBG))
                            Button {
                                Task {
                                    let ok = await store.changePassword(
                                        current: form.currentPasswordInput,
                                        new: form.newPasswordInput
                                    )
                                    if ok {
                                        form.currentPasswordInput = ""
                                        form.newPasswordInput = ""
                                        form.showPasswordEditor = false
                                    }
                                }
                            } label: {
                                Text(tr("Update Password", "确认修改密码", locale: store.locale))
                                    .font(.system(size: 12, weight: .semibold))
                                    .foregroundStyle(.black)
                                    .frame(maxWidth: .infinity)
                                    .padding(.vertical, 8)
                                    .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.textPrimary))
                            }
                            .buttonStyle(.plain)
                        }
                    }

                    if form.showDeleteAccount {
                        VStack(alignment: .leading, spacing: 8) {
                            Text(tr("This permanently deletes your account, paired devices, and sessions.", "此操作将永久删除你的账户、全部已配对设备与会话。", locale: store.locale))
                                .font(.system(size: 11))
                                .foregroundStyle(CodaHerdrTheme.redDiff)
                            SecureField(tr("Confirm current password", "输入当前密码确认注销", locale: store.locale), text: $form.deletePasswordInput)
                                .font(.system(size: 12))
                                .padding(.horizontal, 10)
                                .padding(.vertical, 7)
                                .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.canvasBG))
                            Button(role: .destructive) {
                                Task {
                                    let ok = await store.deleteAccount(currentPassword: form.deletePasswordInput)
                                    if ok {
                                        form.deletePasswordInput = ""
                                        form.showDeleteAccount = false
                                    }
                                }
                            } label: {
                                Text(tr("Permanently Delete Account", "永久注销账户", locale: store.locale))
                                    .font(.system(size: 12, weight: .semibold))
                                    .foregroundStyle(.white)
                                    .frame(maxWidth: .infinity)
                                    .padding(.vertical, 8)
                                    .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.redDiff))
                            }
                            .buttonStyle(.plain)
                        }
                    }
                }
            } else {
                VStack(alignment: .leading, spacing: 12) {
                    Picker("", selection: $form.authMode) {
                        Text(tr("Browser", "浏览器", locale: store.locale)).tag(0)
                        Text(tr("Sign In", "登录", locale: store.locale)).tag(1)
                        Text(tr("Register", "注册", locale: store.locale)).tag(2)
                        Text(tr("Reset", "重置密码", locale: store.locale)).tag(3)
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
                    } else if form.authMode == 3 {
                        VStack(alignment: .leading, spacing: 10) {
                            Text(tr(
                                "Reset Forgotten Password — paste the reset link or rst_... token sent by your admin on WeChat, then set a new password.",
                                "重置忘记的密码 — 粘贴管理员通过微信发给你的重置链接或 rst_... 令牌，并设置新密码。",
                                locale: store.locale
                            ))
                            .font(.system(size: 11.5))
                            .foregroundStyle(CodaHerdrTheme.textSecondary)

                            HStack(spacing: 8) {
                                TextField(
                                    tr("Reset link or rst_... token", "粘贴重置链接或 rst_... 令牌", locale: store.locale),
                                    text: $form.resetTokenOrURLInput
                                )
                                .font(.system(size: 12.5, design: .monospaced))
                                .padding(.horizontal, 10)
                                .padding(.vertical, 8)
                                .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.canvasBG))
                                .codaNoAutocapAndSpelling()

                                Button {
                                    if let clip = readTextFromClipboard(), !clip.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                                        form.resetTokenOrURLInput = clip
                                        if let origin = extractServerOriginFromAuthLink(clip) {
                                            form.serverURLInput = origin
                                        }
                                    }
                                } label: {
                                    Label(tr("Paste", "粘贴", locale: store.locale), systemImage: "doc.on.clipboard")
                                        .font(.system(size: 11.5, weight: .semibold))
                                        .foregroundStyle(CodaHerdrTheme.cyanWorking)
                                        .padding(.horizontal, 10)
                                        .padding(.vertical, 8)
                                        .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.pillActiveBG))
                                }
                                .buttonStyle(.plain)
                            }

                            SecureField(tr("New password (min 8 chars)", "新密码（至少 8 位）", locale: store.locale), text: $form.resetNewPasswordInput)
                                .font(.system(size: 13))
                                .padding(.horizontal, 10)
                                .padding(.vertical, 8)
                                .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.canvasBG))

                            SecureField(tr("Confirm new password", "确认新密码", locale: store.locale), text: $form.resetConfirmPasswordInput)
                                .font(.system(size: 13))
                                .padding(.horizontal, 10)
                                .padding(.vertical, 8)
                                .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.canvasBG))

                            Button {
                                guard form.resetNewPasswordInput == form.resetConfirmPasswordInput else {
                                    store.authError = tr("New passwords do not match.", "两次输入的新密码不一致。", locale: store.locale)
                                    return
                                }
                                Task {
                                    if let res = await store.resetPasswordWithToken(
                                        rawTokenOrURL: form.resetTokenOrURLInput,
                                        newPassword: form.resetNewPasswordInput
                                    ) {
                                        form.serverURLInput = store.settings.serverBaseURLString
                                        if !res.handle.isEmpty {
                                            form.emailInput = res.handle
                                        }
                                        form.resetTokenOrURLInput = ""
                                        form.resetNewPasswordInput = ""
                                        form.resetConfirmPasswordInput = ""
                                        form.passwordInput = ""
                                        form.authMode = 1
                                    }
                                }
                            } label: {
                                HStack(spacing: 6) {
                                    if store.isResettingPassword {
                                        ProgressView().controlSize(.small).tint(.black)
                                    }
                                    Text(
                                        store.isResettingPassword
                                            ? tr("Resetting Password…", "正在重置密码…", locale: store.locale)
                                            : tr("Reset Forgotten Password", "确认重置密码", locale: store.locale)
                                    )
                                    .font(.system(size: 13, weight: .semibold))
                                }
                                .foregroundStyle(.black)
                                .frame(maxWidth: .infinity)
                                .padding(.vertical, 10)
                                .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.cyanWorking))
                            }
                            .buttonStyle(.plain)
                            .disabled(
                                store.isResettingPassword
                                    || form.resetTokenOrURLInput.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                                    || form.resetNewPasswordInput.isEmpty
                            )
                        }
                    } else {
                        VStack(spacing: 10) {
                            TextField(
                                form.authMode == 2
                                    ? tr("Email", "邮箱", locale: store.locale)
                                    : tr("Email or Handle", "邮箱或用户名", locale: store.locale),
                                text: $form.emailInput
                            )
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

                                HStack(spacing: 8) {
                                    TextField(tr("Invite Code or Link (optional)", "邀请码或邀请链接（如服务器要求）", locale: store.locale), text: $form.inviteCodeInput)
                                        .font(.system(size: 13, design: .monospaced))
                                        .padding(.horizontal, 10)
                                        .padding(.vertical, 8)
                                        .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.canvasBG))
                                        .codaNoAutocapAndSpelling()

                                    Button {
                                        if let clip = readTextFromClipboard(), !clip.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                                            form.inviteCodeInput = extractInviteCodeFromInput(clip)
                                            if let origin = extractServerOriginFromAuthLink(clip) {
                                                form.serverURLInput = origin
                                            }
                                        }
                                    } label: {
                                        Image(systemName: "doc.on.clipboard")
                                            .font(.system(size: 12, weight: .semibold))
                                            .foregroundStyle(CodaHerdrTheme.cyanWorking)
                                            .padding(.horizontal, 10)
                                            .padding(.vertical, 8)
                                            .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.pillActiveBG))
                                    }
                                    .buttonStyle(.plain)
                                }
                            }

                            SecureField(tr("Password", "密码", locale: store.locale), text: $form.passwordInput)
                                .font(.system(size: 13))
                                .padding(.horizontal, 10)
                                .padding(.vertical, 8)
                                .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.canvasBG))

                            Button {
                                Task {
                                    if form.authMode == 2 {
                                        await store.registerAccount(
                                            email: form.emailInput,
                                            handle: form.handleInput,
                                            password: form.passwordInput,
                                            inviteCode: form.inviteCodeInput
                                        )
                                        form.serverURLInput = store.settings.serverBaseURLString
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

                            if form.authMode == 1 {
                                Button {
                                    store.authError = nil
                                    if form.resetTokenOrURLInput.isEmpty,
                                       let clip = readTextFromClipboard(),
                                       extractResetTokenFromInput(clip) != nil {
                                        form.resetTokenOrURLInput = clip
                                    }
                                    form.authMode = 3
                                } label: {
                                    HStack(spacing: 4) {
                                        Image(systemName: "key.horizontal.fill")
                                            .font(.system(size: 11))
                                        Text(tr("Reset Forgotten Password (paste WeChat reset link)", "忘记密码？粘贴管理员重置链接", locale: store.locale))
                                            .font(.system(size: 12, weight: .medium))
                                    }
                                    .foregroundStyle(CodaHerdrTheme.cyanWorking)
                                    .frame(maxWidth: .infinity, alignment: .leading)
                                    .padding(.top, 2)
                                }
                                .buttonStyle(.plain)
                            }
                        }
                    }

                    if let status = store.accountStatusMessage {
                        HStack(spacing: 6) {
                            Image(systemName: "checkmark.circle.fill")
                                .font(.system(size: 12))
                            Text(status)
                                .font(.system(size: 12, weight: .medium))
                        }
                        .foregroundStyle(CodaHerdrTheme.greenDiff)
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
                        form.copiedFlag = false
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

                Divider().background(CodaHerdrTheme.borderHairline)

                VStack(alignment: .leading, spacing: 6) {
                    Text(tr("Approve Host Login Code (RFC 8628)", "批准主机授权码（如 ABCD-2345）", locale: store.locale))
                        .font(.system(size: 11.5, weight: .medium))
                        .foregroundStyle(CodaHerdrTheme.textSecondary)
                    HStack(spacing: 8) {
                        TextField("ABCD-2345", text: $form.approveUserCodeInput)
                            .font(.system(size: 13, design: .monospaced))
                            .padding(.horizontal, 10)
                            .padding(.vertical, 8)
                            .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.canvasBG))
                            .codaNoAutocapAndSpelling()

                        Button {
                            let code = form.approveUserCodeInput
                            form.approveUserCodeInput = ""
                            Task { await store.approveHostDeviceCode(code) }
                        } label: {
                            Text(tr("Approve", "批准入网", locale: store.locale))
                                .font(.system(size: 12, weight: .semibold))
                                .foregroundStyle(.black)
                                .padding(.horizontal, 12)
                                .padding(.vertical, 8)
                                .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.cyanWorking))
                        }
                        .buttonStyle(.plain)
                        .disabled(form.approveUserCodeInput.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    }
                }

                if !store.remoteMachines.isEmpty {
                    Divider().background(CodaHerdrTheme.borderHairline)
                    ForEach(store.remoteMachines) { dev in
                        let hh = store.hostHealthByDevice[dev.id]
                        let canOTA = dev.isOnline && (hh?.otaSupported ?? true)
                        VStack(alignment: .leading, spacing: 6) {
                            HStack(spacing: 8) {
                                Circle()
                                    .fill(dev.isOnline ? CodaHerdrTheme.greenDiff : CodaHerdrTheme.textDim)
                                    .frame(width: 8, height: 8)
                                VStack(alignment: .leading, spacing: 2) {
                                    HStack(spacing: 6) {
                                        Text(dev.name)
                                            .font(.system(size: 13, weight: .medium))
                                            .foregroundStyle(CodaHerdrTheme.textPrimary)
                                        Text(dev.platform)
                                            .font(.system(size: 11, design: .monospaced))
                                            .foregroundStyle(CodaHerdrTheme.textSecondary)
                                    }
                                    if let hh {
                                        Text(hh.compactSummary)
                                            .font(.system(size: 10.5, design: .monospaced))
                                            .foregroundStyle(CodaHerdrTheme.textSecondary)
                                    }
                                }
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

                                let isUpgradingOTA = store.otaUpgradingDeviceIDs.contains(dev.id)
                                if canOTA || isUpgradingOTA {
                                    Button {
                                        Task { await store.upgradeRemoteMachine(dev) }
                                    } label: {
                                        HStack(spacing: 4) {
                                            if isUpgradingOTA {
                                                ProgressView()
                                                    .controlSize(.mini)
                                                    .tint(CodaHerdrTheme.cyanWorking)
                                            }
                                            Text(
                                                isUpgradingOTA
                                                    ? tr("Upgrading…", "升级中…", locale: store.locale)
                                                    : tr("Upgrade", "升级", locale: store.locale)
                                            )
                                            .font(.system(size: 10.5, weight: .semibold))
                                        }
                                        .foregroundStyle(CodaHerdrTheme.cyanWorking)
                                        .padding(.horizontal, 7)
                                        .padding(.vertical, 4)
                                        .background(
                                            RoundedRectangle(cornerRadius: 6)
                                                .fill(CodaHerdrTheme.cyanWorking.opacity(isUpgradingOTA ? 0.08 : 0.14))
                                        )
                                        .opacity(isUpgradingOTA ? 0.72 : 1.0)
                                    }
                                    .buttonStyle(.plain)
                                    .disabled(isUpgradingOTA)
                                }

                                Button {
                                    if form.renamingDeviceId == dev.id {
                                        form.renamingDeviceId = nil
                                    } else {
                                        form.renamingDeviceId = dev.id
                                        form.renameDeviceInput = dev.name
                                    }
                                } label: {
                                    Image(systemName: "pencil")
                                        .font(.system(size: 11))
                                        .foregroundStyle(CodaHerdrTheme.textSecondary)
                                        .padding(5)
                                        .background(RoundedRectangle(cornerRadius: 6).fill(CodaHerdrTheme.canvasBG))
                                }
                                .buttonStyle(.plain)

                                Button(role: .destructive) {
                                    Task { await store.deleteRemoteMachine(dev) }
                                } label: {
                                    Image(systemName: "trash")
                                        .font(.system(size: 11))
                                        .foregroundStyle(CodaHerdrTheme.redDiff)
                                        .padding(5)
                                        .background(RoundedRectangle(cornerRadius: 6).fill(CodaHerdrTheme.redDiff.opacity(0.12)))
                                }
                                .buttonStyle(.plain)
                            }

                            if form.renamingDeviceId == dev.id {
                                HStack(spacing: 6) {
                                    TextField(tr("New host name", "新主机名称", locale: store.locale), text: $form.renameDeviceInput)
                                        .font(.system(size: 12))
                                        .padding(.horizontal, 8)
                                        .padding(.vertical, 6)
                                        .background(RoundedRectangle(cornerRadius: 7).fill(CodaHerdrTheme.canvasBG))
                                    Button(tr("Save", "保存", locale: store.locale)) {
                                        let nextName = form.renameDeviceInput
                                        form.renamingDeviceId = nil
                                        Task { await store.renameRemoteMachine(dev, to: nextName) }
                                    }
                                    .font(.system(size: 11.5, weight: .semibold))
                                    .foregroundStyle(.black)
                                    .padding(.horizontal, 10)
                                    .padding(.vertical, 6)
                                    .background(RoundedRectangle(cornerRadius: 7).fill(CodaHerdrTheme.textPrimary))
                                    .buttonStyle(.plain)
                                }
                            }
                        }
                    }
                }

                Divider().background(CodaHerdrTheme.borderHairline)

                HStack(spacing: 8) {
                    if let onlineHost = store.remoteMachines.first(where: { $0.isOnline }) {
                        Button {
                            dismiss()
                            Task { @MainActor in
                                try? await Task.sleep(nanoseconds: 250_000_000)
                                store.openBootstrapWorkspaceSheet(for: onlineHost)
                            }
                        } label: {
                            Label(
                                tr("Bootstrap Workspace", "新建/克隆工作区", locale: store.locale),
                                systemImage: "folder.badge.plus"
                            )
                            .font(.system(size: 11.5, weight: .semibold))
                            .foregroundStyle(CodaHerdrTheme.cyanWorking)
                            .padding(.horizontal, 10)
                            .padding(.vertical, 7)
                            .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.cyanWorking.opacity(0.14)))
                        }
                        .buttonStyle(.plain)
                    }

                    Spacer()

                    Button {
                        Task { await store.pruneStaleWebDevices() }
                    } label: {
                        Label(
                            store.isPruningStaleDevices
                                ? tr("Pruning…", "正在清理…", locale: store.locale)
                                : tr("Prune Offline Web Devices", "清理离线网页端设备", locale: store.locale),
                            systemImage: "sparkles"
                        )
                        .font(.system(size: 11.5, weight: .semibold))
                        .foregroundStyle(CodaHerdrTheme.textPrimary)
                        .padding(.horizontal, 10)
                        .padding(.vertical, 7)
                        .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.pillActiveBG))
                    }
                    .buttonStyle(.plain)
                    .disabled(store.isPruningStaleDevices)
                }
            }
        }
    }

    // MARK: - Admin Friend Circle · Invite Links & Passwordless Member Reset

    private var adminFriendsAndInvitesSection: some View {
        sectionCard(title: tr("Admin · Friends & Invites", "管理员 · 好友与邀请链接", locale: store.locale)) {
            VStack(alignment: .leading, spacing: 12) {
                HStack {
                    Text(tr("Friend Invite Codes", "好友注册邀请码", locale: store.locale))
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(CodaHerdrTheme.textPrimary)
                    Spacer()
                    Button {
                        Task { await store.loadAdminDashboardData() }
                    } label: {
                        HStack(spacing: 4) {
                            if store.isLoadingAdminData {
                                ProgressView().controlSize(.mini).tint(CodaHerdrTheme.textSecondary)
                            } else {
                                Image(systemName: "arrow.clockwise")
                                    .font(.system(size: 11, weight: .semibold))
                            }
                            Text(tr("Refresh", "刷新", locale: store.locale))
                                .font(.system(size: 11, weight: .medium))
                        }
                        .foregroundStyle(CodaHerdrTheme.textSecondary)
                        .padding(.horizontal, 8)
                        .padding(.vertical, 4)
                        .background(RoundedRectangle(cornerRadius: 6).fill(CodaHerdrTheme.canvasBG))
                    }
                    .buttonStyle(.plain)
                }

                HStack(spacing: 8) {
                    TextField(
                        tr("Optional note (e.g. for WeChat friend)", "备注（如：给好友小王）", locale: store.locale),
                        text: $form.newInviteNoteInput
                    )
                    .font(.system(size: 12.5))
                    .padding(.horizontal, 10)
                    .padding(.vertical, 7)
                    .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.canvasBG))

                    Button {
                        let note = form.newInviteNoteInput
                        form.newInviteNoteInput = ""
                        Task {
                            if let created = await store.createAdminInvite(note: note) {
                                form.copiedInviteCode = created.code
                            }
                        }
                    } label: {
                        Label(tr("New Invite", "新建邀请码", locale: store.locale), systemImage: "plus")
                            .font(.system(size: 11.5, weight: .semibold))
                            .foregroundStyle(.black)
                            .padding(.horizontal, 11)
                            .padding(.vertical, 7.5)
                            .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.textPrimary))
                    }
                    .buttonStyle(.plain)
                }

                if store.adminInvites.isEmpty {
                    Text(tr("No invite codes yet. Create one to share a registration link.", "暂无邀请码，点击上方按钮即可生成专属注册链接。", locale: store.locale))
                        .font(.system(size: 11.5))
                        .foregroundStyle(CodaHerdrTheme.textSecondary)
                } else {
                    ForEach(store.adminInvites) { invite in
                        let isCopied = (form.copiedInviteCode == invite.code)
                        HStack(spacing: 8) {
                            VStack(alignment: .leading, spacing: 2) {
                                HStack(spacing: 6) {
                                    Text(invite.code)
                                        .font(.system(size: 12.5, weight: .semibold, design: .monospaced))
                                        .foregroundStyle(CodaHerdrTheme.textPrimary)
                                    Text("\(invite.usedCount)/\(invite.maxUses)")
                                        .font(.system(size: 10.5, design: .monospaced))
                                        .foregroundStyle(CodaHerdrTheme.textSecondary)
                                }
                                if !invite.note.isEmpty {
                                    Text(invite.note)
                                        .font(.system(size: 11))
                                        .foregroundStyle(CodaHerdrTheme.textSecondary)
                                        .lineLimit(1)
                                }
                            }
                            Spacer()

                            Button {
                                if let url = buildInviteRegistrationURL(
                                    serverURL: store.settings.serverBaseURLString,
                                    code: invite.code
                                ) {
                                    copyTextToClipboard(url)
                                    form.copiedInviteCode = invite.code
                                    CodaHaptics.success()
                                    store.adminStatusMessage = tr(
                                        "Copied invite link: \(url)",
                                        "已复制注册邀请链接：\(url)",
                                        locale: store.locale
                                    )
                                }
                            } label: {
                                Label(
                                    isCopied
                                        ? tr("Copied Link", "已复制链接", locale: store.locale)
                                        : tr("Copy Invite Link", "复制邀请链接", locale: store.locale),
                                    systemImage: isCopied ? "checkmark" : "link"
                                )
                                .font(.system(size: 11, weight: .semibold))
                                .foregroundStyle(isCopied ? CodaHerdrTheme.greenDiff : CodaHerdrTheme.cyanWorking)
                                .padding(.horizontal, 9)
                                .padding(.vertical, 5.5)
                                .background(
                                    RoundedRectangle(cornerRadius: 7)
                                        .fill((isCopied ? CodaHerdrTheme.greenDiff : CodaHerdrTheme.cyanWorking).opacity(0.14))
                                )
                            }
                            .buttonStyle(.plain)

                            Button(role: .destructive) {
                                Task { await store.deleteAdminInvite(code: invite.code) }
                            } label: {
                                Image(systemName: "trash")
                                    .font(.system(size: 11))
                                    .foregroundStyle(CodaHerdrTheme.redDiff)
                                    .padding(5.5)
                                    .background(RoundedRectangle(cornerRadius: 6).fill(CodaHerdrTheme.redDiff.opacity(0.12)))
                            }
                            .buttonStyle(.plain)
                        }
                        .padding(.horizontal, 10)
                        .padding(.vertical, 7)
                        .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.canvasBG))
                    }
                }

                Divider().background(CodaHerdrTheme.borderHairline)

                HStack {
                    Text(
                        tr(
                            "Friends / Members (\(store.adminUsers.count))",
                            "好友 / 成员列表 (\(store.adminUsers.count))",
                            locale: store.locale
                        )
                    )
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(CodaHerdrTheme.textPrimary)
                    Spacer()
                }

                if store.adminUsers.isEmpty {
                    Text(tr("No registered members loaded yet.", "暂无已注册成员数据。", locale: store.locale))
                        .font(.system(size: 11.5))
                        .foregroundStyle(CodaHerdrTheme.textSecondary)
                } else {
                    ForEach(store.adminUsers) { member in
                        let isCopiedReset = (form.copiedResetUserId == member.id)
                        let isMinting = (form.mintingResetUserId == member.id)
                        HStack(spacing: 8) {
                            Circle()
                                .fill(member.onlineDevices > 0 ? CodaHerdrTheme.greenDiff : CodaHerdrTheme.textDim)
                                .frame(width: 8, height: 8)

                            VStack(alignment: .leading, spacing: 2) {
                                HStack(spacing: 6) {
                                    Text("@\(member.handle)")
                                        .font(.system(size: 13, weight: .semibold, design: .monospaced))
                                        .foregroundStyle(CodaHerdrTheme.textPrimary)
                                    if member.isAdmin {
                                        Text("ADMIN")
                                            .font(.system(size: 9, weight: .bold, design: .monospaced))
                                            .foregroundStyle(CodaHerdrTheme.amberNeedsYou)
                                            .padding(.horizontal, 5)
                                            .padding(.vertical, 1.5)
                                            .background(Capsule().fill(CodaHerdrTheme.amberNeedsYou.opacity(0.16)))
                                    }
                                }
                                Text(
                                    tr(
                                        "\(member.onlineDevices)/\(member.totalDevices) devices online",
                                        "\(member.onlineDevices)/\(member.totalDevices) 台设备在线",
                                        locale: store.locale
                                    )
                                )
                                .font(.system(size: 11, design: .monospaced))
                                .foregroundStyle(CodaHerdrTheme.textSecondary)
                            }

                            Spacer()

                            Button {
                                form.mintingResetUserId = member.id
                                Task {
                                    let res = await store.generateAdminPasswordResetLink(for: member)
                                    form.mintingResetUserId = nil
                                    if res != nil {
                                        form.copiedResetUserId = member.id
                                    }
                                }
                            } label: {
                                HStack(spacing: 4) {
                                    if isMinting {
                                        ProgressView().controlSize(.mini).tint(CodaHerdrTheme.amberNeedsYou)
                                    } else {
                                        Image(systemName: isCopiedReset ? "checkmark" : "key.fill")
                                            .font(.system(size: 10.5, weight: .semibold))
                                    }
                                    Text(
                                        isCopiedReset
                                            ? tr("Copied Reset Link", "已复制重置链接", locale: store.locale)
                                            : tr("Copy Reset Link", "复制重置链接", locale: store.locale)
                                    )
                                    .font(.system(size: 11, weight: .semibold))
                                }
                                .foregroundStyle(isCopiedReset ? CodaHerdrTheme.greenDiff : CodaHerdrTheme.amberNeedsYou)
                                .padding(.horizontal, 9)
                                .padding(.vertical, 5.5)
                                .background(
                                    RoundedRectangle(cornerRadius: 7)
                                        .fill((isCopiedReset ? CodaHerdrTheme.greenDiff : CodaHerdrTheme.amberNeedsYou).opacity(0.14))
                                )
                            }
                            .buttonStyle(.plain)
                            .disabled(isMinting)
                        }
                        .padding(.horizontal, 10)
                        .padding(.vertical, 7)
                        .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.canvasBG))
                    }
                }

                if let okMsg = store.adminStatusMessage {
                    Text(okMsg)
                        .font(.system(size: 11.5))
                        .foregroundStyle(CodaHerdrTheme.greenDiff)
                }
                if let errMsg = store.adminErrorMessage {
                    Text(errMsg)
                        .font(.system(size: 11.5))
                        .foregroundStyle(CodaHerdrTheme.redDiff)
                }
            }
        }
    }

    // MARK: - Zero-Knowledge Offline Alert Webhooks

    private var offlineWebhooksSection: some View {
        sectionCard(title: tr("Offline Alert Webhooks", "离线告警推送 Webhooks", locale: store.locale)) {
            VStack(alignment: .leading, spacing: 10) {
                Text(tr(
                    "Zero-knowledge push when remote agents enter Needs-You or Finish (metadata only — never terminal plaintext).",
                    "零知识后台推送：当远端 Agent 进入待审批或完成任务时发送通知（仅含元数据，绝不包含终端明文）。",
                    locale: store.locale
                ))
                .font(.system(size: 11))
                .foregroundStyle(CodaHerdrTheme.textSecondary)

                Toggle(isOn: $store.notificationSettings.notifyOnNeedsYou) {
                    Text(tr("Notify on Needs-You (Approval)", "待审批 (Needs-You) 时推送", locale: store.locale))
                        .font(.system(size: 12.5))
                        .foregroundStyle(CodaHerdrTheme.textPrimary)
                }
                .tint(CodaHerdrTheme.amberNeedsYou)

                Toggle(isOn: $store.notificationSettings.notifyOnFinished) {
                    Text(tr("Notify on Task Finished", "任务完成 (Finished) 时推送", locale: store.locale))
                        .font(.system(size: 12.5))
                        .foregroundStyle(CodaHerdrTheme.textPrimary)
                }
                .tint(CodaHerdrTheme.greenDiff)

                labeledInput(
                    tr("Bark URL (e.g. https://api.day.app/<key>)", "Bark 推送地址", locale: store.locale),
                    text: $store.notificationSettings.barkURL
                )

                VStack(alignment: .leading, spacing: 4) {
                    Text(tr("Telegram Bot Token", "Telegram Bot Token", locale: store.locale))
                        .font(.system(size: 11))
                        .foregroundStyle(CodaHerdrTheme.textSecondary)
                    SecureField("123456789:AA...", text: $store.notificationSettings.telegramBotToken)
                        .font(.system(size: 12, design: .monospaced))
                        .padding(.horizontal, 10)
                        .padding(.vertical, 8)
                        .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.canvasBG))
                }

                labeledInput(
                    tr("Telegram Chat ID", "Telegram Chat ID", locale: store.locale),
                    text: $store.notificationSettings.telegramChatId
                )

                labeledInput(
                    tr("Feishu Webhook URL", "飞书机器人 Webhook URL", locale: store.locale),
                    text: $store.notificationSettings.feishuWebhookURL
                )

                labeledInput(
                    tr("Discord Webhook URL", "Discord Webhook URL", locale: store.locale),
                    text: $store.notificationSettings.discordWebhookURL
                )

                labeledInput(
                    tr("Generic JSON Webhook URL (optional)", "通用 JSON Webhook URL（可选）", locale: store.locale),
                    text: $store.notificationSettings.genericWebhookURL
                )

                HStack(spacing: 10) {
                    Button {
                        Task { await store.saveNotificationWebhookSettings() }
                    } label: {
                        Label(
                            store.isSavingNotificationSettings
                                ? tr("Saving…", "保存中…", locale: store.locale)
                                : tr("Save Webhooks", "保存推送配置", locale: store.locale),
                            systemImage: "checkmark.circle.fill"
                        )
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(.black)
                        .padding(.horizontal, 12)
                        .padding(.vertical, 8)
                        .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.textPrimary))
                    }
                    .buttonStyle(.plain)
                    .disabled(store.isSavingNotificationSettings || store.isTestingNotificationSettings)

                    Button {
                        Task { await store.sendTestNotificationWebhook() }
                    } label: {
                        Label(
                            store.isTestingNotificationSettings
                                ? tr("Sending…", "推送中…", locale: store.locale)
                                : tr("Test Push", "发送测试推送", locale: store.locale),
                            systemImage: "paperplane.fill"
                        )
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(.black)
                        .padding(.horizontal, 12)
                        .padding(.vertical, 8)
                        .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.cyanWorking))
                    }
                    .buttonStyle(.plain)
                    .disabled(store.isSavingNotificationSettings || store.isTestingNotificationSettings)
                }

                if let okMsg = store.notificationStatusMessage {
                    Text(okMsg)
                        .font(.system(size: 11.5))
                        .foregroundStyle(CodaHerdrTheme.greenDiff)
                }
                if let errMsg = store.notificationErrorMessage {
                    Text(errMsg)
                        .font(.system(size: 11.5))
                        .foregroundStyle(CodaHerdrTheme.redDiff)
                }
            }
        }
    }

    // MARK: - Voice Model Configuration

    private var voiceModelSection: some View {
        let hostHasWhisper = store.remoteMachines.contains { dev in
            dev.isOnline && (store.daemonVoiceConfigsByDevice[dev.id]?.apiKey.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty == false)
        }
        return sectionCard(title: tr("Voice Dictation Model", "语音识别模型", locale: store.locale)) {
            VStack(alignment: .leading, spacing: 10) {
                if CodaVoiceRecorder.usesNativeSpeechFallback(for: store.settings.voiceConfig) {
                    HStack(spacing: 6) {
                        Image(systemName: "waveform.badge.mic")
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundStyle(CodaHerdrTheme.greenDiff)
                        Text(
                            hostHasWhisper
                                ? tr(
                                    "Zero-config Apple Native Speech active (with E2EE remote host Whisper fallback).",
                                    "零配置 Apple 原生语音已激活（同时支持远端主机 Whisper E2EE 自动回退）。",
                                    locale: store.locale
                                )
                                : tr(
                                    "Zero-config Apple Native Speech (SFSpeechRecognizer) fallback active while API Key is empty.",
                                    "零配置模式已激活：未填 API Key 时自动使用 Apple 原生语音识别 (SFSpeechRecognizer)。",
                                    locale: store.locale
                                )
                        )
                        .font(.system(size: 11))
                        .foregroundStyle(CodaHerdrTheme.greenDiff)
                    }
                }
                labeledInput(tr("Provider", "服务商", locale: store.locale), text: $store.settings.voiceConfig.provider)
                labeledInput(tr("Base URL", "接口地址", locale: store.locale), text: $store.settings.voiceConfig.baseURL)
                labeledInput(tr("Model", "模型名称", locale: store.locale), text: $store.settings.voiceConfig.model)
                VStack(alignment: .leading, spacing: 4) {
                    Text(tr("API Key (leave empty for Apple Native Speech)", "API Key（留空则使用 Apple 原生语音识别）", locale: store.locale))
                        .font(.system(size: 11))
                        .foregroundStyle(CodaHerdrTheme.textSecondary)
                    SecureField(tr("Optional Whisper API Key", "可选 Whisper API Key", locale: store.locale), text: $store.settings.voiceConfig.apiKey)
                        .font(.system(size: 12, design: .monospaced))
                        .padding(.horizontal, 10)
                        .padding(.vertical, 8)
                        .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.canvasBG))
                }
                HStack {
                    Button {
                        Task { await store.saveVoiceConfig() }
                    } label: {
                        Label(
                            tr("Save & Sync Voice Config", "保存并同步语音配置", locale: store.locale),
                            systemImage: "icloud.and.arrow.up"
                        )
                        .font(.system(size: 12, weight: .medium))
                    }
                    .buttonStyle(.borderedProminent)
                    .controlSize(.small)

                    if let status = store.voiceConfigSyncMessage {
                        Text(status)
                            .font(.system(size: 11))
                            .foregroundStyle(CodaHerdrTheme.textSecondary)
                            .lineLimit(1)
                    }
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

// MARK: - Remote Workspace Bootstrapper Sheet (`git clone` / New Folder + Launch Session)

private final class BootstrapWorkspaceFormModel: ObservableObject {
    @Published var selectedDeviceId: UUID? = nil
    @Published var parentDir: String = "~/workspace"
    @Published var gitCloneURL: String = ""
    @Published var folderName: String = ""
    @Published var selectedAgent: String = "claude"
    @Published var initialPrompt: String = ""
    @Published var didAutoSuggestFolder: Bool = false
}

public struct CodaBootstrapWorkspaceSheet: View {
    @ObservedObject public var store: CodaWorkspaceStore
    @Environment(\.dismiss) private var dismiss
    @StateObject private var form = BootstrapWorkspaceFormModel()

    public init(store: CodaWorkspaceStore) {
        self.store = store
    }

    private var onlineHosts: [CodaRemoteDevice] {
        store.remoteMachines.filter(\.isOnline)
    }

    private var activeDevice: CodaRemoteDevice? {
        if let id = form.selectedDeviceId, let match = onlineHosts.first(where: { $0.id == id }) {
            return match
        }
        return onlineHosts.first
    }

    public var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    if onlineHosts.isEmpty {
                        Text(tr("No online hosts paired. Connect a workstation first.", "当前没有在线主机，请先启动或配对工作站。", locale: store.locale))
                            .font(.system(size: 13))
                            .foregroundStyle(CodaHerdrTheme.amberNeedsYou)
                    } else {
                        VStack(alignment: .leading, spacing: 6) {
                            Text(tr("Target Host", "目标主机", locale: store.locale))
                                .font(.system(size: 11.5, weight: .medium))
                                .foregroundStyle(CodaHerdrTheme.textSecondary)

                            Picker("", selection: Binding(
                                get: { activeDevice?.id ?? onlineHosts[0].id },
                                set: { form.selectedDeviceId = $0 }
                            )) {
                                ForEach(onlineHosts) { dev in
                                    Text("\(dev.name) (\(dev.platform))").tag(dev.id)
                                }
                            }
                            .pickerStyle(.menu)
                        }

                        fieldBlock(
                            title: tr("Parent Directory on Host", "远端父目录", locale: store.locale),
                            placeholder: "~/workspace",
                            text: $form.parentDir
                        )

                        fieldBlock(
                            title: tr("Git Clone URL (optional — https://, ssh://, or git@)", "Git 仓库地址（可选，留空则仅创建空目录）", locale: store.locale),
                            placeholder: "https://github.com/org/repo.git",
                            text: $form.gitCloneURL
                        )
                        .onChange(of: form.gitCloneURL) { _, next in
                            if form.folderName.isEmpty || form.didAutoSuggestFolder {
                                if let suggested = CodaWorkspaceBootstrapValidator.suggestedFolderName(fromGitCloneURL: next) {
                                    form.folderName = suggested
                                    form.didAutoSuggestFolder = true
                                }
                            }
                        }

                        fieldBlock(
                            title: tr("New Folder Name", "新工作区目录名", locale: store.locale),
                            placeholder: "my-project",
                            text: $form.folderName
                        )
                        .onChange(of: form.folderName) { _, next in
                            if next.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                                form.didAutoSuggestFolder = true
                            } else if let suggested = CodaWorkspaceBootstrapValidator.suggestedFolderName(fromGitCloneURL: form.gitCloneURL),
                                      next != suggested {
                                form.didAutoSuggestFolder = false
                            }
                        }

                        VStack(alignment: .leading, spacing: 6) {
                            Text(tr("Launch Agent Session", "创建后立即启动会话", locale: store.locale))
                                .font(.system(size: 11.5, weight: .medium))
                                .foregroundStyle(CodaHerdrTheme.textSecondary)

                            Picker("", selection: $form.selectedAgent) {
                                Text("Claude Code").tag("claude")
                                Text("Codex").tag("codex")
                                Text("Antigravity (AGY)").tag("agy")
                                Text("Gemini CLI").tag("gemini")
                                Text("Grok").tag("grok")
                                Text("Shell").tag("shell")
                                Text(tr("None (Create Folder Only)", "不启动（仅初始化目录）", locale: store.locale)).tag("none")
                            }
                            .pickerStyle(.menu)
                        }

                        if form.selectedAgent != "none" {
                            fieldBlock(
                                title: tr("Initial Prompt (optional)", "初始指令（可选）", locale: store.locale),
                                placeholder: tr("e.g. Inspect repo structure and run tests", "例如：阅读项目结构并运行测试", locale: store.locale),
                                text: $form.initialPrompt
                            )
                        }

                        if let err = store.bootstrapErrorMessage {
                            Text(err)
                                .font(.system(size: 12))
                                .foregroundStyle(CodaHerdrTheme.redDiff)
                        }

                        Button {
                            guard let dev = activeDevice else { return }
                            Task {
                                let ok = await store.bootstrapWorkspaceAndLaunch(
                                    on: dev,
                                    parentDir: form.parentDir,
                                    folderName: form.folderName,
                                    gitCloneUrl: form.gitCloneURL,
                                    agent: form.selectedAgent,
                                    initialPrompt: form.initialPrompt
                                )
                                if ok {
                                    dismiss()
                                }
                            }
                        } label: {
                            HStack(spacing: 8) {
                                if store.isBootstrappingWorkspace {
                                    ProgressView().controlSize(.small).tint(.black)
                                } else {
                                    Image(systemName: form.gitCloneURL.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? "folder.badge.plus" : "arrow.down.doc.fill")
                                }
                                Text(
                                    store.isBootstrappingWorkspace
                                        ? tr("Bootstrapping over E2EE…", "正在远端创建/克隆工作区…", locale: store.locale)
                                        : (form.gitCloneURL.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                                            ? tr("Create Workspace & Launch", "创建目录并启动", locale: store.locale)
                                            : tr("Git Clone & Launch", "Git Clone 并启动", locale: store.locale))
                                )
                                .font(.system(size: 13, weight: .semibold))
                            }
                            .foregroundStyle(.black)
                            .frame(maxWidth: .infinity)
                            .padding(.vertical, 11)
                            .background(RoundedRectangle(cornerRadius: 10).fill(CodaHerdrTheme.cyanWorking))
                        }
                        .buttonStyle(.plain)
                        .disabled(
                            store.isBootstrappingWorkspace
                                || form.folderName.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                        )
                    }
                }
                .padding(18)
            }
            .background(CodaHerdrTheme.canvasBG)
            .navigationTitle(tr("Bootstrap Workspace", "新建 / 克隆远端工作区", locale: store.locale))
            #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
            #endif
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(tr("Cancel", "取消", locale: store.locale)) {
                        dismiss()
                    }
                }
            }
            .onAppear {
                form.selectedDeviceId = store.bootstrapTargetDeviceID ?? onlineHosts.first?.id
                form.parentDir = store.bootstrapDefaultParentDir
            }
        }
        .preferredColorScheme(.dark)
    }

    private func fieldBlock(title: String, placeholder: String, text: Binding<String>) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title)
                .font(.system(size: 11.5, weight: .medium))
                .foregroundStyle(CodaHerdrTheme.textSecondary)
            TextField(placeholder, text: text)
                .font(.system(size: 12.5, design: .monospaced))
                .foregroundStyle(CodaHerdrTheme.textPrimary)
                .padding(.horizontal, 10)
                .padding(.vertical, 8)
                .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.cardBG))
                .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
                .codaNoAutocapAndSpelling()
        }
    }
}
