import CodaKit
import Combine
import SwiftUI

public enum CodaInspectorSheetMode: String, CaseIterable, Identifiable {
    case files
    case portPreview

    public var id: String { rawValue }
}

private final class RemoteFilesLocalState: ObservableObject {
    @Published var portText: String = "3000"
    @Published var pathText: String = "/"
}

public struct CodaRemoteFilesSheet: View {
    @ObservedObject public var store: CodaWorkspaceStore
    public let session: CodaAgentSession

    @StateObject private var local = RemoteFilesLocalState()

    public init(store: CodaWorkspaceStore, session: CodaAgentSession) {
        self.store = store
        self.session = session
    }

    public var body: some View {
        NavigationStack {
            VStack(spacing: 10) {
                Picker("", selection: $store.inspectorSheetMode) {
                    Text(tr("Files", "远端文件", locale: store.locale)).tag(CodaInspectorSheetMode.files)
                    Text(tr("Port Preview", "端口预览", locale: store.locale)).tag(CodaInspectorSheetMode.portPreview)
                }
                .pickerStyle(.segmented)
                .padding(.horizontal, 14)
                .padding(.top, 8)

                if store.inspectorSheetMode == .files {
                    filesBody
                } else {
                    portPreviewBody
                }
            }
            .background(CodaHerdrTheme.canvasBG)
            .navigationTitle(
                store.inspectorSheetMode == .files
                    ? tr("Workspace Files (E2EE)", "工作区文件 (E2EE)", locale: store.locale)
                    : tr("Localhost Port Preview", "本地端口加密预览", locale: store.locale)
            )
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

    @ViewBuilder
    private var filesBody: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                if !store.remoteFileSubPath.isEmpty {
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
                        ? store.remoteFileRootPath
                        : "\(store.remoteFileRootPath)/\(store.remoteFileSubPath)"
                )
                .font(.system(size: 11, design: .monospaced))
                .foregroundStyle(CodaHerdrTheme.textSecondary)
                .lineLimit(1)
                .truncationMode(.head)

                Spacer()
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
                    Text(selectedRel)
                        .font(.system(size: 11.5, weight: .semibold, design: .monospaced))
                        .foregroundStyle(CodaHerdrTheme.cyanWorking)
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
                HStack {
                    Text("HTTP \(res.statusCode)")
                        .font(.system(size: 12, weight: .bold, design: .monospaced))
                        .foregroundStyle((200..<300).contains(res.statusCode) ? CodaHerdrTheme.greenDiff : CodaHerdrTheme.amberNeedsYou)
                    Spacer()
                    Text(res.contentType)
                        .font(.system(size: 11, design: .monospaced))
                        .foregroundStyle(CodaHerdrTheme.textSecondary)
                        .lineLimit(1)
                }
                .padding(.horizontal, 14)

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
            } else {
                Spacer()
            }
        }
    }
}
