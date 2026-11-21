import CodaKit
import Combine
import SwiftUI
#if canImport(WebKit)
import WebKit
#endif

public enum CodaInspectorSheetMode: String, CaseIterable, Identifiable, Sendable {
    case files
    case changes
    case portPreview

    public var id: String { rawValue }
}

private final class RemoteFilesLocalState: ObservableObject {
    @Published var portText: String = "3000"
    @Published var pathText: String = "/"
    @Published var showRenderedHTML: Bool = true
}

public struct CodaRemoteFilesSheet: View {
    @ObservedObject public var store: CodaWorkspaceStore
    public let session: CodaAgentSession

    @StateObject private var local = RemoteFilesLocalState()

    public init(store: CodaWorkspaceStore, session: CodaAgentSession) {
        self.store = store
        self.session = session
    }

    private var sheetTitle: String {
        switch store.inspectorSheetMode {
        case .files:
            return tr("Workspace Files (E2EE)", "工作区文件 (E2EE)", locale: store.locale)
        case .changes:
            return tr("Git Changes (E2EE)", "代码变更审查 (E2EE)", locale: store.locale)
        case .portPreview:
            return tr("Localhost Port Preview", "本地端口加密预览", locale: store.locale)
        }
    }

    public var body: some View {
        NavigationStack {
            VStack(spacing: 10) {
                Picker("", selection: $store.inspectorSheetMode) {
                    Text(tr("Files", "远端文件", locale: store.locale)).tag(CodaInspectorSheetMode.files)
                    Text(tr("Changes", "代码变更", locale: store.locale)).tag(CodaInspectorSheetMode.changes)
                    Text(tr("Port Preview", "端口预览", locale: store.locale)).tag(CodaInspectorSheetMode.portPreview)
                }
                .pickerStyle(.segmented)
                .padding(.horizontal, 14)
                .padding(.top, 8)
                .onChange(of: store.inspectorSheetMode) { _, mode in
                    if mode == .changes && (store.inspectorSessionId != session.id || store.gitDiffResult == nil) && !store.gitDiffLoading {
                        store.loadGitDiff(for: session, presentModal: false)
                    } else if mode == .files && store.remoteFileSelectedRel == nil && store.remoteFileEntries.isEmpty && !store.remoteFileLoading {
                        store.browseRemoteSubPath(store.remoteFileSubPath, for: session)
                    }
                }

                switch store.inspectorSheetMode {
                case .files:
                    filesBody
                case .changes:
                    changesBody
                case .portPreview:
                    portPreviewBody
                }
            }
            .background(CodaHerdrTheme.canvasBG)
            .navigationTitle(sheetTitle)
            #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
            #endif
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(tr("Done", "完成", locale: store.locale)) {
                        store.showRemoteFilesSheet = false
                    }
                }
            }
        }
    }

    // MARK: - Files Explorer Tab

    @ViewBuilder
    private var filesBody: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                if !store.remoteFileSubPath.isEmpty && store.remoteFileSelectedRel == nil {
                    Button {
                        let parent = CodaTerminalTranscriptHelper.parentSubPath(store.remoteFileSubPath)
                        store.browseRemoteSubPath(parent, for: session)
                    } label: {
                        Label(tr("Up", "上级", locale: store.locale), systemImage: "arrow.up.left")
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundStyle(CodaHerdrTheme.cyanWorking)
                            .padding(.horizontal, 10)
                            .padding(.vertical, 5)
                            .background(Capsule().fill(CodaHerdrTheme.elevatedBG))
                    }
                    .buttonStyle(.plain)
                }

                if store.remoteFileSelectedRel != nil {
                    Button {
                        store.remoteFileSelectedRel = nil
                        store.remoteFilePreviewText = ""
                        store.remoteFileTruncated = false
                        store.remoteFileError = nil
                        if store.remoteFileEntries.isEmpty && !store.remoteFileLoading {
                            store.browseRemoteSubPath(store.remoteFileSubPath, for: session)
                        }
                    } label: {
                        Label(tr("List", "返回列表", locale: store.locale), systemImage: "chevron.left")
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundStyle(CodaHerdrTheme.cyanWorking)
                            .padding(.horizontal, 10)
                            .padding(.vertical, 5)
                            .background(Capsule().fill(CodaHerdrTheme.elevatedBG))
                    }
                    .buttonStyle(.plain)
                }

                Text(
                    store.remoteFileSubPath.isEmpty
                        ? (store.remoteFileRootPath.isEmpty ? session.cwd : store.remoteFileRootPath)
                        : "\(store.remoteFileRootPath)/\(store.remoteFileSubPath)"
                )
                .font(.system(size: 11, design: .monospaced))
                .foregroundStyle(CodaHerdrTheme.textSecondary)
                .lineLimit(1)
                .truncationMode(.head)

                Spacer()

                Button {
                    if let selectedRel = store.remoteFileSelectedRel {
                        store.previewRemoteFile(selectedRel, for: session)
                    } else {
                        store.browseRemoteSubPath(store.remoteFileSubPath, for: session)
                    }
                } label: {
                    Image(systemName: "arrow.clockwise")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(CodaHerdrTheme.textPrimary)
                        .padding(6)
                        .background(Circle().fill(CodaHerdrTheme.elevatedBG))
                }
                .buttonStyle(.plain)
            }
            .padding(.horizontal, 14)

            if let err = store.remoteFileError {
                Text(err)
                    .font(.system(size: 12))
                    .foregroundStyle(CodaHerdrTheme.redDiff)
                    .padding(.horizontal, 14)
            }

            if store.remoteFileLoading {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text(tr("Loading over E2EE…", "正在通过端到端加密通道读取…", locale: store.locale))
                        .font(.system(size: 12))
                        .foregroundStyle(CodaHerdrTheme.textSecondary)
                }
                .padding(.horizontal, 14)
            }

            if let selectedRel = store.remoteFileSelectedRel {
                VStack(alignment: .leading, spacing: 6) {
                    HStack(spacing: 8) {
                        Text(selectedRel)
                            .font(.system(size: 11.5, weight: .semibold, design: .monospaced))
                            .foregroundStyle(CodaHerdrTheme.cyanWorking)
                            .lineLimit(1)
                            .truncationMode(.middle)
                        if store.remoteFileTruncated {
                            Text(tr("Truncated (64 KB)", "已截断 (64 KB)", locale: store.locale))
                                .font(.system(size: 10, weight: .semibold, design: .monospaced))
                                .foregroundStyle(CodaHerdrTheme.amberNeedsYou)
                                .padding(.horizontal, 6)
                                .padding(.vertical, 2)
                                .background(Capsule().fill(CodaHerdrTheme.amberNeedsYou.opacity(0.15)))
                        }
                    }
                    .padding(.horizontal, 14)

                    ScrollView([.vertical, .horizontal]) {
                        Text(store.remoteFilePreviewText.isEmpty ? tr("(Empty file)", "（空文件）", locale: store.locale) : store.remoteFilePreviewText)
                            .font(.system(size: 11.5, design: .monospaced))
                            .foregroundStyle(CodaHerdrTheme.textPrimary)
                            .textSelection(.enabled)
                            .padding(12)
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .background(RoundedRectangle(cornerRadius: 10).fill(CodaHerdrTheme.elevatedBG))
                    .padding(.horizontal, 14)
                    .padding(.bottom, 12)
                }
            } else {
                ScrollView {
                    LazyVStack(spacing: 4) {
                        if store.remoteFileEntries.isEmpty && !store.remoteFileLoading && store.remoteFileError == nil {
                            Text(tr("Directory is empty.", "目录为空。", locale: store.locale))
                                .font(.system(size: 12))
                                .foregroundStyle(CodaHerdrTheme.textSecondary)
                                .padding(.top, 20)
                        }
                        ForEach(store.remoteFileEntries) { entry in
                            Button {
                                if entry.isDir {
                                    store.browseRemoteSubPath(entry.relPath, for: session)
                                } else {
                                    store.previewRemoteFile(entry.relPath, for: session)
                                }
                            } label: {
                                HStack(spacing: 10) {
                                    Image(systemName: entry.isDir ? "folder.fill" : "doc.text")
                                        .font(.system(size: 13))
                                        .foregroundStyle(entry.isDir ? CodaHerdrTheme.cyanWorking : CodaHerdrTheme.textSecondary)
                                    Text(entry.isDir ? "\(entry.name)/" : entry.name)
                                        .font(.system(size: 13, design: .monospaced))
                                        .foregroundStyle(CodaHerdrTheme.textPrimary)
                                        .lineLimit(1)
                                    Spacer()
                                    if !entry.isDir {
                                        Text(entry.formattedSize)
                                            .font(.system(size: 11, design: .monospaced))
                                            .foregroundStyle(CodaHerdrTheme.textDim)
                                    }
                                }
                                .padding(.horizontal, 12)
                                .padding(.vertical, 9)
                                .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.elevatedBG))
                            }
                            .buttonStyle(.plain)
                        }
                    }
                    .padding(.horizontal, 14)
                    .padding(.bottom, 12)
                }
            }
        }
    }

    // MARK: - E2EE Git Diff Changes Tab

    @ViewBuilder
    private var changesBody: some View {
        CodaGitDiffInteractiveView(
            store: store,
            session: session,
            onOpenFilePreview: { relPath in
                let rawRepoRoot = store.gitDiffResult?.repoRoot?.trimmingCharacters(in: .whitespacesAndNewlines)
                let repoRoot = (rawRepoRoot?.isEmpty == false)
                    ? rawRepoRoot
                    : store.matchedRepoRoot(for: session)
                store.inspectorSheetMode = .files
                store.previewRemoteFile(relPath, for: session, baseRootOverride: repoRoot)
            }
        )
        .padding(.horizontal, 14)
        .padding(.bottom, 12)
    }

    // MARK: - Port Preview Tab

    @ViewBuilder
    private var portPreviewBody: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                TextField("3000", text: $local.portText)
                    .font(.system(size: 13, design: .monospaced))
                    .frame(width: 78)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 8)
                    .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.elevatedBG))
                    .codaNoAutocapAndSpelling()

                TextField("/", text: $local.pathText)
                    .font(.system(size: 13, design: .monospaced))
                    .padding(.horizontal, 10)
                    .padding(.vertical, 8)
                    .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.elevatedBG))
                    .codaNoAutocapAndSpelling()

                Button {
                    if let p = Int(local.portText.trimmingCharacters(in: .whitespaces)), (1024...65535).contains(p) {
                        store.loadRemotePortPreview(port: p, path: local.pathText, for: session)
                    } else {
                        store.portPreviewError = tr("Port must be 1024..65535", "端口号需在 1024..65535 之间", locale: store.locale)
                    }
                } label: {
                    Text(store.portPreviewLoading ? "…" : tr("Fetch", "探测", locale: store.locale))
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(.black)
                        .padding(.horizontal, 12)
                        .padding(.vertical, 8)
                        .background(RoundedRectangle(cornerRadius: 8).fill(CodaHerdrTheme.cyanWorking))
                }
                .buttonStyle(.plain)
            }
            .padding(.horizontal, 14)

            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 6) {
                    ForEach(CodaTerminalTranscriptHelper.quickPreviewPorts, id: \.self) { p in
                        Button {
                            local.portText = String(p)
                            store.loadRemotePortPreview(port: p, path: local.pathText, for: session)
                        } label: {
                            Text(":\(p)")
                                .font(.system(size: 11, weight: .medium, design: .monospaced))
                                .foregroundStyle(local.portText == String(p) ? CodaHerdrTheme.cyanWorking : CodaHerdrTheme.textSecondary)
                                .padding(.horizontal, 10)
                                .padding(.vertical, 5)
                                .background(Capsule().fill(CodaHerdrTheme.elevatedBG))
                        }
                        .buttonStyle(.plain)
                    }
                }
                .padding(.horizontal, 14)
            }

            if let err = store.portPreviewError {
                Text(err)
                    .font(.system(size: 12))
                    .foregroundStyle(CodaHerdrTheme.redDiff)
                    .padding(.horizontal, 14)
            }

            if let res = store.portPreviewResult {
                HStack(spacing: 8) {
                    Text("HTTP \(res.statusCode)")
                        .font(.system(size: 12, weight: .bold, design: .monospaced))
                        .foregroundStyle((200..<300).contains(res.statusCode) ? CodaHerdrTheme.greenDiff : CodaHerdrTheme.amberNeedsYou)

                    Text(res.contentType)
                        .font(.system(size: 11, design: .monospaced))
                        .foregroundStyle(CodaHerdrTheme.textSecondary)
                        .lineLimit(1)

                    Spacer()

                    #if canImport(WebKit)
                    if res.isHTML {
                        HStack(spacing: 2) {
                            Button {
                                local.showRenderedHTML = true
                            } label: {
                                Text(tr("Rendered", "网页渲染", locale: store.locale))
                                    .font(.system(size: 10.5, weight: local.showRenderedHTML ? .semibold : .regular))
                                    .foregroundStyle(local.showRenderedHTML ? CodaHerdrTheme.textPrimary : CodaHerdrTheme.textSecondary)
                                    .padding(.horizontal, 8)
                                    .padding(.vertical, 3.5)
                                    .background(Capsule().fill(local.showRenderedHTML ? CodaHerdrTheme.pillActiveBG : .clear))
                            }
                            .buttonStyle(.plain)

                            Button {
                                local.showRenderedHTML = false
                            } label: {
                                Text(tr("Source", "源码", locale: store.locale))
                                    .font(.system(size: 10.5, weight: !local.showRenderedHTML ? .semibold : .regular))
                                    .foregroundStyle(!local.showRenderedHTML ? CodaHerdrTheme.textPrimary : CodaHerdrTheme.textSecondary)
                                    .padding(.horizontal, 8)
                                    .padding(.vertical, 3.5)
                                    .background(Capsule().fill(!local.showRenderedHTML ? CodaHerdrTheme.pillActiveBG : .clear))
                            }
                            .buttonStyle(.plain)
                        }
                        .padding(2)
                        .background(
                            Capsule()
                                .fill(CodaHerdrTheme.elevatedBG)
                                .overlay(Capsule().strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
                        )
                    }
                    #endif
                }
                .padding(.horizontal, 14)

                #if canImport(WebKit)
                if res.isHTML && local.showRenderedHTML {
                    CodaSandboxedHTMLWebView(html: res.rawHTML ?? res.bodyText)
                        .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
                        .overlay(
                            RoundedRectangle(cornerRadius: 10, style: .continuous)
                                .strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1)
                        )
                        .padding(.horizontal, 14)
                        .padding(.bottom, 12)
                } else {
                    rawPortSourceView(res)
                }
                #else
                rawPortSourceView(res)
                #endif
            } else {
                Spacer()
            }
        }
    }

    private func rawPortSourceView(_ res: CodaPortPreviewResult) -> some View {
        ScrollView {
            Text(res.bodyText.isEmpty ? tr("(Empty response body)", "（响应体为空）", locale: store.locale) : res.bodyText)
                .font(.system(size: 11.5, design: .monospaced))
                .foregroundStyle(CodaHerdrTheme.textPrimary)
                .textSelection(.enabled)
                .padding(12)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .background(RoundedRectangle(cornerRadius: 10).fill(CodaHerdrTheme.elevatedBG))
        .padding(.horizontal, 14)
        .padding(.bottom, 12)
    }
}

// MARK: - Sandboxed Non-Persistent `WKWebView` HTML Snapshot Preview (JS Disabled)

#if canImport(WebKit)
@MainActor
private final class SandboxedWebNavigationDelegate: NSObject, WKNavigationDelegate {
    var lastLoadedHTML: String = ""

    func webView(
        _ webView: WKWebView,
        decidePolicyFor navigationAction: WKNavigationAction
    ) async -> WKNavigationActionPolicy {
        if navigationAction.navigationType == .other {
            return .allow
        }
        // Block external link clicks & form submissions inside sandboxed snapshot preview.
        return .cancel
    }
}

#if os(iOS)
private struct CodaSandboxedHTMLWebView: UIViewRepresentable {
    let html: String

    func makeCoordinator() -> SandboxedWebNavigationDelegate {
        SandboxedWebNavigationDelegate()
    }

    func makeUIView(context: Context) -> WKWebView {
        let config = WKWebViewConfiguration()
        config.websiteDataStore = .nonPersistent()
        config.defaultWebpagePreferences.allowsContentJavaScript = false
        let webView = WKWebView(frame: .zero, configuration: config)
        webView.navigationDelegate = context.coordinator
        webView.isOpaque = true
        context.coordinator.lastLoadedHTML = html
        webView.loadHTMLString(html, baseURL: nil)
        return webView
    }

    func updateUIView(_ webView: WKWebView, context: Context) {
        guard context.coordinator.lastLoadedHTML != html else { return }
        context.coordinator.lastLoadedHTML = html
        webView.loadHTMLString(html, baseURL: nil)
    }
}
#elseif os(macOS)
private struct CodaSandboxedHTMLWebView: NSViewRepresentable {
    let html: String

    func makeCoordinator() -> SandboxedWebNavigationDelegate {
        SandboxedWebNavigationDelegate()
    }

    func makeNSView(context: Context) -> WKWebView {
        let config = WKWebViewConfiguration()
        config.websiteDataStore = .nonPersistent()
        config.defaultWebpagePreferences.allowsContentJavaScript = false
        let webView = WKWebView(frame: .zero, configuration: config)
        webView.navigationDelegate = context.coordinator
        context.coordinator.lastLoadedHTML = html
        webView.loadHTMLString(html, baseURL: nil)
        return webView
    }

    func updateNSView(_ webView: WKWebView, context: Context) {
        guard context.coordinator.lastLoadedHTML != html else { return }
        context.coordinator.lastLoadedHTML = html
        webView.loadHTMLString(html, baseURL: nil)
    }
}
#endif
#endif

// MARK: - Shared Interactive Git Diff Inspector Component (Branch Badge + File Filter Chips + Colored Unified Diff)

public struct CodaGitDiffInteractiveView: View {
    @ObservedObject public var store: CodaWorkspaceStore
    public let session: CodaAgentSession
    public var onOpenFilePreview: ((String) -> Void)?

    public init(
        store: CodaWorkspaceStore,
        session: CodaAgentSession,
        onOpenFilePreview: ((String) -> Void)? = nil
    ) {
        self.store = store
        self.session = session
        self.onOpenFilePreview = onOpenFilePreview
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            let diffForCurrentSession = (store.inspectorSessionId == nil || store.inspectorSessionId == session.id)
                ? store.gitDiffResult
                : nil
            let branchLabel = diffForCurrentSession?.branch.isEmpty == false
                ? (diffForCurrentSession?.branch ?? session.branch)
                : session.branch
            let changedFiles = diffForCurrentSession?.changedFiles ?? []
            let selectedChangedItem = changedFiles.first(where: { $0.path == store.gitDiffSelectedFile })

            HStack(spacing: 8) {
                if diffForCurrentSession?.isGitRepo != false {
                    HStack(spacing: 5) {
                        Image(systemName: "arrow.triangle.branch")
                            .font(.system(size: 11, weight: .semibold))
                            .foregroundStyle(CodaHerdrTheme.cyanWorking)
                        Text(branchLabel)
                            .font(.system(size: 11.5, weight: .semibold, design: .monospaced))
                            .foregroundStyle(CodaHerdrTheme.textPrimary)
                            .lineLimit(1)
                    }
                    .padding(.horizontal, 10)
                    .padding(.vertical, 5)
                    .background(
                        Capsule()
                            .fill(CodaHerdrTheme.elevatedBG)
                            .overlay(Capsule().strokeBorder(CodaHerdrTheme.cyanWorking.opacity(0.4), lineWidth: 1))
                    )
                }

                if !changedFiles.isEmpty {
                    Text(tr("\(changedFiles.count) changed", "\(changedFiles.count) 处变更", locale: store.locale))
                        .font(.system(size: 11, weight: .medium, design: .monospaced))
                        .foregroundStyle(CodaHerdrTheme.textSecondary)
                }

                Spacer()

                if let item = selectedChangedItem,
                   !item.isDeleted,
                   !item.isDirectoryEntry,
                   let onOpenFilePreview {
                    Button {
                        onOpenFilePreview(item.path)
                    } label: {
                        Label(tr("Open File", "预览文件", locale: store.locale), systemImage: "doc.text.magnifyingglass")
                            .font(.system(size: 11, weight: .semibold))
                            .foregroundStyle(CodaHerdrTheme.cyanWorking)
                            .padding(.horizontal, 9)
                            .padding(.vertical, 5)
                            .background(Capsule().fill(CodaHerdrTheme.elevatedBG))
                    }
                    .buttonStyle(.plain)
                }

                Button {
                    store.loadGitDiff(for: session, presentModal: false)
                } label: {
                    Label(tr("Refresh", "刷新", locale: store.locale), systemImage: "arrow.clockwise")
                        .font(.system(size: 11.5, weight: .semibold))
                        .foregroundStyle(CodaHerdrTheme.textPrimary)
                        .padding(.horizontal, 10)
                        .padding(.vertical, 5)
                        .background(
                            Capsule()
                                .fill(CodaHerdrTheme.elevatedBG)
                                .overlay(Capsule().strokeBorder(CodaHerdrTheme.borderHairline, lineWidth: 1))
                        )
                }
                .buttonStyle(.plain)
            }

            if let err = store.gitDiffError {
                Text(err)
                    .font(.system(size: 12))
                    .foregroundStyle(CodaHerdrTheme.redDiff)
            }

            if store.gitDiffLoading {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small).tint(CodaHerdrTheme.cyanWorking)
                    Text(tr("Fetching remote Git diff over E2EE…", "正在通过端到端加密获取 Git Diff…", locale: store.locale))
                        .font(.system(size: 12))
                        .foregroundStyle(CodaHerdrTheme.textSecondary)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if let diff = diffForCurrentSession {
                if !diff.isGitRepo {
                    VStack(spacing: 8) {
                        Image(systemName: "folder.badge.questionmark")
                            .font(.system(size: 22))
                            .foregroundStyle(CodaHerdrTheme.textSecondary)
                        Text(tr(
                            "Current workspace directory is not inside a Git repository.",
                            "当前工作目录不在 Git 代码仓库内。",
                            locale: store.locale
                        ))
                        .font(.system(size: 12.5))
                        .foregroundStyle(CodaHerdrTheme.textSecondary)
                        .multilineTextAlignment(.center)
                    }
                    .padding(16)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .background(RoundedRectangle(cornerRadius: 10).fill(CodaHerdrTheme.elevatedBG))
                } else {
                    if !changedFiles.isEmpty {
                        ScrollView(.horizontal, showsIndicators: false) {
                            HStack(spacing: 6) {
                                fileFilterChip(
                                    status: nil,
                                    label: tr("All (\(changedFiles.count))", "全部 (\(changedFiles.count))", locale: store.locale),
                                    isSelected: store.gitDiffSelectedFile == nil
                                ) {
                                    store.gitDiffSelectedFile = nil
                                }

                                ForEach(changedFiles) { item in
                                    let active = store.gitDiffSelectedFile == item.path
                                    let chipLabel: String = {
                                        if let prev = item.previousPath, !prev.isEmpty {
                                            return "\(prev) → \(item.path)"
                                        }
                                        return item.path
                                    }()
                                    fileFilterChip(
                                        status: item.status,
                                        label: chipLabel,
                                        isSelected: active
                                    ) {
                                        store.gitDiffSelectedFile = active ? nil : item.path
                                    }
                                }
                            }
                        }
                    }

                    let filteredDiff = CodaGitDiffResult.filterDiffByFile(
                        diff.diffUnified,
                        filePath: store.gitDiffSelectedFile,
                        previousPath: selectedChangedItem?.previousPath
                    )
                    let rpcErr = diff.error?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
                    let displayBody: String = {
                        if !filteredDiff.isEmpty {
                            return filteredDiff
                        }
                        if !rpcErr.isEmpty {
                            return rpcErr
                        }
                        if let uiErr = store.gitDiffError, !uiErr.isEmpty {
                            return uiErr
                        }
                        if let selected = store.gitDiffSelectedFile {
                            return tr(
                                "No unified diff hunks for \(selected) (untracked, mode-only, or binary file — tap Open File to inspect).",
                                "\(selected) 暂无统一差异块（可能是未跟踪新文件或二进制文件，可点击右上角“预览文件”查看）。",
                                locale: store.locale
                            )
                        }
                        if !changedFiles.isEmpty {
                            return tr(
                                "Working tree has \(changedFiles.count) untracked or binary file(s). Select a file chip above to inspect.",
                                "工作区有 \(changedFiles.count) 个未跟踪新文件或二进制变更，请点击上方文件标签查看。",
                                locale: store.locale
                            )
                        }
                        return tr("Working tree clean", "工作区无未提交变更", locale: store.locale)
                    }()

                    ScrollView([.vertical, .horizontal]) {
                        LazyVStack(alignment: .leading, spacing: 2) {
                            let rawLines = displayBody.components(separatedBy: "\n")
                            ForEach(Array(rawLines.enumerated()), id: \.offset) { _, line in
                                diffLineRow(line)
                            }
                        }
                        .padding(10)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .background(RoundedRectangle(cornerRadius: 10).fill(CodaHerdrTheme.elevatedBG))
                }
            } else {
                VStack(spacing: 8) {
                    Text(tr("Tap Refresh to inspect remote Git changes.", "点击右上角刷新以审查远端代码变更。", locale: store.locale))
                        .font(.system(size: 12))
                        .foregroundStyle(CodaHerdrTheme.textSecondary)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
    }

    private func statusBadgeColor(_ status: String) -> Color {
        if status.contains("D") { return CodaHerdrTheme.redDiff }
        if status.contains("A") || status == "??" { return CodaHerdrTheme.greenDiff }
        if status.contains("R") || status.contains("C") { return CodaHerdrTheme.blueUnread }
        return CodaHerdrTheme.amberNeedsYou
    }

    private func fileFilterChip(
        status: String?,
        label: String,
        isSelected: Bool,
        action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            HStack(spacing: 5) {
                if let status {
                    Text(status)
                        .font(.system(size: 10, weight: .bold, design: .monospaced))
                        .foregroundStyle(statusBadgeColor(status))
                        .padding(.horizontal, 4.5)
                        .padding(.vertical, 1.5)
                        .background(Capsule().fill(statusBadgeColor(status).opacity(0.16)))
                }
                Text(label)
                    .font(.system(size: 11.5, weight: isSelected ? .semibold : .regular, design: .monospaced))
                    .foregroundStyle(isSelected ? CodaHerdrTheme.textPrimary : CodaHerdrTheme.textSecondary)
                    .lineLimit(1)
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 5)
            .background(
                Capsule()
                    .fill(isSelected ? CodaHerdrTheme.pillActiveBG : CodaHerdrTheme.elevatedBG)
                    .overlay(
                        Capsule().strokeBorder(
                            isSelected ? CodaHerdrTheme.cyanWorking.opacity(0.65) : CodaHerdrTheme.borderHairline,
                            lineWidth: 1
                        )
                    )
            )
        }
        .buttonStyle(.plain)
    }

    private func diffLineRow(_ line: String) -> some View {
        let isAdd = line.hasPrefix("+") && !line.hasPrefix("+++")
        let isDel = line.hasPrefix("-") && !line.hasPrefix("---")
        let isHunk = line.hasPrefix("@@")
        let isMeta = line.hasPrefix("diff --git") || line.hasPrefix("---") || line.hasPrefix("+++") || line.hasPrefix("index ")

        let fg: Color = isAdd
            ? CodaHerdrTheme.greenDiff
            : (isDel
                ? CodaHerdrTheme.redDiff
                : (isHunk
                    ? CodaHerdrTheme.cyanWorking
                    : (isMeta ? CodaHerdrTheme.textSecondary : CodaHerdrTheme.textPrimary)))

        let bg: Color = isAdd
            ? CodaHerdrTheme.greenDiff.opacity(0.12)
            : (isDel
                ? CodaHerdrTheme.redDiff.opacity(0.12)
                : (isHunk ? CodaHerdrTheme.cyanWorking.opacity(0.09) : .clear))

        return Text(line.isEmpty ? " " : line)
            .font(.system(size: 11.5, weight: isMeta ? .semibold : .regular, design: .monospaced))
            .foregroundStyle(fg)
            .textSelection(.enabled)
            .padding(.horizontal, 6)
            .padding(.vertical, 1)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: 3).fill(bg))
    }
}
