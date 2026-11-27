import CoreGraphics
import CryptoKit
import Darwin
import Foundation
import Security

// MARK: - Secure Secret Storage (Keychain on Signed Bundles / iOS + 0600 File Store for CLI / Tests)

public enum CodaSecretStore {
    private static let serviceName = "dev.codahub.client.secrets.v1"

    private static var shouldBypassKeychain: Bool {
        if let override = ProcessInfo.processInfo.environment["CODA_APPLE_SECRETS_DIR"], !override.isEmpty {
            return true
        }
        #if os(macOS)
        if NSClassFromString("XCTestCase") != nil
            || Bundle.main.bundleIdentifier?.hasPrefix("com.apple.dt.") == true {
            return true
        }
        // Unsigned CLI executables and XCTest runners on macOS trigger modal Keychain prompts across rebuilds;
        // use 0600 file-backed secret storage unless running inside a signed .app bundle.
        return Bundle.main.bundleURL.pathExtension != "app"
        #else
        return false
        #endif
    }

    private static func fallbackDirectoryURL() -> URL {
        if let override = ProcessInfo.processInfo.environment["CODA_APPLE_SECRETS_DIR"], !override.isEmpty {
            return URL(fileURLWithPath: override, isDirectory: true)
        }
        if let appSupport = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first {
            return appSupport.appendingPathComponent("Coda/secrets", isDirectory: true)
        }
        return FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".config/coda/apple-secrets", isDirectory: true)
    }

    private static func fallbackFileURL(account: String) -> URL {
        let digest = SHA256.hash(data: Data(account.utf8))
        let hex = digest.map { String(format: "%02x", $0) }.joined()
        return fallbackDirectoryURL().appendingPathComponent("\(hex).bin", isDirectory: false)
    }

    private static func writeFallback(_ data: Data, account: String) {
        let dir = fallbackDirectoryURL()
        try? FileManager.default.createDirectory(
            at: dir,
            withIntermediateDirectories: true,
            attributes: [.posixPermissions: NSNumber(value: Int16(0o700))]
        )
        _ = chmod(dir.path, 0o700)

        let target = fallbackFileURL(account: account)
        let tmp = dir.appendingPathComponent(".\(UUID().uuidString).tmp", isDirectory: false)
        let fd = open(tmp.path, O_WRONLY | O_CREAT | O_TRUNC, 0o600)
        guard fd >= 0 else { return }
        let wroteAll = data.withUnsafeBytes { rawBuf -> Bool in
            guard let base = rawBuf.baseAddress else { return true }
            var offset = 0
            while offset < rawBuf.count {
                let n = Darwin.write(fd, base.advanced(by: offset), rawBuf.count - offset)
                if n < 0 {
                    if errno == EINTR { continue }
                    return false
                }
                offset += n
            }
            return true
        }
        _ = fsync(fd)
        _ = fchmod(fd, 0o600)
        _ = close(fd)
        if wroteAll {
            _ = rename(tmp.path, target.path)
        } else {
            _ = unlink(tmp.path)
        }
    }

    private static func readFallback(account: String) -> Data? {
        let url = fallbackFileURL(account: account)
        return try? Data(contentsOf: url)
    }

    private static func deleteFallback(account: String) {
        let url = fallbackFileURL(account: account)
        try? FileManager.default.removeItem(at: url)
    }

    public static func saveData(_ data: Data, account: String) {
        if shouldBypassKeychain {
            writeFallback(data, account: account)
            return
        }

        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: serviceName,
            kSecAttrAccount as String: account,
        ]
        let updateAttrs: [String: Any] = [
            kSecValueData as String: data,
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
        ]

        var status = SecItemUpdate(query as CFDictionary, updateAttrs as CFDictionary)
        if status == errSecItemNotFound {
            var addQuery = query
            addQuery[kSecValueData as String] = data
            addQuery[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
            status = SecItemAdd(addQuery as CFDictionary, nil)
        }
        if status == errSecSuccess {
            deleteFallback(account: account)
        } else {
            writeFallback(data, account: account)
        }
    }

    public static func readData(account: String) -> Data? {
        if !shouldBypassKeychain {
            let query: [String: Any] = [
                kSecClass as String: kSecClassGenericPassword,
                kSecAttrService as String: serviceName,
                kSecAttrAccount as String: account,
                kSecReturnData as String: true,
                kSecMatchLimit as String: kSecMatchLimitOne,
            ]
            var item: CFTypeRef?
            let status = SecItemCopyMatching(query as CFDictionary, &item)
            if status == errSecSuccess, let data = item as? Data {
                return data
            }
        }
        return readFallback(account: account)
    }

    public static func saveString(_ value: String, account: String) {
        if value.isEmpty {
            delete(account: account)
        } else {
            saveData(Data(value.utf8), account: account)
        }
    }

    public static func readString(account: String) -> String? {
        guard let data = readData(account: account) else { return nil }
        return String(data: data, encoding: .utf8)
    }

    public static func delete(account: String) {
        if !shouldBypassKeychain {
            let query: [String: Any] = [
                kSecClass as String: kSecClassGenericPassword,
                kSecAttrService as String: serviceName,
                kSecAttrAccount as String: account,
            ]
            _ = SecItemDelete(query as CFDictionary)
        }
        deleteFallback(account: account)
    }
}

// MARK: - Zero-Knowledge Device Identity & E2EE Session (100% Rust `coda-protocol` Wire Compatible)

public struct CodaDeviceIdentity: Codable, Sendable, Equatable {
    public var deviceId: UUID?
    public let name: String
    public let ed25519SecretB64: String
    public let ed25519PublicB64: String
    public let x25519SecretB64: String
    public let x25519PublicB64: String

    enum CodingKeys: String, CodingKey {
        case deviceId = "device_id"
        case name
        case ed25519SecretB64 = "ed25519_secret_b64"
        case ed25519PublicB64 = "ed25519_public_b64"
        case x25519SecretB64 = "x25519_secret_b64"
        case x25519PublicB64 = "x25519_public_b64"
    }

    private static let persistedDefaultsKey = "dev.codahub.client.device_identity.v2"
    private static let secretStoreAccount = "coda.device_identity.v2"

    public static func generate(name: String) -> CodaDeviceIdentity {
        let signing = Curve25519.Signing.PrivateKey()
        let agreement = Curve25519.KeyAgreement.PrivateKey()
        return CodaDeviceIdentity(
            deviceId: nil,
            name: name,
            ed25519SecretB64: signing.rawRepresentation.base64EncodedString(),
            ed25519PublicB64: signing.publicKey.rawRepresentation.base64EncodedString(),
            x25519SecretB64: agreement.rawRepresentation.base64EncodedString(),
            x25519PublicB64: agreement.publicKey.rawRepresentation.base64EncodedString()
        )
    }

    public static func loadOrCreatePersisted(defaultName: String) -> CodaDeviceIdentity {
        if let data = CodaSecretStore.readData(account: secretStoreAccount),
           let decoded = try? JSONDecoder().decode(CodaDeviceIdentity.self, from: data) {
            // Ensure any legacy plaintext copy in UserDefaults is purged.
            UserDefaults.standard.removeObject(forKey: persistedDefaultsKey)
            return decoded
        }
        if let legacyData = UserDefaults.standard.data(forKey: persistedDefaultsKey),
           let legacy = try? JSONDecoder().decode(CodaDeviceIdentity.self, from: legacyData) {
            savePersisted(legacy)
            return legacy
        }
        let created = generate(name: defaultName)
        savePersisted(created)
        return created
    }

    public static func savePersisted(_ identity: CodaDeviceIdentity) {
        if let data = try? JSONEncoder().encode(identity) {
            CodaSecretStore.saveData(data, account: secretStoreAccount)
        }
        UserDefaults.standard.removeObject(forKey: persistedDefaultsKey)
    }

    public static func resetPersisted(defaultName: String) -> CodaDeviceIdentity {
        CodaSecretStore.delete(account: secretStoreAccount)
        UserDefaults.standard.removeObject(forKey: persistedDefaultsKey)
        let fresh = generate(name: defaultName)
        savePersisted(fresh)
        return fresh
    }

    public func sign(message: Data) throws -> String {
        guard let raw = Data(base64Encoded: ed25519SecretB64) else {
            throw CodaCryptoError.invalidKeyEncoding
        }
        let key = try Curve25519.Signing.PrivateKey(rawRepresentation: raw)
        let sig = try key.signature(for: message)
        return sig.base64EncodedString()
    }

    public static func verifySignature(
        ed25519PublicB64: String,
        message: Data,
        signatureB64: String
    ) -> Bool {
        guard let pkRaw = Data(base64Encoded: ed25519PublicB64),
              pkRaw.count == 32,
              let sigRaw = Data(base64Encoded: signatureB64),
              sigRaw.count == 64,
              let vk = try? Curve25519.Signing.PublicKey(rawRepresentation: pkRaw) else {
            return false
        }
        return vk.isValidSignature(sigRaw, for: message)
    }
}

public enum CodaAPIError: Error, LocalizedError, Sendable {
    case httpError(statusCode: Int, message: String)
    case invalidResponse

    public var errorDescription: String? {
        switch self {
        case .httpError(_, let message): return message
        case .invalidResponse: return "Unexpected server response"
        }
    }

    public static func fromResponse(data: Data, response: URLResponse, fallback: String) -> CodaAPIError {
        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        if let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
           let err = json["error"] as? String, !err.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            return .httpError(statusCode: status, message: err)
        }
        if let txt = String(data: data, encoding: .utf8)?.trimmingCharacters(in: .whitespacesAndNewlines),
           !txt.isEmpty, txt.count < 240 {
            return .httpError(statusCode: status, message: txt)
        }
        return .httpError(statusCode: status, message: status > 0 ? "\(fallback) (HTTP \(status))" : fallback)
    }
}

public enum CodaNetworkError: Error, LocalizedError, Sendable, Equatable {
    case remoteOperationFailed(String)
    case invalidPayload(String)

    public var errorDescription: String? {
        switch self {
        case .remoteOperationFailed(let message),
             .invalidPayload(let message):
            return message
        }
    }
}

public enum CodaClipboardPasteContent: Sendable, Equatable {
    case text(String)
    case imagePNG(Data)
    case empty
}

public enum CodaClipboardPasteResolver {
    /// Prioritizes non-empty plain text over rasterized clipboard images so copying cells or styled text
    /// from Numbers, Excel, Word, Keynote, Pages, or Notes pastes text instead of uploading a PNG snapshot.
    public static func resolve(
        text: String?,
        readImagePNG: () -> Data?
    ) -> CodaClipboardPasteContent {
        if let text, !text.isEmpty {
            return .text(text)
        }
        if let img = readImagePNG(), !img.isEmpty {
            return .imagePNG(img)
        }
        return .empty
    }
}

public enum CodaSheetSessionStateHelper {
    public static func reconciledActiveSheetSessionID(
        current: String?,
        selectedSessionID: String?,
        showComposeSheet: Bool,
        showPaneMinimapSheet: Bool,
        showGitDiffSheet: Bool,
        showRemoteFilesSheet: Bool,
        didSwitchSelectedSession: Bool = false,
        activeSheetIsSecondary: Bool = false
    ) -> String? {
        let anySheetOpen = showComposeSheet || showPaneMinimapSheet || showGitDiffSheet || showRemoteFilesSheet
        guard anySheetOpen else {
            return nil
        }
        if didSwitchSelectedSession && !activeSheetIsSecondary {
            return selectedSessionID
        }
        return current ?? selectedSessionID
    }
}

public enum CodaCryptoError: Error, LocalizedError {
    case invalidKeyEncoding
    case nonContributoryKey
    case replayDetected(UInt64)
    case invalidCiphertext
    case compressedFrameRequiresRustFFI
    case decompressedFrameTooLarge
    case invalidHandshake(String)

    public var errorDescription: String? {
        switch self {
        case .invalidKeyEncoding: return "Invalid Curve25519 key encoding"
        case .nonContributoryKey: return "Non-contributory (low-order) Curve25519 key"
        case .replayDetected(let seq): return "Replay or out-of-order E2EE sequence: \(seq)"
        case .invalidCiphertext: return "Invalid ChaCha20-Poly1305 ciphertext"
        case .compressedFrameRequiresRustFFI: return "zstd frame decompression requires libcoda_apple_ffi or supports_zstd=false negotiation"
        case .decompressedFrameTooLarge: return "Decompressed E2EE frame exceeds 16 MiB limit"
        case .invalidHandshake(let msg): return "E2EE handshake rejected: \(msg)"
        }
    }
}

public struct CodaHostHealthSnapshot: Codable, Sendable, Equatable {
    public let daemonVersion: String
    public let os: String
    public let arch: String
    public let cpuCores: Int
    public let loadAvg1m: Double?
    public let memUsedMb: UInt64?
    public let memTotalMb: UInt64?
    public let osUptimeSecs: UInt64?
    public let daemonUptimeSecs: UInt64
    public let herdrVersion: String?
    public let otaSupported: Bool

    public static let quickTerminalCommands: [String] = [
        "/compact",
        "/clear",
        "git status",
        "git diff",
        "cargo test",
    ]

    enum CodingKeys: String, CodingKey {
        case daemonVersion = "daemon_version"
        case os
        case arch
        case cpuCores = "cpu_cores"
        case loadAvg1m = "load_avg_1m"
        case memUsedMb = "mem_used_mb"
        case memTotalMb = "mem_total_mb"
        case osUptimeSecs = "os_uptime_secs"
        case daemonUptimeSecs = "daemon_uptime_secs"
        case herdrVersion = "herdr_version"
        case otaSupported = "ota_supported"
    }

    public var compactSummary: String {
        let ver = daemonVersion.hasPrefix("v") ? daemonVersion : "v\(daemonVersion)"
        var parts = [ver, "\(cpuCores)c"]
        if let load = loadAvg1m {
            parts.append(String(format: "load %.2f", load))
        }
        if let used = memUsedMb, let tot = memTotalMb, tot > 0 {
            parts.append(String(format: "%.1f/%.1fG", Double(used) / 1024.0, Double(tot) / 1024.0))
        }
        return parts.joined(separator: " · ")
    }
}

public struct CodaRemoteFileEntry: Codable, Sendable, Equatable, Identifiable {
    public var id: String { relPath }
    public let name: String
    public let relPath: String
    public let isDir: Bool
    public let sizeBytes: UInt64

    enum CodingKeys: String, CodingKey {
        case name
        case relPath = "rel_path"
        case isDir = "is_dir"
        case sizeBytes = "size_bytes"
    }

    public var formattedSize: String {
        if isDir { return "" }
        if sizeBytes < 1024 { return "\(sizeBytes) B" }
        if sizeBytes < 1024 * 1024 { return String(format: "%.1f KB", Double(sizeBytes) / 1024.0) }
        return String(format: "%.1f MB", Double(sizeBytes) / (1024.0 * 1024.0))
    }
}

public struct CodaDirectoryListingResult: Sendable, Equatable {
    public let rootPath: String
    public let subPath: String
    public let entries: [CodaRemoteFileEntry]
    public let error: String?

    public init(rootPath: String, subPath: String, entries: [CodaRemoteFileEntry], error: String?) {
        self.rootPath = rootPath
        self.subPath = subPath
        self.entries = entries
        self.error = error
    }
}

public struct CodaFilePreviewResult: Sendable, Equatable {
    public let path: String
    public let content: String
    public let error: String?
    public let truncated: Bool

    public init(path: String, content: String, error: String?, truncated: Bool = false) {
        self.path = path
        self.content = content
        self.error = error
        self.truncated = truncated
    }
}

public struct CodaPortPreviewResult: Sendable, Equatable {
    public let statusCode: Int
    public let contentType: String
    public let bodyText: String
    public let rawHTML: String?
    public let truncated: Bool

    public var isHTML: Bool {
        let lower = contentType.lowercased()
        return lower.contains("text/html") || lower.contains("application/xhtml")
    }

    public init(
        statusCode: Int,
        contentType: String,
        bodyText: String,
        rawHTML: String? = nil,
        truncated: Bool = false
    ) {
        self.statusCode = statusCode
        self.contentType = contentType
        self.bodyText = bodyText
        self.rawHTML = rawHTML
        self.truncated = truncated
    }
}

public struct CodaTranscribeAudioResult: Codable, Sendable, Equatable {
    public let ok: Bool
    public let text: String
    public let error: String?

    public init(ok: Bool, text: String, error: String? = nil) {
        self.ok = ok
        self.text = text
        self.error = error
    }
}

public struct CodaGitChangedFile: Identifiable, Codable, Sendable, Equatable, Hashable {
    public var id: String { path }
    public let status: String
    public let path: String
    public let previousPath: String?

    public var isDeleted: Bool {
        status.contains("D")
    }

    public var isDirectoryEntry: Bool {
        path.hasSuffix("/")
    }

    public init(status: String, path: String, previousPath: String? = nil) {
        self.status = status
        self.path = path
        self.previousPath = previousPath
    }
}

public struct CodaGitDiffResult: Codable, Sendable, Equatable {
    public let branch: String
    public let statusShort: String
    public let diffUnified: String
    public let isGitRepo: Bool
    public let repoRoot: String?
    public let error: String?

    enum CodingKeys: String, CodingKey {
        case branch
        case statusShort = "status_short"
        case diffUnified = "diff_unified"
        case isGitRepo = "is_git_repo"
        case repoRoot = "repo_root"
        case error
    }

    public init(
        branch: String,
        statusShort: String,
        diffUnified: String,
        isGitRepo: Bool = true,
        repoRoot: String? = nil,
        error: String? = nil
    ) {
        self.branch = branch
        self.statusShort = statusShort
        self.diffUnified = diffUnified
        self.isGitRepo = isGitRepo
        self.repoRoot = repoRoot
        self.error = error
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        self.branch = (try? c.decodeIfPresent(String.self, forKey: .branch)) ?? ""
        self.statusShort = (try? c.decodeIfPresent(String.self, forKey: .statusShort)) ?? ""
        self.diffUnified = (try? c.decodeIfPresent(String.self, forKey: .diffUnified)) ?? ""
        self.isGitRepo = (try? c.decodeIfPresent(Bool.self, forKey: .isGitRepo)) ?? true
        self.repoRoot = try? c.decodeIfPresent(String.self, forKey: .repoRoot)
        self.error = try? c.decodeIfPresent(String.self, forKey: .error)
    }

    public var changedFiles: [CodaGitChangedFile] {
        Self.parseStatusShort(statusShort)
    }

    /// Parses `git status --short` output into structured `CodaGitChangedFile` items.
    public static func parseStatusShort(_ raw: String) -> [CodaGitChangedFile] {
        var out: [CodaGitChangedFile] = []
        var seen = Set<String>()
        for rawLine in raw.components(separatedBy: "\n") {
            let cleanCR = rawLine.hasSuffix("\r") ? String(rawLine.dropLast()) : rawLine
            guard !cleanCR.trimmingCharacters(in: .whitespaces).isEmpty else { continue }

            let statusCode: String
            let restPath: String
            if cleanCR.count >= 3 {
                let idx2 = cleanCR.index(cleanCR.startIndex, offsetBy: 2)
                if cleanCR[idx2] == " " {
                    let prefix = String(cleanCR[..<idx2]).trimmingCharacters(in: .whitespaces)
                    statusCode = prefix.isEmpty ? "M" : prefix
                    restPath = String(cleanCR[cleanCR.index(after: idx2)...]).trimmingCharacters(in: .whitespaces)
                } else {
                    let trimmed = cleanCR.trimmingCharacters(in: .whitespaces)
                    if let sp = trimmed.firstIndex(where: { $0.isWhitespace }) {
                        statusCode = String(trimmed[..<sp])
                        restPath = String(trimmed[sp...]).trimmingCharacters(in: .whitespaces)
                    } else {
                        continue
                    }
                }
            } else {
                continue
            }

            guard !restPath.isEmpty else { continue }
            let previousPath: String?
            let currentPath: String
            let isRenameOrCopy = statusCode.contains("R") || statusCode.contains("C")
            if isRenameOrCopy, let arrowRange = restPath.range(of: " -> ") {
                previousPath = unquoteGitPath(String(restPath[..<arrowRange.lowerBound]))
                currentPath = unquoteGitPath(String(restPath[arrowRange.upperBound...]))
            } else {
                previousPath = nil
                currentPath = unquoteGitPath(restPath)
            }
            guard !currentPath.isEmpty, seen.insert(currentPath).inserted else { continue }
            out.append(CodaGitChangedFile(status: statusCode, path: currentPath, previousPath: previousPath))
        }
        return out
    }

    /// Filters a unified `git diff` string down to hunks touching `filePath` (or `previousPath` on split low-similarity renames).
    public static func filterDiffByFile(
        _ diffUnified: String,
        filePath: String?,
        previousPath: String? = nil
    ) -> String {
        guard let rawTarget = filePath?.trimmingCharacters(in: .whitespacesAndNewlines),
              !rawTarget.isEmpty else {
            return diffUnified
        }
        let target = rawTarget.hasPrefix("./") ? String(rawTarget.dropFirst(2)) : rawTarget
        let prevTarget: String? = previousPath.flatMap { rawPrev in
            let clean = rawPrev.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !clean.isEmpty else { return nil }
            return clean.hasPrefix("./") ? String(clean.dropFirst(2)) : clean
        }
        guard !diffUnified.isEmpty else { return "" }

        let lines = diffUnified.components(separatedBy: "\n")
        var sections: [[String]] = []
        var current: [String] = []
        for line in lines {
            if line.hasPrefix("diff --git ") {
                if !current.isEmpty {
                    sections.append(current)
                }
                current = [line]
            } else if !current.isEmpty {
                current.append(line)
            }
        }
        if !current.isEmpty {
            sections.append(current)
        }
        if sections.isEmpty {
            return diffUnified
        }

        let matched = sections.filter { section in
            if sectionMatchesFile(section, target: target) {
                return true
            }
            if let prevTarget, sectionMatchesFile(section, target: prevTarget) {
                return true
            }
            return false
        }
        return matched.map { $0.joined(separator: "\n") }.joined(separator: "\n")
    }

    private static func sectionMatchesFile(_ section: [String], target: String) -> Bool {
        guard let first = section.first else { return false }
        let headerSuffixes = [" b/\(target)", " \"b/\(target)\"", " a/\(target)", " \"a/\(target)\""]
        if headerSuffixes.contains(where: { first.hasSuffix($0) || first.contains("\($0) ") }) {
            return true
        }
        for line in section {
            if line.hasPrefix("@@") { break }
            if line.hasPrefix("+++ ") || line.hasPrefix("--- ") {
                let raw = unquoteGitPath(String(line.dropFirst(4)).trimmingCharacters(in: .whitespaces))
                let stripped: String
                if raw.hasPrefix("a/") || raw.hasPrefix("b/") {
                    stripped = String(raw.dropFirst(2))
                } else {
                    stripped = raw
                }
                if stripped == target { return true }
            } else if line.hasPrefix("rename to ") {
                if unquoteGitPath(String(line.dropFirst("rename to ".count))) == target { return true }
            } else if line.hasPrefix("rename from ") {
                if unquoteGitPath(String(line.dropFirst("rename from ".count))) == target { return true }
            }
        }
        return false
    }

    private static func unquoteGitPath(_ raw: String) -> String {
        let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed.count >= 2, trimmed.hasPrefix("\""), trimmed.hasSuffix("\"") else {
            return trimmed
        }
        let inner = String(trimmed.dropFirst().dropLast())
        return inner
            .replacingOccurrences(of: "\\\"", with: "\"")
            .replacingOccurrences(of: "\\\\", with: "\\")
    }
}

/// Shell-escapes a file path safely for POSIX shells (`sh`/`bash`/`zsh`), stripping NUL bytes and blocking flag/command injection.
public func codaShellQuotePath(_ path: String) -> String {
    let sanitized = path.replacingOccurrences(of: "\0", with: "")
    guard !sanitized.isEmpty else { return "''" }
    let isSafeBare = !sanitized.hasPrefix("-") && sanitized.unicodeScalars.allSatisfy { scalar in
        let v = scalar.value
        return (v >= 0x30 && v <= 0x39) // 0-9
            || (v >= 0x41 && v <= 0x5A) // A-Z
            || (v >= 0x61 && v <= 0x7A) // a-z
            || v == 0x5F || v == 0x2E || v == 0x2F || v == 0x2D // _ . / -
    }
    if isSafeBare {
        return sanitized
    }
    let escaped = sanitized.replacingOccurrences(of: "'", with: "'\\''")
    return "'\(escaped)'"
}

public enum CodaTerminalTranscriptHelper {
    public static let quickPreviewPorts: [Int] = [3000, 5173, 8000, 8080, 4321]

    public static func shellQuotePath(_ path: String) -> String {
        codaShellQuotePath(path)
    }

    /// Sanitizes text for terminal paste: normalizes `\r\n` and `\r` to `\n`, strips NUL bytes,
    /// and iteratively removes bracketed-paste markers (`\u{1B}[200~`, `\u{1B}[201~`, `\u{9B}200~`, `\u{9B}201~`)
    /// until fixed-point so nested markers like `\u{1B}[\u{1B}[201~201~` can never break out of paste mode.
    public static func sanitizeBracketedPasteText(_ text: String) -> String {
        var sanitized = text
            .replacingOccurrences(of: "\r\n", with: "\n")
            .replacingOccurrences(of: "\r", with: "\n")
            .replacingOccurrences(of: "\0", with: "")
        while true {
            let next = sanitized
                .replacingOccurrences(of: "\u{1B}[200~", with: "")
                .replacingOccurrences(of: "\u{1B}[201~", with: "")
                .replacingOccurrences(of: "\u{9B}200~", with: "")
                .replacingOccurrences(of: "\u{9B}201~", with: "")
            if next == sanitized {
                break
            }
            sanitized = next
        }
        return sanitized
    }

    /// Resolves whether multi-line prompt submissions should wrap in `\e[200~...\e[201~`:
    /// - If the terminal explicitly emitted DECSET `?2004h` or DECRST `?2004l` (`explicitSeen == true`),
    ///   strictly honor `engineEnabled` (so explicit `?2004l` is always respected).
    /// - Otherwise (`explicitSeen == false`, e.g. attaching to an already-running idle session),
    ///   default `true` ONLY for known bracketed-paste TUIs/shells (`claude`, `codex`, `gemini`, `agy`,
    ///   `grok`, `opencode`, `aider`, `cursor`, `goose`, `amp`, `zsh`, `fish`) and `false` for plain
    ///   shells/REPLs (`sh`, `dash`, `bash`, `python`, `psql`, `ssh`, `cat`, etc.).
    public static func resolveEffectiveBracketedPaste(
        explicitSeen: Bool,
        engineEnabled: Bool,
        agentName: String
    ) -> Bool {
        if explicitSeen {
            return engineEnabled
        }
        if engineEnabled {
            return true
        }
        let rawTrimmed = agentName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !rawTrimmed.isEmpty else { return false }
        let baseName = URL(fileURLWithPath: rawTrimmed).lastPathComponent.lowercased()
        let cleaned = baseName.hasPrefix("-") ? String(baseName.dropFirst()) : baseName
        let exactKnown: Set<String> = [
            "claude", "claude-code",
            "codex",
            "gemini", "gemini-cli",
            "agy", "antigravity",
            "grok",
            "opencode",
            "aider",
            "cursor", "cursor-agent",
            "goose",
            "amp",
            "zsh",
            "fish",
        ]
        if exactKnown.contains(cleaned) {
            return true
        }
        for prefix in ["claude-", "codex-", "gemini-", "opencode-", "aider-", "cursor-", "goose-", "amp-"] {
            if cleaned.hasPrefix(prefix) {
                return true
            }
        }
        return false
    }

    /// Encodes a prompt using terminal Bracketed Paste (`\u{001B}[200~<text>\u{001B}[201~\r`) when
    /// `bracketedModeEnabled` is true (DECSET `?2004h`), or clean newline-normalized UTF-8 when false.
    public static func encodeBracketedPaste(
        _ text: String,
        appendCarriageReturn: Bool = true,
        bracketedModeEnabled: Bool = true
    ) -> Data {
        let sanitized = sanitizeBracketedPasteText(text)
        var out = Data()
        if bracketedModeEnabled {
            out.append(contentsOf: "\u{1B}[200~".utf8)
            out.append(contentsOf: sanitized.utf8)
            out.append(contentsOf: "\u{1B}[201~".utf8)
        } else {
            out.append(contentsOf: sanitized.utf8)
        }
        if appendCarriageReturn {
            out.append(0x0D)
        }
        return out
    }

    public static func findMatchingLineIndices(in lines: [String], query: String) -> [Int] {
        let needle = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard !needle.isEmpty else { return [] }
        var out: [Int] = []
        for (idx, line) in lines.enumerated() where line.lowercased().contains(needle) {
            out.append(idx)
            if out.count >= 2000 { break }
        }
        return out
    }

    public static func buildMarkdown(
        title: String,
        cwd: String,
        hostName: String,
        lines: [String],
        exportedISO: String = ISO8601DateFormatter().string(from: Date())
    ) -> String {
        var end = lines.count
        while end > 0 && lines[end - 1].trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            end -= 1
        }
        let body = lines.prefix(end).map { line in
            var s = line
            while s.last?.isWhitespace == true { s.removeLast() }
            return s
        }.joined(separator: "\n")

        return """
        # Coda Session Transcript — \(title.isEmpty ? "Terminal" : title)

        - **Exported**: `\(exportedISO)`
        - **Host**: `\(hostName)`
        - **Working Directory**: `\(cwd.isEmpty ? "~" : cwd)`
        - **Lines**: `\(end)`

        ```text
        \(body)
        ```
        """
    }

    public static func parentSubPath(_ subPath: String) -> String {
        let clean = subPath.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        guard !clean.isEmpty, let slash = clean.lastIndex(of: "/") else { return "" }
        return String(clean[..<slash])
    }

    public static func joinRootAndRel(rootPath: String, relPath: String) -> String {
        var root = rootPath
        while root.count > 1 && root.hasSuffix("/") { root.removeLast() }
        if root.isEmpty { root = "/" }
        let rel = relPath.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        if rel.isEmpty { return root }
        return root == "/" ? "/\(rel)" : "\(root)/\(rel)"
    }
}

public struct CodaEncryptedPayload: Codable, Sendable {
    public let seq: UInt64
    public let compressed: Bool
    public let ciphertextB64: String
    public let sessionNonceB64: String?

    enum CodingKeys: String, CodingKey {
        case seq
        case compressed
        case ciphertextB64 = "ciphertext_b64"
        case sessionNonceB64 = "session_nonce_b64"
    }

    public init(seq: UInt64, compressed: Bool, ciphertextB64: String, sessionNonceB64: String? = nil) {
        self.seq = seq
        self.compressed = compressed
        self.ciphertextB64 = ciphertextB64
        self.sessionNonceB64 = sessionNonceB64
    }
}

/// 64-bit sliding anti-replay window matching `crates/coda-protocol::ReplayWindow`.
/// Call `check(_:)` before AEAD decryption and `commit(_:)` ONLY after AEAD authentication succeeds.
public struct CodaReplayWindow: Sendable {
    public private(set) var highestSeq: UInt64 = 0
    private var bitmap: UInt64 = 0

    public init() {}

    public func check(_ seq: UInt64) throws {
        if seq == 0 {
            throw CodaCryptoError.replayDetected(seq)
        }
        if highestSeq > 0 && seq <= highestSeq {
            let diff = highestSeq - seq
            if diff >= 64 {
                throw CodaCryptoError.replayDetected(seq)
            }
            if (bitmap & (UInt64(1) << diff)) != 0 {
                throw CodaCryptoError.replayDetected(seq)
            }
        }
    }

    public mutating func commit(_ seq: UInt64) {
        if highestSeq == 0 {
            highestSeq = seq
            bitmap = 1
        } else if seq > highestSeq {
            let shift = seq - highestSeq
            bitmap = shift >= 64 ? 1 : ((bitmap << shift) | 1)
            highestSeq = seq
        } else {
            let diff = highestSeq - seq
            if diff < 64 {
                bitmap |= (UInt64(1) << diff)
            }
        }
    }
}

private final class CodaFFILoader: @unchecked Sendable {
    static let shared = CodaFFILoader()
    private let lock = NSLock()
    private var loadedHandle: UnsafeMutableRawPointer?

    func resolveSymbol(_ name: String) -> UnsafeMutableRawPointer? {
        let rtldDefault = UnsafeMutableRawPointer(bitPattern: -2)
        if let sym = dlsym(rtldDefault, name) {
            return sym
        }
        lock.lock()
        defer { lock.unlock() }
        if let handle = loadedHandle, let sym = dlsym(handle, name) {
            return sym
        }
        for path in candidateDylibPaths() {
            guard FileManager.default.fileExists(atPath: path) else { continue }
            if let handle = dlopen(path, RTLD_NOW | RTLD_GLOBAL) {
                loadedHandle = handle
                if let sym = dlsym(handle, name) {
                    return sym
                }
            }
        }
        return nil
    }

    private func candidateDylibPaths() -> [String] {
        var candidates: [String] = []
        if let execURL = Bundle.main.executableURL {
            let execDir = execURL.deletingLastPathComponent()
            candidates.append(execDir.appendingPathComponent("libcoda_apple_ffi.dylib").path)
            candidates.append(execDir.appendingPathComponent("../Frameworks/libcoda_apple_ffi.dylib").path)
        }
        // Only allow developer / test-runner dylib search paths outside signed production .app bundles.
        #if DEBUG
        let allowDevSearch = true
        #else
        let allowDevSearch = Bundle.main.bundleURL.pathExtension != "app"
        #endif
        if allowDevSearch {
            if let env = ProcessInfo.processInfo.environment["CODA_FFI_DYLIB_PATH"], !env.isEmpty {
                candidates.append(env)
            }
            if let execURL = Bundle.main.executableURL {
                // Locate Cargo target/ relative to SwiftPM .build/<cfg>/ binary without leaking #filePath in Release.
                var dir = execURL.deletingLastPathComponent()
                for _ in 0..<5 {
                    candidates.append(dir.appendingPathComponent("target/release/libcoda_apple_ffi.dylib").path)
                    candidates.append(dir.appendingPathComponent("target/debug/libcoda_apple_ffi.dylib").path)
                    dir = dir.deletingLastPathComponent()
                }
            }
            #if DEBUG
            let sourceRoot = URL(fileURLWithPath: #filePath)
                .deletingLastPathComponent()
                .deletingLastPathComponent()
                .deletingLastPathComponent()
                .deletingLastPathComponent()
            candidates.append(sourceRoot.appendingPathComponent("target/release/libcoda_apple_ffi.dylib").path)
            candidates.append(sourceRoot.appendingPathComponent("target/debug/libcoda_apple_ffi.dylib").path)
            #endif
        }
        return candidates
    }
}

/// Pure Apple `CryptoKit` implementation of Coda's directional E2EE session (`X25519 + HKDF-SHA256 + ChaChaPoly`).
/// Matches `crates/coda-protocol/src/lib.rs` byte-for-byte, with dynamic `dlopen`/`dlsym` FFI support for binary-safe `zstd` frames.
public struct CodaE2EESession: Sendable {
    private static let maxDecompressedBytes = 16 * 1024 * 1024

    private let txKey: SymmetricKey
    private let rxKey: SymmetricKey
    private let txDirTag: UInt8
    private let rxDirTag: UInt8
    private let aadBinding: Data?
    public let sessionNonceB64: String?
    private var nextTxSeq: UInt64 = 1
    private var rxWindow = CodaReplayWindow()

    private typealias ZstdDecompressFn = @convention(c) (UnsafePointer<CChar>?) -> UnsafeMutablePointer<CChar>?
    private typealias StringFreeFn = @convention(c) (UnsafeMutablePointer<CChar>?) -> Void

    public static var hasNativeZstdFFI: Bool {
        CodaFFILoader.shared.resolveSymbol("coda_ffi_zstd_decompress_b64_to_b64") != nil
            || CodaFFILoader.shared.resolveSymbol("coda_ffi_zstd_decompress_b64") != nil
    }

    public static var hasRelayV2FFIBridge: Bool {
        CodaFFILoader.shared.resolveSymbol("coda_ffi_session_new_relay_v2") != nil
    }

    private static func checkContributorySharedSecret(_ shared: SharedSecret) throws -> Data {
        let raw = shared.withUnsafeBytes { Data($0) }
        guard raw.count == 32, raw.contains(where: { $0 != 0 }) else {
            throw CodaCryptoError.nonContributoryKey
        }
        return raw
    }

    public init(myX25519SecretB64: String, peerX25519PublicB64: String, isClientInitiator: Bool = true) throws {
        guard let myRaw = Data(base64Encoded: myX25519SecretB64),
              let peerRaw = Data(base64Encoded: peerX25519PublicB64) else {
            throw CodaCryptoError.invalidKeyEncoding
        }
        let myPriv = try Curve25519.KeyAgreement.PrivateKey(rawRepresentation: myRaw)
        let peerPub = try Curve25519.KeyAgreement.PublicKey(rawRepresentation: peerRaw)
        let shared = try myPriv.sharedSecretFromKeyAgreement(with: peerPub)
        _ = try Self.checkContributorySharedSecret(shared)

        let salt = Data("coda-zk-mesh-salt-v1".utf8)
        let c2d = shared.hkdfDerivedSymmetricKey(
            using: SHA256.self,
            salt: salt,
            sharedInfo: Data("coda-client-to-daemon-aead".utf8),
            outputByteCount: 32
        )
        let d2c = shared.hkdfDerivedSymmetricKey(
            using: SHA256.self,
            salt: salt,
            sharedInfo: Data("coda-daemon-to-client-aead".utf8),
            outputByteCount: 32
        )

        if isClientInitiator {
            self.txKey = c2d
            self.rxKey = d2c
            self.txDirTag = 0x01
            self.rxDirTag = 0x02
        } else {
            self.txKey = d2c
            self.rxKey = c2d
            self.txDirTag = 0x02
            self.rxDirTag = 0x01
        }
        self.aadBinding = nil
        self.sessionNonceB64 = nil
    }

    public init(myIdentity: CodaDeviceIdentity, peerX25519PublicB64: String, isInitiator: Bool = true) throws {
        try self.init(
            myX25519SecretB64: myIdentity.x25519SecretB64,
            peerX25519PublicB64: peerX25519PublicB64,
            isClientInitiator: isInitiator
        )
    }

    /// Direct P2P v2 hybrid static + ephemeral X25519 derivation matching `E2eeSession::derive_ephemeral_p2p`.
    public init(
        directP2Pv2Identity myIdentity: CodaDeviceIdentity,
        peerStaticX25519PublicB64: String,
        myEphemeralPrivateKey: Curve25519.KeyAgreement.PrivateKey,
        peerEphemeralX25519PublicB64: String,
        clientDeviceId: UUID,
        daemonDeviceId: UUID,
        isInitiator: Bool = true
    ) throws {
        guard let myStaticRaw = Data(base64Encoded: myIdentity.x25519SecretB64),
              let peerStaticRaw = Data(base64Encoded: peerStaticX25519PublicB64),
              let peerEphRaw = Data(base64Encoded: peerEphemeralX25519PublicB64),
              peerStaticRaw.count == 32,
              peerEphRaw.count == 32 else {
            throw CodaCryptoError.invalidKeyEncoding
        }
        let myStaticPriv = try Curve25519.KeyAgreement.PrivateKey(rawRepresentation: myStaticRaw)
        let peerStaticPub = try Curve25519.KeyAgreement.PublicKey(rawRepresentation: peerStaticRaw)
        let peerEphPub = try Curve25519.KeyAgreement.PublicKey(rawRepresentation: peerEphRaw)

        let staticShared = try Self.checkContributorySharedSecret(
            try myStaticPriv.sharedSecretFromKeyAgreement(with: peerStaticPub)
        )
        let ephShared = try Self.checkContributorySharedSecret(
            try myEphemeralPrivateKey.sharedSecretFromKeyAgreement(with: peerEphPub)
        )

        let myEphRaw = myEphemeralPrivateKey.publicKey.rawRepresentation
        let clientEphRaw = isInitiator ? myEphRaw : peerEphRaw
        let daemonEphRaw = isInitiator ? peerEphRaw : myEphRaw

        var hasher = SHA256()
        hasher.update(data: clientEphRaw)
        hasher.update(data: daemonEphRaw)
        let salt = Data(hasher.finalize())

        var ikm = Data(capacity: 64)
        ikm.append(staticShared)
        ikm.append(ephShared)

        let cStr = clientDeviceId.uuidString.lowercased()
        let dStr = daemonDeviceId.uuidString.lowercased()
        let c2d = HKDF<SHA256>.deriveKey(
            inputKeyMaterial: SymmetricKey(data: ikm),
            salt: salt,
            info: Data("coda-p2p-v2-c2d:\(cStr):\(dStr)".utf8),
            outputByteCount: 32
        )
        let d2c = HKDF<SHA256>.deriveKey(
            inputKeyMaterial: SymmetricKey(data: ikm),
            salt: salt,
            info: Data("coda-p2p-v2-d2c:\(cStr):\(dStr)".utf8),
            outputByteCount: 32
        )

        if isInitiator {
            self.txKey = c2d
            self.rxKey = d2c
            self.txDirTag = 0x01
            self.rxDirTag = 0x02
        } else {
            self.txKey = d2c
            self.rxKey = c2d
            self.txDirTag = 0x02
            self.rxDirTag = 0x01
        }
        self.aadBinding = Data(salt.prefix(16))
        self.sessionNonceB64 = nil
    }

    /// Relay v2 salted X25519 session derivation matching `E2eeSession::derive_relay_session`.
    public init(
        relayV2Identity myIdentity: CodaDeviceIdentity,
        peerStaticX25519PublicB64: String,
        sessionNonceB64: String,
        clientDeviceId: UUID,
        daemonDeviceId: UUID,
        isInitiator: Bool = true
    ) throws {
        guard let myStaticRaw = Data(base64Encoded: myIdentity.x25519SecretB64),
              let peerStaticRaw = Data(base64Encoded: peerStaticX25519PublicB64),
              let nonceRaw = Data(base64Encoded: sessionNonceB64),
              peerStaticRaw.count == 32,
              nonceRaw.count == 16 else {
            throw CodaCryptoError.invalidKeyEncoding
        }
        let myStaticPriv = try Curve25519.KeyAgreement.PrivateKey(rawRepresentation: myStaticRaw)
        let peerStaticPub = try Curve25519.KeyAgreement.PublicKey(rawRepresentation: peerStaticRaw)
        let staticShared = try Self.checkContributorySharedSecret(
            try myStaticPriv.sharedSecretFromKeyAgreement(with: peerStaticPub)
        )

        let cStr = clientDeviceId.uuidString.lowercased()
        let dStr = daemonDeviceId.uuidString.lowercased()
        let c2d = HKDF<SHA256>.deriveKey(
            inputKeyMaterial: SymmetricKey(data: staticShared),
            salt: nonceRaw,
            info: Data("coda-relay-v2-c2d:\(cStr):\(dStr)".utf8),
            outputByteCount: 32
        )
        let d2c = HKDF<SHA256>.deriveKey(
            inputKeyMaterial: SymmetricKey(data: staticShared),
            salt: nonceRaw,
            info: Data("coda-relay-v2-d2c:\(cStr):\(dStr)".utf8),
            outputByteCount: 32
        )

        if isInitiator {
            self.txKey = c2d
            self.rxKey = d2c
            self.txDirTag = 0x01
            self.rxDirTag = 0x02
        } else {
            self.txKey = d2c
            self.rxKey = c2d
            self.txDirTag = 0x02
            self.rxDirTag = 0x01
        }
        self.aadBinding = nonceRaw
        self.sessionNonceB64 = nonceRaw.base64EncodedString()
    }

    public static func randomSessionNonceB64() -> String {
        var bytes = [UInt8](repeating: 0, count: 16)
        _ = SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes)
        return Data(bytes).base64EncodedString()
    }

    public static func decompressZstdB64ToData(_ compressedB64: String) throws -> Data {
        if let sym = CodaFFILoader.shared.resolveSymbol("coda_ffi_zstd_decompress_b64_to_b64"),
           let freeSym = CodaFFILoader.shared.resolveSymbol("coda_ffi_string_free") {
            let decompFn = unsafeBitCast(sym, to: ZstdDecompressFn.self)
            let freeFn = unsafeBitCast(freeSym, to: StringFreeFn.self)
            if let outPtr = compressedB64.withCString({ decompFn($0) }) {
                defer { freeFn(outPtr) }
                let b64Out = String(cString: outPtr)
                guard let decoded = Data(base64Encoded: b64Out) else {
                    throw CodaCryptoError.invalidCiphertext
                }
                guard decoded.count <= maxDecompressedBytes else {
                    throw CodaCryptoError.decompressedFrameTooLarge
                }
                return decoded
            }
        }
        if let sym = CodaFFILoader.shared.resolveSymbol("coda_ffi_zstd_decompress_b64"),
           let freeSym = CodaFFILoader.shared.resolveSymbol("coda_ffi_string_free") {
            let decompFn = unsafeBitCast(sym, to: ZstdDecompressFn.self)
            let freeFn = unsafeBitCast(freeSym, to: StringFreeFn.self)
            if let outPtr = compressedB64.withCString({ decompFn($0) }) {
                defer { freeFn(outPtr) }
                let utf8 = String(cString: outPtr)
                let decoded = Data(utf8.utf8)
                guard decoded.count <= maxDecompressedBytes else {
                    throw CodaCryptoError.decompressedFrameTooLarge
                }
                return decoded
            }
        }
        throw CodaCryptoError.compressedFrameRequiresRustFFI
    }

    private static func makeNonce(dirTag: UInt8, seq: UInt64) throws -> ChaChaPoly.Nonce {
        var bytes = [UInt8](repeating: 0, count: 12)
        bytes[0] = dirTag
        var be = seq.bigEndian
        withUnsafeBytes(of: &be) { rawBuf in
            for i in 0..<8 {
                bytes[4 + i] = rawBuf[i]
            }
        }
        return try ChaChaPoly.Nonce(data: Data(bytes))
    }

    private func makeAAD(compressed: Bool, dirTag: UInt8) -> Data {
        let cByte: UInt8 = compressed ? 0x01 : 0x00
        guard let binding = aadBinding else {
            return Data([cByte])
        }
        var aad = Data(capacity: 2 + binding.count)
        aad.append(cByte)
        aad.append(dirTag)
        aad.append(binding)
        return aad
    }

    public mutating func sealJSON(_ jsonBytes: Data) throws -> CodaEncryptedPayload {
        let seq = nextTxSeq
        nextTxSeq += 1
        let nonce = try Self.makeNonce(dirTag: txDirTag, seq: seq)
        let aad = makeAAD(compressed: false, dirTag: txDirTag)
        let box = try ChaChaPoly.seal(jsonBytes, using: txKey, nonce: nonce, authenticating: aad)
        let combinedCiphertextAndTag = box.ciphertext + box.tag
        return CodaEncryptedPayload(
            seq: seq,
            compressed: false,
            ciphertextB64: combinedCiphertextAndTag.base64EncodedString(),
            sessionNonceB64: sessionNonceB64
        )
    }

    public mutating func openJSON(_ payload: CodaEncryptedPayload) throws -> Data {
        if let expectedNonce = sessionNonceB64 {
            guard payload.sessionNonceB64 == expectedNonce else {
                throw CodaCryptoError.invalidCiphertext
            }
        }
        try rxWindow.check(payload.seq)

        guard let raw = Data(base64Encoded: payload.ciphertextB64), raw.count >= 16 else {
            throw CodaCryptoError.invalidCiphertext
        }
        let ct = raw.prefix(raw.count - 16)
        let tag = raw.suffix(16)
        let nonce = try Self.makeNonce(dirTag: rxDirTag, seq: payload.seq)
        let box = try ChaChaPoly.SealedBox(nonce: nonce, ciphertext: ct, tag: tag)
        let aad = makeAAD(compressed: payload.compressed, dirTag: rxDirTag)
        let pt = try ChaChaPoly.open(box, using: rxKey, authenticating: aad)

        let decoded: Data
        if payload.compressed {
            decoded = try Self.decompressZstdB64ToData(pt.base64EncodedString())
        } else {
            guard pt.count <= Self.maxDecompressedBytes else {
                throw CodaCryptoError.decompressedFrameTooLarge
            }
            decoded = pt
        }

        // Only advance replay window AFTER AEAD authentication and bounded decompression succeed.
        rxWindow.commit(payload.seq)
        return decoded
    }
}

/// Safe Rust FFI session wrapper backed by `libcoda_apple_ffi.dylib` when available (used for cross-language verification).
public final class CodaRustFFISessionBridge: @unchecked Sendable {
    private typealias NewRelayV2Fn = @convention(c) (
        UnsafePointer<CChar>?, UnsafePointer<CChar>?, UnsafePointer<CChar>?,
        UnsafePointer<CChar>?, UnsafePointer<CChar>?, Bool, Bool
    ) -> OpaquePointer?
    private typealias FreeFn = @convention(c) (OpaquePointer?) -> Void
    private typealias CallFn = @convention(c) (OpaquePointer?, UnsafePointer<CChar>?) -> UnsafeMutablePointer<CChar>?
    private typealias StringFreeFn = @convention(c) (UnsafeMutablePointer<CChar>?) -> Void

    private var ptr: OpaquePointer?
    private let freeFn: FreeFn
    private let sealFn: CallFn
    private let openFn: CallFn
    private let strFreeFn: StringFreeFn

    public init?(
        identity: CodaDeviceIdentity,
        peerX25519PublicB64: String,
        sessionNonceB64: String,
        clientDeviceId: UUID,
        daemonDeviceId: UUID,
        isInitiator: Bool,
        peerSupportsZstd: Bool = true
    ) {
        guard let newSym = CodaFFILoader.shared.resolveSymbol("coda_ffi_session_new_relay_v2"),
              let freeSym = CodaFFILoader.shared.resolveSymbol("coda_ffi_session_free"),
              let sealSym = CodaFFILoader.shared.resolveSymbol("coda_ffi_session_seal"),
              let openSym = CodaFFILoader.shared.resolveSymbol("coda_ffi_session_open"),
              let strFreeSym = CodaFFILoader.shared.resolveSymbol("coda_ffi_string_free"),
              let idData = try? JSONEncoder().encode(identity),
              let idJSON = String(data: idData, encoding: .utf8) else {
            return nil
        }
        let newFn = unsafeBitCast(newSym, to: NewRelayV2Fn.self)
        self.freeFn = unsafeBitCast(freeSym, to: FreeFn.self)
        self.sealFn = unsafeBitCast(sealSym, to: CallFn.self)
        self.openFn = unsafeBitCast(openSym, to: CallFn.self)
        self.strFreeFn = unsafeBitCast(strFreeSym, to: StringFreeFn.self)

        let cid = clientDeviceId.uuidString.lowercased()
        let did = daemonDeviceId.uuidString.lowercased()
        self.ptr = idJSON.withCString { pId in
            peerX25519PublicB64.withCString { pPeer in
                sessionNonceB64.withCString { pNonce in
                    cid.withCString { pCid in
                        did.withCString { pDid in
                            newFn(pId, pPeer, pNonce, pCid, pDid, isInitiator, peerSupportsZstd)
                        }
                    }
                }
            }
        }
        if self.ptr == nil { return nil }
    }

    deinit {
        if let ptr {
            freeFn(ptr)
        }
    }

    public func sealPeerMessageJSON(_ msgJSON: String) -> String? {
        guard let ptr else { return nil }
        guard let outPtr = msgJSON.withCString({ sealFn(ptr, $0) }) else { return nil }
        defer { strFreeFn(outPtr) }
        return String(cString: outPtr)
    }

    public func openEncryptedPayloadJSON(_ payloadJSON: String) -> String? {
        guard let ptr else { return nil }
        guard let outPtr = payloadJSON.withCString({ openFn(ptr, $0) }) else { return nil }
        defer { strFreeFn(outPtr) }
        return String(cString: outPtr)
    }
}

// MARK: - Domain Models (Herdr Lifecycle, Multi-Tenant Devices, SmartPath Routes)

public enum SmartPathRouteMode: String, Codable, Sendable {
    case lanDirect = "L0 LAN Direct"
    case cnIPv6P2P = "L1 IPv6 P2P"
    case domesticRawTCP = "L2 CN RawTCP"
    case globalCDNWSS = "L2 CDN WSS"

    public static var lanDirectP2P: SmartPathRouteMode { .lanDirect }
    public static var ipv6DirectP2P: SmartPathRouteMode { .cnIPv6P2P }
}

public enum HerdrLifecycleState: String, Codable, Sendable, CaseIterable, Hashable {
    case needsYou = "needs_you"
    case done
    case working
    case idle
    case error

    public static var blocked: HerdrLifecycleState { .needsYou }

    public var priorityRank: Int {
        switch self {
        case .needsYou: return 0
        case .error: return 1
        case .done: return 2
        case .working: return 3
        case .idle: return 4
        }
    }

    public static func from(rawState: String?, kind: String? = nil) -> HerdrLifecycleState {
        let s = (rawState ?? "").trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        let k = (kind ?? "").trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        if s == "blocked" || s == "needs_you" || s == "needsyou" || s == "permission" || s == "question" || s == "auth"
            || k == "permission" || k == "question" || k == "auth" {
            return .needsYou
        }
        if s == "done" || s == "completed" || s == "finished" {
            return .done
        }
        if s == "working" || s == "running" || s == "busy" {
            return .working
        }
        if s == "error" || s == "failed" {
            return .error
        }
        return .idle
    }

    public init(from decoder: Decoder) throws {
        let raw = try decoder.singleValueContainer().decode(String.self)
        self = Self.from(rawState: raw)
    }
}

public struct CodaAgentSession: Identifiable, Codable, Sendable, Equatable, Hashable {
    public let id: String
    public let terminalAddress: String
    public let deviceId: UUID?
    public let workspaceName: String
    public let cwd: String
    public let branch: String
    public let hostName: String
    public let agentName: String
    public var taskTitle: String
    public var statusMessage: String?
    public var statusKind: String?
    public var state: HerdrLifecycleState
    public var helperCount: Int
    public var workingHelperCount: Int
    public var unreadCompletion: Bool
    public var isPinned: Bool
    public var updatedAt: Date
    public var cols: Int
    public var rows: Int
    public var alive: Bool
    public var diffAdded: Int
    public var diffDeleted: Int
    public var screenPreview: String

    public var address: String { terminalAddress }
    public var status: HerdrLifecycleState { state }

    public init(
        id: String,
        terminalAddress: String,
        deviceId: UUID? = nil,
        workspaceName: String,
        cwd: String = "",
        branch: String = "main",
        hostName: String,
        agentName: String,
        taskTitle: String,
        statusMessage: String? = nil,
        statusKind: String? = nil,
        state: HerdrLifecycleState,
        helperCount: Int = 0,
        workingHelperCount: Int = 0,
        unreadCompletion: Bool = false,
        isPinned: Bool = false,
        updatedAt: Date = Date(),
        cols: Int = 100,
        rows: Int = 32,
        alive: Bool = true,
        diffAdded: Int = 0,
        diffDeleted: Int = 0,
        screenPreview: String = ""
    ) {
        self.id = id
        self.terminalAddress = terminalAddress
        self.deviceId = deviceId
        self.workspaceName = workspaceName
        self.cwd = cwd
        self.branch = branch
        self.hostName = hostName
        self.agentName = agentName
        self.taskTitle = taskTitle
        self.statusMessage = statusMessage
        self.statusKind = statusKind
        self.state = state
        self.helperCount = helperCount
        self.workingHelperCount = workingHelperCount
        self.unreadCompletion = unreadCompletion
        self.isPinned = isPinned
        self.updatedAt = updatedAt
        self.cols = cols
        self.rows = rows
        self.alive = alive
        self.diffAdded = diffAdded
        self.diffDeleted = diffDeleted
        self.screenPreview = screenPreview
    }

    /// Canonical Herdr Inbox sort order: Needs you -> Error -> Done (unread first) -> Working -> Idle,
    /// keeping pinned sessions prioritized inside each bucket.
    public static func inboxOrder(_ lhs: CodaAgentSession, _ rhs: CodaAgentSession) -> Bool {
        if lhs.state.priorityRank != rhs.state.priorityRank {
            return lhs.state.priorityRank < rhs.state.priorityRank
        }
        if lhs.isPinned != rhs.isPinned {
            return lhs.isPinned
        }
        if lhs.unreadCompletion != rhs.unreadCompletion {
            return lhs.unreadCompletion
        }
        if lhs.updatedAt != rhs.updatedAt {
            return lhs.updatedAt > rhs.updatedAt
        }
        return lhs.id < rhs.id
    }
}

public enum CodaUILocale: String, Codable, CaseIterable, Sendable {
    case en
    case zh
}

public struct CodaPersistedSettings: Codable, Sendable, Equatable {
    public var serverBaseURLString: String
    public var sessionToken: String
    public var ownerHandle: String
    public var locale: CodaUILocale
    public var pinnedSessionIDs: [String]
    public var voiceConfig: CodaVoiceModelConfig
    public var registeredDeviceIDs: [String: String]
    public var notificationsEnabled: Bool

    private static let defaultsKey = "dev.codahub.client.settings.v2"
    private static let sessionTokenSecretAccount = "coda.session_token.v1"
    private static let voiceApiKeySecretAccount = "coda.voice_api_key.v1"

    enum CodingKeys: String, CodingKey {
        case serverBaseURLString
        case sessionToken
        case ownerHandle
        case locale
        case pinnedSessionIDs
        case voiceConfig
        case registeredDeviceIDs
        case notificationsEnabled
    }

    public init(
        serverBaseURLString: String = "https://codahub.dev",
        sessionToken: String = "",
        ownerHandle: String = "",
        locale: CodaUILocale = .en,
        pinnedSessionIDs: [String] = [],
        voiceConfig: CodaVoiceModelConfig = CodaVoiceModelConfig(),
        registeredDeviceIDs: [String: String] = [:],
        notificationsEnabled: Bool = true
    ) {
        self.serverBaseURLString = serverBaseURLString
        self.sessionToken = sessionToken
        self.ownerHandle = ownerHandle
        self.locale = locale
        self.pinnedSessionIDs = pinnedSessionIDs
        self.voiceConfig = voiceConfig
        self.registeredDeviceIDs = registeredDeviceIDs
        self.notificationsEnabled = notificationsEnabled
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        self.serverBaseURLString = (try? c.decodeIfPresent(String.self, forKey: .serverBaseURLString)) ?? "https://codahub.dev"
        self.sessionToken = (try? c.decodeIfPresent(String.self, forKey: .sessionToken)) ?? ""
        self.ownerHandle = (try? c.decodeIfPresent(String.self, forKey: .ownerHandle)) ?? ""
        self.locale = (try? c.decodeIfPresent(CodaUILocale.self, forKey: .locale)) ?? .en
        self.pinnedSessionIDs = (try? c.decodeIfPresent([String].self, forKey: .pinnedSessionIDs)) ?? []
        self.voiceConfig = (try? c.decodeIfPresent(CodaVoiceModelConfig.self, forKey: .voiceConfig)) ?? CodaVoiceModelConfig()
        self.registeredDeviceIDs = (try? c.decodeIfPresent([String: String].self, forKey: .registeredDeviceIDs)) ?? [:]
        self.notificationsEnabled = (try? c.decodeIfPresent(Bool.self, forKey: .notificationsEnabled)) ?? true
    }

    public static func load() -> CodaPersistedSettings {
        var settings: CodaPersistedSettings
        if let data = UserDefaults.standard.data(forKey: defaultsKey),
           let decoded = try? JSONDecoder().decode(CodaPersistedSettings.self, from: data) {
            settings = decoded
        } else {
            settings = CodaPersistedSettings()
        }

        // Migrate any legacy plaintext secrets out of UserDefaults into CodaSecretStore.
        var needsDefaultsRewrite = false
        if !settings.sessionToken.isEmpty {
            CodaSecretStore.saveString(settings.sessionToken, account: sessionTokenSecretAccount)
            needsDefaultsRewrite = true
        }
        if !settings.voiceConfig.apiKey.isEmpty {
            CodaSecretStore.saveString(settings.voiceConfig.apiKey, account: voiceApiKeySecretAccount)
            needsDefaultsRewrite = true
        }
        if needsDefaultsRewrite {
            var sanitized = settings
            sanitized.sessionToken = ""
            sanitized.voiceConfig.apiKey = ""
            if let data = try? JSONEncoder().encode(sanitized) {
                UserDefaults.standard.set(data, forKey: defaultsKey)
            }
        }

        settings.sessionToken = CodaSecretStore.readString(account: sessionTokenSecretAccount) ?? ""
        settings.voiceConfig.apiKey = CodaSecretStore.readString(account: voiceApiKeySecretAccount) ?? ""
        return settings
    }

    public func save() {
        CodaSecretStore.saveString(sessionToken, account: Self.sessionTokenSecretAccount)
        CodaSecretStore.saveString(voiceConfig.apiKey, account: Self.voiceApiKeySecretAccount)

        // Persist non-secret preferences only; secret token & API key stay in CodaSecretStore.
        var sanitized = self
        sanitized.sessionToken = ""
        sanitized.voiceConfig.apiKey = ""
        if let data = try? JSONEncoder().encode(sanitized) {
            UserDefaults.standard.set(data, forKey: Self.defaultsKey)
        }
    }

    public static func registrationKey(serverBaseURL: String, ownerHandle: String) -> String {
        let s = serverBaseURL.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        let normalized = s.hasSuffix("/") ? String(s.dropLast()) : s
        let h = ownerHandle.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        return "\(normalized)|\(h)"
    }
}

// MARK: - Wire Structs matching `crates/coda-protocol` & `crates/coda-server`

public struct CodaHerdrAgentStatus: Codable, Sendable, Equatable, Hashable {
    public let state: String
    public let kind: String?
    public let app: String?
    public let title: String?
    public let msg: String?
    public let progress: UInt8?
}

public struct CodaHerdrTerminalSummary: Codable, Sendable, Equatable, Hashable {
    public let address: String
    public let name: String?
    public let argv: [String]
    public let cwd: String
    public let cols: UInt16
    public let rows: UInt16
    public let alive: Bool
    public let status: CodaHerdrAgentStatus?
}

public struct CodaAccountLiveEvent: Codable, Sendable, Equatable {
    public let kind: String
    public let deviceId: UUID?
    public let deviceName: String?
    public let role: String?
    public let online: Bool?
    public let timestampMs: UInt64?

    enum CodingKeys: String, CodingKey {
        case kind
        case deviceId = "device_id"
        case deviceName = "device_name"
        case role
        case online
        case timestampMs = "timestamp_ms"
    }
}

public struct CodaBranchWorktreeItem: Identifiable, Codable, Sendable, Equatable {
    public var id: String { "\(worktreePath)#\(branch)" }
    public let branch: String
    public let worktreePath: String
    public let diffAdded: UInt32
    public let diffDeleted: UInt32
    public let activeTerminals: [CodaHerdrTerminalSummary]
    public let statusSummary: String

    enum CodingKeys: String, CodingKey {
        case branch
        case worktreePath = "worktree_path"
        case diffAdded = "diff_added"
        case diffDeleted = "diff_deleted"
        case activeTerminals = "active_terminals"
        case statusSummary = "status_summary"
    }

    public init(
        branch: String,
        worktreePath: String,
        diffAdded: UInt32,
        diffDeleted: UInt32,
        activeTerminals: [CodaHerdrTerminalSummary] = [],
        statusSummary: String
    ) {
        self.branch = branch
        self.worktreePath = worktreePath
        self.diffAdded = diffAdded
        self.diffDeleted = diffDeleted
        self.activeTerminals = activeTerminals
        self.statusSummary = statusSummary
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        branch = try c.decode(String.self, forKey: .branch)
        worktreePath = try c.decode(String.self, forKey: .worktreePath)
        diffAdded = try c.decode(UInt32.self, forKey: .diffAdded)
        diffDeleted = try c.decode(UInt32.self, forKey: .diffDeleted)
        activeTerminals = (try? c.decodeIfPresent([CodaHerdrTerminalSummary].self, forKey: .activeTerminals)) ?? []
        statusSummary = try c.decode(String.self, forKey: .statusSummary)
    }
}

public struct CodaRepoWorkspaceSummary: Identifiable, Codable, Sendable, Equatable {
    public var id: String { rootPath }
    public let repoName: String
    public let rootPath: String
    public let machineName: String
    public let branches: [CodaBranchWorktreeItem]

    enum CodingKeys: String, CodingKey {
        case repoName = "repo_name"
        case rootPath = "root_path"
        case machineName = "machine_name"
        case branches
    }

    public init(
        repoName: String,
        rootPath: String,
        machineName: String,
        branches: [CodaBranchWorktreeItem]
    ) {
        self.repoName = repoName
        self.rootPath = rootPath
        self.machineName = machineName
        self.branches = branches
    }
}

public struct CodaVoiceModelConfig: Codable, Sendable, Equatable {
    public var provider: String
    public var baseURL: String
    public var apiKey: String
    public var model: String

    enum CodingKeys: String, CodingKey {
        case provider
        case baseURL = "base_url"
        case apiKey = "api_key"
        case model
    }

    public var hasMaskedDaemonKey: Bool {
        let trimmed = apiKey.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed == "••••••••" || trimmed == "********"
    }

    public var hasLocalPlaintextApiKey: Bool {
        let trimmed = apiKey.trimmingCharacters(in: .whitespacesAndNewlines)
        return !trimmed.isEmpty && !hasMaskedDaemonKey
    }

    public init(
        provider: String = "openai",
        baseURL: String = "https://api.openai.com/v1",
        apiKey: String = "",
        model: String = "gpt-4o-mini-transcribe"
    ) {
        self.provider = provider
        self.baseURL = baseURL
        self.apiKey = apiKey
        self.model = model
    }
}

public enum CodaVoiceEndpointValidationError: LocalizedError, Sendable, Equatable {
    case invalidEndpoint(String)

    public var errorDescription: String? {
        switch self {
        case .invalidEndpoint(let msg):
            return msg
        }
    }
}

public enum CodaVoiceEndpointValidator {
    /// Verifies that the current process is a `.app` bundle containing a non-empty TCC usage description
    /// before invoking macOS/iOS TCC APIs (`AVCaptureDevice` / `SFSpeechRecognizer`), preventing `SIGABRT`.
    public static func isBundledWithUsageDescription(
        bundlePathExtension: String,
        usageDescription: String?
    ) -> Bool {
        guard bundlePathExtension.lowercased() == "app" else { return false }
        guard let desc = usageDescription?.trimmingCharacters(in: .whitespacesAndNewlines),
              !desc.isEmpty else {
            return false
        }
        return true
    }

    /// Validates that a voice API endpoint uses HTTPS (or loopback HTTP for local Whisper servers),
    /// rejects empty hosts (`https:///foo`), and inserts `/audio/transcriptions` before any `?query`.
    public static func validatedTranscriptionURL(from rawBaseURL: String) throws -> URL {
        let trimmed = rawBaseURL.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty,
              var comps = URLComponents(string: trimmed),
              let scheme = comps.scheme?.lowercased(),
              let rawHost = comps.host?.lowercased(),
              !rawHost.isEmpty
        else {
            throw CodaVoiceEndpointValidationError.invalidEndpoint("Invalid voice API base URL.")
        }
        if comps.user != nil || comps.password != nil {
            throw CodaVoiceEndpointValidationError.invalidEndpoint("Voice API URL must not embed credentials.")
        }
        let isLoopback = rawHost == "localhost" || rawHost == "127.0.0.1" || rawHost == "::1" || rawHost == "[::1]"
        if scheme != "https" && !(scheme == "http" && isLoopback) {
            throw CodaVoiceEndpointValidationError.invalidEndpoint(
                "Voice API URL must use https:// (or http://localhost for local Whisper)."
            )
        }
        var path = comps.path
        while path.count > 1 && path.hasSuffix("/") {
            path.removeLast()
        }
        if path == "/" {
            path = ""
        }
        if !path.hasSuffix("/audio/transcriptions") {
            path += "/audio/transcriptions"
        }
        comps.path = path
        guard let endpoint = comps.url, let finalHost = endpoint.host, !finalHost.isEmpty else {
            throw CodaVoiceEndpointValidationError.invalidEndpoint("Could not construct /audio/transcriptions URL.")
        }
        return endpoint
    }
}

public struct CodaEndpointCandidate: Codable, Sendable, Equatable, Hashable {
    public let kind: String
    public let addr: String

    public init(kind: String, addr: String) {
        self.kind = kind
        self.addr = addr
    }
}

public struct CodaRemoteDevice: Identifiable, Codable, Sendable, Equatable, Hashable {
    public let id: UUID
    public let name: String
    public let role: String
    public let platform: String
    public let ed25519Pubkey: String
    public let x25519Pubkey: String
    public let endpoints: [CodaEndpointCandidate]
    public let isOnline: Bool

    public var x25519PublicB64: String { x25519Pubkey }

    enum CodingKeys: String, CodingKey {
        case id
        case name
        case role
        case platform
        case ed25519Pubkey = "ed25519_pubkey"
        case x25519Pubkey = "x25519_pubkey"
        case endpointsJson = "endpoints_json"
        case endpoints
        case isOnline = "is_online"
    }

    public init(
        id: UUID,
        name: String,
        role: String,
        platform: String,
        ed25519Pubkey: String = "",
        x25519Pubkey: String,
        endpoints: [CodaEndpointCandidate],
        isOnline: Bool
    ) {
        self.id = id
        self.name = name
        self.role = role
        self.platform = platform
        self.ed25519Pubkey = ed25519Pubkey
        self.x25519Pubkey = x25519Pubkey
        self.endpoints = endpoints
        self.isOnline = isOnline
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(UUID.self, forKey: .id)
        name = try c.decode(String.self, forKey: .name)
        role = try c.decode(String.self, forKey: .role)
        platform = try c.decode(String.self, forKey: .platform)
        ed25519Pubkey = (try? c.decodeIfPresent(String.self, forKey: .ed25519Pubkey)) ?? ""
        x25519Pubkey = try c.decode(String.self, forKey: .x25519Pubkey)
        if let eps = try? c.decodeIfPresent([CodaEndpointCandidate].self, forKey: .endpointsJson) {
            endpoints = eps
        } else if let eps = try? c.decodeIfPresent([CodaEndpointCandidate].self, forKey: .endpoints) {
            endpoints = eps
        } else {
            endpoints = []
        }
        isOnline = try c.decode(Bool.self, forKey: .isOnline)
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(id, forKey: .id)
        try c.encode(name, forKey: .name)
        try c.encode(role, forKey: .role)
        try c.encode(platform, forKey: .platform)
        try c.encode(ed25519Pubkey, forKey: .ed25519Pubkey)
        try c.encode(x25519Pubkey, forKey: .x25519Pubkey)
        try c.encode(endpoints, forKey: .endpointsJson)
        try c.encode(isOnline, forKey: .isOnline)
    }
}

public struct CodaDeviceAuthStart: Codable, Sendable, Equatable {
    public let deviceCode: String
    public let userCode: String
    public let verificationURI: String
    public let verificationURIComplete: String
    public let expiresInSecs: Int
    public let intervalSecs: Int

    enum CodingKeys: String, CodingKey {
        case deviceCode = "device_code"
        case userCode = "user_code"
        case verificationURI = "verification_uri"
        case verificationURIComplete = "verification_uri_complete"
        case expiresInSecs = "expires_in_secs"
        case intervalSecs = "interval_secs"
    }
}

public struct CodaRelayNodeInfo: Codable, Sendable, Equatable {
    public let id: String
    public let region: String
    public let mode: String
    public let endpoint: String
    public let priority: Int
}

public struct CodaDeviceAuthPollResult: Codable, Sendable, Equatable {
    public let state: String
    public let expiresInSecs: Int?
    public let sessionToken: String?
    public let ownerHandle: String?
    public let device: CodaRemoteDevice?
    public let daemonRelayTicket: String?
    public let relays: [CodaRelayNodeInfo]?

    enum CodingKeys: String, CodingKey {
        case state
        case expiresInSecs = "expires_in_secs"
        case sessionToken = "session_token"
        case ownerHandle = "owner_handle"
        case device
        case daemonRelayTicket = "daemon_relay_ticket"
        case relays
    }
}

public struct CodaBootstrapWorkspaceResult: Codable, Sendable, Equatable {
    public let ok: Bool
    public let workspacePath: String
    public let detail: String

    enum CodingKeys: String, CodingKey {
        case ok
        case workspacePath = "workspace_path"
        case detail
    }

    public init(ok: Bool, workspacePath: String, detail: String) {
        self.ok = ok
        self.workspacePath = workspacePath
        self.detail = detail
    }
}

public enum CodaWorkspaceBootstrapValidator {
    public static func isValidFolderName(_ raw: String) -> Bool {
        let name = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty, name.count <= 128 else { return false }
        if name == "." || name == ".." || name.hasPrefix("-") {
            return false
        }
        return !name.unicodeScalars.contains { s in
            s.value < 0x20 || s.value == 0x7F || s == "/" || s == "\\"
        }
    }

    /// Strict client-side RCE guard matching `coda-daemon`: allows ONLY `https://`, `ssh://`, or `git@<host>:<path>`.
    public static func isAllowedGitCloneURL(_ raw: String) -> Bool {
        let urlStr = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        if urlStr.isEmpty { return true }
        guard urlStr.count <= 1024, !urlStr.hasPrefix("-") else { return false }
        if urlStr.unicodeScalars.contains(where: { $0.value <= 0x20 || $0.value == 0x7F }) {
            return false
        }
        if urlStr.hasPrefix("https://") || urlStr.hasPrefix("ssh://") {
            guard let parsed = URL(string: urlStr),
                  let host = parsed.host,
                  !host.isEmpty,
                  !host.hasPrefix("-"),
                  !parsed.path.isEmpty,
                  parsed.path != "/" else {
                return false
            }
            return true
        }
        if urlStr.hasPrefix("git@") {
            let afterAt = String(urlStr.dropFirst(4))
            guard let colon = afterAt.firstIndex(of: ":") else { return false }
            let host = String(afterAt[..<colon])
            let repoPath = String(afterAt[afterAt.index(after: colon)...])
            guard !host.isEmpty, !host.hasPrefix("-"),
                  !repoPath.isEmpty, !repoPath.hasPrefix("-"),
                  !repoPath.contains("..") else {
                return false
            }
            let hostValid = host.unicodeScalars.allSatisfy { s in
                (s.value >= 0x30 && s.value <= 0x39)
                    || (s.value >= 0x41 && s.value <= 0x5A)
                    || (s.value >= 0x61 && s.value <= 0x7A)
                    || s == "." || s == "-"
            }
            return hostValid
        }
        return false
    }

    public static func suggestedFolderName(fromGitCloneURL raw: String) -> String? {
        var s = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        while s.hasSuffix("/") { s.removeLast() }
        if s.hasSuffix(".git") {
            s = String(s.dropLast(4))
        }
        let lastSeg: String
        if let slash = s.lastIndex(of: "/") {
            lastSeg = String(s[s.index(after: slash)...])
        } else if let colon = s.lastIndex(of: ":") {
            lastSeg = String(s[s.index(after: colon)...])
        } else {
            return nil
        }
        return isValidFolderName(lastSeg) ? lastSeg : nil
    }
}

public struct CodaNotificationWebhookSettings: Codable, Sendable, Equatable {
    public var barkURL: String
    public var telegramBotToken: String
    public var telegramChatId: String
    public var feishuWebhookURL: String
    public var discordWebhookURL: String
    public var genericWebhookURL: String
    public var notifyOnNeedsYou: Bool
    public var notifyOnFinished: Bool

    enum CodingKeys: String, CodingKey {
        case barkURL = "bark_url"
        case telegramBotToken = "telegram_bot_token"
        case telegramChatId = "telegram_chat_id"
        case feishuWebhookURL = "feishu_webhook_url"
        case discordWebhookURL = "discord_webhook_url"
        case genericWebhookURL = "generic_webhook_url"
        case notifyOnNeedsYou = "notify_on_needs_you"
        case notifyOnFinished = "notify_on_finished"
    }

    public init(
        barkURL: String = "",
        telegramBotToken: String = "",
        telegramChatId: String = "",
        feishuWebhookURL: String = "",
        discordWebhookURL: String = "",
        genericWebhookURL: String = "",
        notifyOnNeedsYou: Bool = true,
        notifyOnFinished: Bool = true
    ) {
        self.barkURL = barkURL
        self.telegramBotToken = telegramBotToken
        self.telegramChatId = telegramChatId
        self.feishuWebhookURL = feishuWebhookURL
        self.discordWebhookURL = discordWebhookURL
        self.genericWebhookURL = genericWebhookURL
        self.notifyOnNeedsYou = notifyOnNeedsYou
        self.notifyOnFinished = notifyOnFinished
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        barkURL = (try? c.decodeIfPresent(String.self, forKey: .barkURL)) ?? ""
        telegramBotToken = (try? c.decodeIfPresent(String.self, forKey: .telegramBotToken)) ?? ""
        telegramChatId = (try? c.decodeIfPresent(String.self, forKey: .telegramChatId)) ?? ""
        feishuWebhookURL = (try? c.decodeIfPresent(String.self, forKey: .feishuWebhookURL)) ?? ""
        discordWebhookURL = (try? c.decodeIfPresent(String.self, forKey: .discordWebhookURL)) ?? ""
        genericWebhookURL = (try? c.decodeIfPresent(String.self, forKey: .genericWebhookURL)) ?? ""
        notifyOnNeedsYou = (try? c.decodeIfPresent(Bool.self, forKey: .notifyOnNeedsYou)) ?? true
        notifyOnFinished = (try? c.decodeIfPresent(Bool.self, forKey: .notifyOnFinished)) ?? true
    }

    public func validateWebhookURLs(allowLoopbackHTTP: Bool = false) throws {
        let fields: [(String, String)] = [
            ("Bark URL", barkURL),
            ("Feishu Webhook URL", feishuWebhookURL),
            ("Discord Webhook URL", discordWebhookURL),
            ("Generic Webhook URL", genericWebhookURL),
        ]
        for (label, raw) in fields {
            let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !trimmed.isEmpty else { continue }
            guard let url = URL(string: trimmed),
                  let scheme = url.scheme?.lowercased(),
                  let host = url.host?.lowercased(),
                  !host.isEmpty else {
                throw CodaAPIError.httpError(statusCode: 400, message: "\(label) is not a valid URL.")
            }
            let isLoopback = host == "localhost" || host == "127.0.0.1" || host == "::1" || host == "[::1]"
            let isPrivateOrMeta = isLoopback
                || host == "0.0.0.0"
                || host.hasPrefix("169.254.")
                || host.hasPrefix("10.")
                || host.hasPrefix("192.168.")
                || host.hasSuffix(".internal")
            if allowLoopbackHTTP && isLoopback && (scheme == "http" || scheme == "https") {
                continue
            }
            guard scheme == "https" && !isPrivateOrMeta else {
                throw CodaAPIError.httpError(
                    statusCode: 400,
                    message: "\(label) must use a public https:// endpoint."
                )
            }
        }
    }
}

public struct CodaNotificationTestResult: Codable, Sendable, Equatable {
    public let ok: Bool
    public let deliveredChannels: [String]
    public let errors: [String]

    enum CodingKeys: String, CodingKey {
        case ok
        case deliveredChannels = "delivered_channels"
        case errors
    }

    public init(ok: Bool, deliveredChannels: [String] = [], errors: [String] = []) {
        self.ok = ok
        self.deliveredChannels = deliveredChannels
        self.errors = errors
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        ok = (try? c.decodeIfPresent(Bool.self, forKey: .ok)) ?? false
        deliveredChannels = (try? c.decodeIfPresent([String].self, forKey: .deliveredChannels)) ?? []
        errors = (try? c.decodeIfPresent([String].self, forKey: .errors)) ?? []
    }
}

public struct CodaPruneStaleDevicesResult: Codable, Sendable, Equatable {
    public let ok: Bool
    public let prunedCount: Int

    enum CodingKeys: String, CodingKey {
        case ok
        case prunedCount = "pruned_count"
    }

    public init(ok: Bool, prunedCount: Int) {
        self.ok = ok
        self.prunedCount = prunedCount
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        ok = (try? c.decodeIfPresent(Bool.self, forKey: .ok)) ?? false
        prunedCount = (try? c.decodeIfPresent(Int.self, forKey: .prunedCount)) ?? 0
    }
}

// MARK: - Cross-Device Prompt Snippets, Friend Onboarding & Admin Member Models (v1.1.0)

public struct CodaPromptSnippet: Codable, Identifiable, Equatable, Sendable {
    public static let maxSnippetsCount = 24
    public static let maxTitleLength = 60
    public static let maxPromptLength = 4000

    public var id: String
    public var title: String
    public var prompt: String

    public init(
        id: String = "snip-\(UUID().uuidString.lowercased().prefix(8))",
        title: String,
        prompt: String
    ) {
        self.id = id
        self.title = title
        self.prompt = prompt
    }

    public static let defaultStarterSnippets: [CodaPromptSnippet] = [
        CodaPromptSnippet(
            id: "s1",
            title: "Run Tests & Fix",
            prompt: "Run the full test suite, fix any failures or compiler warnings, and summarize what changed."
        ),
        CodaPromptSnippet(
            id: "s2",
            title: "Review Git Diff",
            prompt: "Review the current git diff carefully for bugs, edge cases, or regressions before committing."
        ),
        CodaPromptSnippet(
            id: "s3",
            title: "Continue",
            prompt: "Continue with the next steps until everything is complete and verified."
        ),
        CodaPromptSnippet(
            id: "s4",
            title: "Clean Commit",
            prompt: "Stage and commit the current changes with a concise conventional commit message."
        ),
    ]

    public static func derivedTitle(fromPrompt rawPrompt: String) -> String {
        let firstNonEmpty = rawPrompt
            .components(separatedBy: .newlines)
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .first(where: { !$0.isEmpty }) ?? ""
        guard !firstNonEmpty.isEmpty else { return "Snippet" }
        let collapsed = firstNonEmpty
            .components(separatedBy: .whitespaces)
            .filter { !$0.isEmpty }
            .joined(separator: " ")
        if collapsed.count <= 34 {
            return collapsed
        }
        return String(collapsed.prefix(33)) + "…"
    }

    private static func truncateScalars(_ text: String, maxScalars: Int) -> String {
        guard text.unicodeScalars.count > maxScalars else { return text }
        return String(String.UnicodeScalarView(text.unicodeScalars.prefix(maxScalars)))
    }

    private static func stripControlScalars(_ text: String, replaceWhitespaceWithSpace: Bool) -> String {
        var out = String.UnicodeScalarView()
        out.reserveCapacity(text.unicodeScalars.count)
        for sc in text.unicodeScalars {
            let v = sc.value
            let isCtrl = v < 0x20 || (v >= 0x7F && v <= 0x9F)
            if isCtrl {
                if replaceWhitespaceWithSpace && (v == 0x09 || v == 0x0A || v == 0x0D) {
                    out.append(" ")
                }
            } else {
                out.append(sc)
            }
        }
        return String(out)
    }

    public func sanitized() -> CodaPromptSnippet? {
        let cleanPrompt = prompt
            .replacingOccurrences(of: "\r\n", with: "\n")
            .replacingOccurrences(of: "\r", with: "\n")
            .replacingOccurrences(of: "\0", with: "")
            .trimmingCharacters(in: .whitespacesAndNewlines)
        guard !cleanPrompt.isEmpty else { return nil }

        let rawId = Self.stripControlScalars(id, replaceWhitespaceWithSpace: false)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let cleanId = rawId.isEmpty
            ? "snip-\(UUID().uuidString.lowercased().prefix(8))"
            : Self.truncateScalars(rawId, maxScalars: 64)

        let rawTitle = Self.stripControlScalars(title, replaceWhitespaceWithSpace: true)
            .components(separatedBy: .whitespaces)
            .filter { !$0.isEmpty }
            .joined(separator: " ")
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let resolvedTitle = rawTitle.isEmpty ? Self.derivedTitle(fromPrompt: cleanPrompt) : rawTitle
        let boundedTitle = Self.truncateScalars(resolvedTitle, maxScalars: Self.maxTitleLength)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let boundedPrompt = Self.truncateScalars(cleanPrompt, maxScalars: Self.maxPromptLength)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        guard !boundedTitle.isEmpty, !boundedPrompt.isEmpty else { return nil }
        return CodaPromptSnippet(id: cleanId, title: boundedTitle, prompt: boundedPrompt)
    }

    public static func sanitizeList(_ snippets: [CodaPromptSnippet]) -> [CodaPromptSnippet] {
        var out: [CodaPromptSnippet] = []
        var seenIds = Set<String>()
        for item in snippets {
            guard let clean = item.sanitized() else { continue }
            var finalItem = clean
            if !seenIds.insert(finalItem.id).inserted {
                finalItem.id = "snip-\(UUID().uuidString.lowercased().prefix(8))"
                seenIds.insert(finalItem.id)
            }
            out.append(finalItem)
            if out.count >= maxSnippetsCount {
                break
            }
        }
        return out
    }
}

public struct CodaPromptSnippetsEnvelope: Codable, Equatable, Sendable {
    public let snippets: [CodaPromptSnippet]

    public init(snippets: [CodaPromptSnippet]) {
        self.snippets = snippets
    }
}

public struct CodaAuthUserProfile: Codable, Identifiable, Equatable, Sendable {
    public let id: UUID
    public let email: String
    public let handle: String
    public let displayName: String
    public let isAdmin: Bool
    public let bandwidthQuotaMbps: Int
    public let createdAt: String?

    enum CodingKeys: String, CodingKey {
        case id
        case email
        case handle
        case displayName = "display_name"
        case isAdmin = "is_admin"
        case bandwidthQuotaMbps = "bandwidth_quota_mbps"
        case createdAt = "created_at"
    }

    public init(
        id: UUID,
        email: String,
        handle: String,
        displayName: String = "",
        isAdmin: Bool = false,
        bandwidthQuotaMbps: Int = 25,
        createdAt: String? = nil
    ) {
        self.id = id
        self.email = email
        self.handle = handle
        self.displayName = displayName
        self.isAdmin = isAdmin
        self.bandwidthQuotaMbps = bandwidthQuotaMbps
        self.createdAt = createdAt
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(UUID.self, forKey: .id)
        email = (try? c.decodeIfPresent(String.self, forKey: .email)) ?? ""
        handle = try c.decode(String.self, forKey: .handle)
        displayName = (try? c.decodeIfPresent(String.self, forKey: .displayName)) ?? ""
        isAdmin = (try? c.decodeIfPresent(Bool.self, forKey: .isAdmin)) ?? false
        bandwidthQuotaMbps = (try? c.decodeIfPresent(Int.self, forKey: .bandwidthQuotaMbps)) ?? 25
        createdAt = try? c.decodeIfPresent(String.self, forKey: .createdAt)
    }
}

public struct CodaInviteCodeItem: Codable, Identifiable, Equatable, Sendable {
    public var id: String { code }
    public let code: String
    public let note: String
    public let maxUses: Int
    public let usedCount: Int
    public let expiresAt: String?
    public let createdAt: String?

    enum CodingKeys: String, CodingKey {
        case code
        case note
        case maxUses = "max_uses"
        case usedCount = "used_count"
        case expiresAt = "expires_at"
        case createdAt = "created_at"
    }

    public init(
        code: String,
        note: String = "",
        maxUses: Int = 5,
        usedCount: Int = 0,
        expiresAt: String? = nil,
        createdAt: String? = nil
    ) {
        self.code = code
        self.note = note
        self.maxUses = maxUses
        self.usedCount = usedCount
        self.expiresAt = expiresAt
        self.createdAt = createdAt
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        code = try c.decode(String.self, forKey: .code)
        note = (try? c.decodeIfPresent(String.self, forKey: .note)) ?? ""
        maxUses = (try? c.decodeIfPresent(Int.self, forKey: .maxUses)) ?? 1
        usedCount = (try? c.decodeIfPresent(Int.self, forKey: .usedCount)) ?? 0
        expiresAt = try? c.decodeIfPresent(String.self, forKey: .expiresAt)
        createdAt = try? c.decodeIfPresent(String.self, forKey: .createdAt)
    }
}

public struct CodaAdminUserSummary: Codable, Identifiable, Equatable, Sendable {
    public let id: UUID
    public let handle: String
    public let email: String
    public let displayName: String
    public let isAdmin: Bool
    public let onlineDevices: Int
    public let totalDevices: Int
    public let lastLoginAt: String?
    public let createdAt: String?

    enum CodingKeys: String, CodingKey {
        case id
        case handle
        case email
        case displayName = "display_name"
        case isAdmin = "is_admin"
        case onlineDevices = "online_devices"
        case totalDevices = "total_devices"
        case lastLoginAt = "last_login_at"
        case createdAt = "created_at"
    }

    public init(
        id: UUID,
        handle: String,
        email: String = "",
        displayName: String = "",
        isAdmin: Bool = false,
        onlineDevices: Int = 0,
        totalDevices: Int = 0,
        lastLoginAt: String? = nil,
        createdAt: String? = nil
    ) {
        self.id = id
        self.handle = handle
        self.email = email
        self.displayName = displayName
        self.isAdmin = isAdmin
        self.onlineDevices = onlineDevices
        self.totalDevices = totalDevices
        self.lastLoginAt = lastLoginAt
        self.createdAt = createdAt
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(UUID.self, forKey: .id)
        handle = try c.decode(String.self, forKey: .handle)
        email = (try? c.decodeIfPresent(String.self, forKey: .email)) ?? ""
        displayName = (try? c.decodeIfPresent(String.self, forKey: .displayName)) ?? ""
        isAdmin = (try? c.decodeIfPresent(Bool.self, forKey: .isAdmin)) ?? false
        onlineDevices = (try? c.decodeIfPresent(Int.self, forKey: .onlineDevices)) ?? 0
        totalDevices = (try? c.decodeIfPresent(Int.self, forKey: .totalDevices)) ?? 0
        lastLoginAt = try? c.decodeIfPresent(String.self, forKey: .lastLoginAt)
        createdAt = try? c.decodeIfPresent(String.self, forKey: .createdAt)
    }
}

public struct CodaAdminUsersEnvelope: Codable, Equatable, Sendable {
    public let users: [CodaAdminUserSummary]

    public init(users: [CodaAdminUserSummary]) {
        self.users = users
    }
}

public struct CodaPasswordResetLinkResult: Codable, Equatable, Sendable {
    public let ok: Bool
    public let userId: UUID
    public let handle: String
    public let resetToken: String
    public let resetUrl: String
    public let expiresInSecs: Int

    public var resetURL: String { resetUrl }

    enum CodingKeys: String, CodingKey {
        case ok
        case userId = "user_id"
        case handle
        case resetToken = "reset_token"
        case resetUrl = "reset_url"
        case expiresInSecs = "expires_in_secs"
    }

    public init(
        ok: Bool = true,
        userId: UUID,
        handle: String,
        resetToken: String,
        resetUrl: String,
        expiresInSecs: Int = 86400
    ) {
        self.ok = ok
        self.userId = userId
        self.handle = handle
        self.resetToken = resetToken
        self.resetUrl = resetUrl
        self.expiresInSecs = expiresInSecs
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        ok = (try? c.decodeIfPresent(Bool.self, forKey: .ok)) ?? true
        userId = try c.decode(UUID.self, forKey: .userId)
        handle = (try? c.decodeIfPresent(String.self, forKey: .handle)) ?? ""
        resetToken = try c.decode(String.self, forKey: .resetToken)
        resetUrl = try c.decode(String.self, forKey: .resetUrl)
        expiresInSecs = (try? c.decodeIfPresent(Int.self, forKey: .expiresInSecs)) ?? 86400
    }
}

public struct CodaPasswordResetConfirmResult: Codable, Equatable, Sendable {
    public let ok: Bool
    public let handle: String

    public init(ok: Bool = true, handle: String = "") {
        self.ok = ok
        self.handle = handle
    }

    enum CodingKeys: String, CodingKey {
        case ok
        case handle
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        ok = (try? c.decodeIfPresent(Bool.self, forKey: .ok)) ?? true
        handle = (try? c.decodeIfPresent(String.self, forKey: .handle)) ?? ""
    }
}

private func isURLSafeResetTokenChar(_ scalar: Unicode.Scalar) -> Bool {
    let v = scalar.value
    return (v >= 0x30 && v <= 0x39) // 0-9
        || (v >= 0x41 && v <= 0x5A) // A-Z
        || (v >= 0x61 && v <= 0x7A) // a-z
        || v == 0x5F || v == 0x2D   // _ -
}

private func isValidBareResetToken(_ raw: String) -> Bool {
    guard raw.hasPrefix("rst_"), raw.count <= 256 else { return false }
    let suffix = raw.dropFirst(4)
    guard !suffix.isEmpty else { return false }
    return suffix.unicodeScalars.allSatisfy(isURLSafeResetTokenChar)
}

/// Extracts a clean `rst_...` password reset token from either a full
/// `https://.../reset-password?token=rst_...` URL (including WeChat message text around a URL)
/// or a raw `rst_...` token string. Returns `nil` when no valid `rst_...` token is present.
public func extractResetTokenFromInput(_ raw: String) -> String? {
    let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !trimmed.isEmpty else { return nil }

    if isValidBareResetToken(trimmed) {
        return trimmed
    }

    if let comps = URLComponents(string: trimmed) {
        if let items = comps.queryItems,
           let tokenItem = items.first(where: { $0.name.lowercased() == "token" }),
           let val = tokenItem.value?.trimmingCharacters(in: .whitespacesAndNewlines),
           isValidBareResetToken(val) {
            return val
        }
        if let frag = comps.fragment, frag.contains("?"),
           let fragComps = URLComponents(string: "https://codahub.local/" + frag),
           let items = fragComps.queryItems,
           let tokenItem = items.first(where: { $0.name.lowercased() == "token" }),
           let val = tokenItem.value?.trimmingCharacters(in: .whitespacesAndNewlines),
           isValidBareResetToken(val) {
            return val
        }
    }

    let decoded = trimmed.removingPercentEncoding ?? trimmed
    var searchStart = decoded.startIndex
    while let range = decoded.range(of: "rst_", range: searchStart..<decoded.endIndex) {
        let precededByWordChar: Bool
        if range.lowerBound > decoded.startIndex {
            let prevIdx = decoded.index(before: range.lowerBound)
            let prevScalar = decoded[prevIdx].unicodeScalars.first
            precededByWordChar = prevScalar.map(isURLSafeResetTokenChar) ?? false
        } else {
            precededByWordChar = false
        }

        var cursor = range.upperBound
        while cursor < decoded.endIndex,
              let sc = decoded[cursor].unicodeScalars.first,
              decoded[cursor].unicodeScalars.count == 1,
              isURLSafeResetTokenChar(sc) {
            cursor = decoded.index(after: cursor)
        }
        let candidate = String(decoded[range.lowerBound..<cursor])
        if !precededByWordChar && isValidBareResetToken(candidate) {
            return candidate
        }
        searchStart = cursor > range.lowerBound ? cursor : decoded.index(after: range.lowerBound)
    }
    return nil
}

/// Builds a shareable friend registration link (`https://<server>/register?invite=CODA-XXXXXX`)
/// from a base server URL and invite code. Returns `nil` if `serverURL` or `code` is invalid.
public func buildInviteRegistrationURL(serverURL: String, code: String) -> String? {
    let cleanServer = serverURL.trimmingCharacters(in: .whitespacesAndNewlines)
    let cleanCode = code.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !cleanServer.isEmpty,
          !cleanCode.isEmpty,
          var comps = URLComponents(string: cleanServer),
          let scheme = comps.scheme?.lowercased(),
          (scheme == "https" || scheme == "http"),
          let host = comps.host,
          !host.isEmpty,
          comps.user == nil,
          comps.password == nil
    else {
        return nil
    }
    var basePath = comps.path
    while basePath.hasSuffix("/") {
        basePath.removeLast()
    }
    comps.path = basePath + "/register"
    comps.queryItems = [URLQueryItem(name: "invite", value: cleanCode)]
    comps.fragment = nil
    return comps.url?.absoluteString
}

/// Extracts an invite code from either a pasted `https://.../register?invite=CODA-XXXXXX` URL or raw code.
public func extractInviteCodeFromInput(_ raw: String) -> String {
    let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !trimmed.isEmpty else { return "" }
    if let comps = URLComponents(string: trimmed),
       let items = comps.queryItems,
       let inv = items.first(where: { $0.name.lowercased() == "invite" })?.value?.trimmingCharacters(in: .whitespacesAndNewlines),
       !inv.isEmpty {
        return inv
    }
    return trimmed
}

/// Extracts a safe `https://host[:port]` (or loopback `http://localhost[:port]`) server origin from a pasted reset/invite URL.
public func extractServerOriginFromAuthLink(_ raw: String) -> String? {
    let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
    guard let comps = URLComponents(string: trimmed),
          let scheme = comps.scheme?.lowercased(),
          let host = comps.host?.lowercased(),
          !host.isEmpty,
          comps.user == nil,
          comps.password == nil
    else {
        return nil
    }
    let isLoopback = host == "localhost" || host == "127.0.0.1" || host == "::1" || host == "[::1]"
    guard scheme == "https" || (scheme == "http" && isLoopback) else {
        return nil
    }
    var origin = URLComponents()
    origin.scheme = scheme
    origin.host = comps.host
    origin.port = comps.port
    return origin.url?.absoluteString
}

/// Builds a shell-safe 1-line host enrollment command (`curl -fsSL <serverURL>/install.sh | sh`).
public func buildOneLineInstallCommand(serverURL: String) -> String {
    var trimmed = serverURL.trimmingCharacters(in: .whitespacesAndNewlines)
    while trimmed.hasSuffix("/") && trimmed.count > "https://".count {
        trimmed.removeLast()
    }
    let unsafeChars = CharacterSet(charactersIn: " \t\r\n'\"`$;&|<>\\")
    if let comps = URLComponents(string: trimmed),
       let scheme = comps.scheme?.lowercased(),
       (scheme == "https" || scheme == "http"),
       let host = comps.host,
       !host.isEmpty,
       comps.user == nil,
       comps.password == nil,
       trimmed.rangeOfCharacter(from: unsafeChars) == nil {
        return "curl -fsSL \(trimmed)/install.sh | sh"
    }
    return "curl -fsSL https://codahub.dev/install.sh | sh"
}

// MARK: - Responsive Apple Device Form Factors, Fold Postures & Safe Area Specs

public enum CodaFoldPosture: String, Codable, Sendable, CaseIterable, Equatable {
    case flat
    case bookHalfOpen
    case tabletopLaptop
}

public struct CodaReservedRegion: Sendable, Equatable {
    public enum Kind: String, Codable, Sendable, Equatable {
        case division
        case occlusion
    }

    public let kind: Kind
    public let rect: CGRect

    public init(kind: Kind, rect: CGRect) {
        self.kind = kind
        self.rect = rect
    }

    public static func duoFoldDivision(
        in size: CGSize,
        posture: CodaFoldPosture,
        thickness: CGFloat = 24
    ) -> CodaReservedRegion? {
        guard size.width > 0 && size.height > 0 else { return nil }
        if size.width > size.height {
            let x = max(0, (size.width - thickness) * 0.5)
            return CodaReservedRegion(
                kind: .division,
                rect: CGRect(x: x, y: 0, width: thickness, height: size.height)
            )
        }
        if posture == .tabletopLaptop {
            let y = max(0, (size.height - thickness) * 0.5)
            return CodaReservedRegion(
                kind: .division,
                rect: CGRect(x: 0, y: y, width: size.width, height: thickness)
            )
        }
        return nil
    }

    public static func duoRightSystemRail(
        in size: CGSize,
        railWidth: CGFloat = CodaEdgeSafeAreas.duoSideRailWidth
    ) -> CodaReservedRegion? {
        guard size.width > 0 && size.height > 0 else { return nil }
        let w = min(railWidth, size.width)
        return CodaReservedRegion(
            kind: .occlusion,
            rect: CGRect(x: max(0, size.width - w), y: 0, width: w, height: size.height)
        )
    }
}

public struct CodaEdgeSafeAreas: Sendable, Equatable {
    public var top: CGFloat
    public var leading: CGFloat
    public var bottom: CGFloat
    public var trailing: CGFloat

    public static let zero = CodaEdgeSafeAreas(top: 0, leading: 0, bottom: 0, trailing: 0)
    public static let duoSideRailWidth: CGFloat = 84

    public init(top: CGFloat, leading: CGFloat, bottom: CGFloat, trailing: CGFloat) {
        self.top = top
        self.leading = leading
        self.bottom = bottom
        self.trailing = trailing
    }

    public var isHorizontallyAsymmetric: Bool {
        abs(leading - trailing) > 1.0
    }
}

public enum CodaFormFactor: String, Codable, Sendable, CaseIterable, Equatable {
    case iphonePortrait
    case iphoneLandscape
    case iphoneDuoCoverPortrait
    case iphoneDuoCoverLandscape
    case iphoneDuoInnerLandscape
    case iphoneDuoInnerPortrait
    case ipadOrDesktop

    public var isCompact: Bool {
        switch self {
        case .iphonePortrait, .iphoneLandscape, .iphoneDuoCoverPortrait, .iphoneDuoCoverLandscape:
            return true
        case .iphoneDuoInnerLandscape, .iphoneDuoInnerPortrait, .ipadOrDesktop:
            return false
        }
    }

    public var isIPhoneDuo: Bool {
        switch self {
        case .iphoneDuoCoverPortrait, .iphoneDuoCoverLandscape, .iphoneDuoInnerLandscape, .iphoneDuoInnerPortrait:
            return true
        case .iphonePortrait, .iphoneLandscape, .ipadOrDesktop:
            return false
        }
    }

    public var usesVerticalSideRail: Bool {
        switch self {
        case .iphoneLandscape, .iphoneDuoCoverPortrait, .iphoneDuoCoverLandscape, .iphoneDuoInnerLandscape, .iphoneDuoInnerPortrait:
            return true
        case .iphonePortrait, .ipadOrDesktop:
            return false
        }
    }

    public var placesSideRailOnTrailingEdge: Bool {
        isIPhoneDuo
    }

    public func fluidHorizontalMargin(for width: CGFloat) -> CGFloat {
        if width <= 395 { return 14 }
        if width <= 425 { return 16 }
        if width <= 448 { return 18 }
        return 20
    }

    public var platformIdentifier: String {
        #if os(macOS)
        return platformIdentifier(forMacOS: true)
        #else
        return platformIdentifier(forMacOS: false)
        #endif
    }

    public func platformIdentifier(forMacOS: Bool) -> String {
        if forMacOS {
            return "macos"
        }
        if isIPhoneDuo {
            return "iphone_duo"
        }
        switch self {
        case .iphonePortrait, .iphoneLandscape:
            return "ios"
        case .iphoneDuoCoverPortrait, .iphoneDuoCoverLandscape, .iphoneDuoInnerLandscape, .iphoneDuoInnerPortrait:
            return "iphone_duo"
        case .ipadOrDesktop:
            return "ipados"
        }
    }

    public static func detect(
        width: CGFloat,
        height: CGFloat,
        isHorizontalRegular: Bool? = nil,
        isVerticalRegular: Bool? = nil,
        hardwareScreenSize: CGSize? = nil,
        isPadOrMacHardware: Bool = false,
        hasAsymmetricDuoRail: Bool? = nil
    ) -> CodaFormFactor {
        let w = max(1, width)
        let h = max(1, height)
        let minDim = min(w, h)
        let maxDim = max(w, h)
        let aspect = maxDim / minDim

        // iPadOS (Split View / Stage Manager / Display Zoom) & macOS windows must NEVER classify as foldable Duo.
        if isPadOrMacHardware {
            if minDim < 500 && isHorizontalRegular == false {
                return .iphonePortrait
            }
            return .ipadOrDesktop
        }

        if let hw = hardwareScreenSize, hw.width > 0, hw.height > 0 {
            let hwMin = min(hw.width, hw.height)
            let hwMax = max(hw.width, hw.height)
            // Full hardware screen >= iPad mini (744x1133) or Desktop -> never foldable Duo
            if hwMin >= 720 || hwMax >= 1000 {
                if minDim < 500 && isHorizontalRegular == false {
                    return .iphonePortrait
                }
                return .ipadOrDesktop
            }
            // If physical screen is Duo Cover (~466x678) and software keyboard shrank viewport height,
            // preserve Duo Cover orientation from full hardware screen dimensions instead of flipping!
            if hwMin >= 449 && hwMin <= 540 && hwMax >= 600 && hwMax <= 755 && (hwMax / hwMin) < 1.70
                && hasAsymmetricDuoRail != false
            {
                return hw.width > hw.height ? .iphoneDuoCoverLandscape : .iphoneDuoCoverPortrait
            }
        }

        // Mainstream slab iPhones (iPhone 14 .. 18 Pro Max, 375..440pt short edge, ~2.16 aspect)
        if (minDim <= 448 && aspect >= 1.70) || (isVerticalRegular == false && minDim <= 448 && maxDim >= 650) {
            return w > h ? .iphoneLandscape : .iphonePortrait
        }

        if hasAsymmetricDuoRail != false {
            // iPhone Duo 5.4" Cover Display (466x678 Portrait, 678x466 Landscape, ~1.455 aspect)
            if minDim >= 449 && minDim <= 540 && maxDim >= 600 && maxDim <= 755 && aspect < 1.70
                && (isHorizontalRegular != true || isVerticalRegular == false)
            {
                return w > h ? .iphoneDuoCoverLandscape : .iphoneDuoCoverPortrait
            }

            // iPhone Duo 7.6" Unfolded Inner Display (exact 626x890 or Display-Zoomed 669x951, ~1.4216 aspect).
            // Tightly bounded so iPad Stage Manager / Split View (620x834, 900x640) & Zoomed iPad mini (640x960)
            // classify cleanly as `.ipadOrDesktop`!
            let isStandardDuoInner = abs(minDim - 626) <= 12 && abs(maxDim - 890) <= 16
            let isZoomedDuoInner = abs(minDim - 669) <= 12 && abs(maxDim - 951) <= 16
            if isStandardDuoInner || isZoomedDuoInner {
                return w > h ? .iphoneDuoInnerLandscape : .iphoneDuoInnerPortrait
            }
        }

        if minDim < 540 {
            return w > h ? .iphoneLandscape : .iphonePortrait
        }

        return .ipadOrDesktop
    }
}

public struct CodaIPhoneDeviceSpec: Sendable, Equatable, Identifiable {
    public var id: String { name }
    public let name: String
    public let generation: String
    public let logicalSize: CGSize
    public let scaleFactor: Int
    public let portraitSafeArea: CodaEdgeSafeAreas
    public let landscapeSafeArea: CodaEdgeSafeAreas
    public let isHorizontalRegularInPortrait: Bool?
    public let isVerticalRegularInPortrait: Bool?
    public let isHorizontalRegularInLandscape: Bool?
    public let isVerticalRegularInLandscape: Bool?
    public let expectedPortraitFormFactor: CodaFormFactor
    public let expectedLandscapeFormFactor: CodaFormFactor

    public init(
        name: String,
        generation: String,
        logicalSize: CGSize,
        scaleFactor: Int,
        portraitSafeArea: CodaEdgeSafeAreas,
        landscapeSafeArea: CodaEdgeSafeAreas,
        isHorizontalRegularInPortrait: Bool? = false,
        isVerticalRegularInPortrait: Bool? = true,
        isHorizontalRegularInLandscape: Bool? = false,
        isVerticalRegularInLandscape: Bool? = false,
        expectedPortraitFormFactor: CodaFormFactor,
        expectedLandscapeFormFactor: CodaFormFactor
    ) {
        self.name = name
        self.generation = generation
        self.logicalSize = logicalSize
        self.scaleFactor = scaleFactor
        self.portraitSafeArea = portraitSafeArea
        self.landscapeSafeArea = landscapeSafeArea
        self.isHorizontalRegularInPortrait = isHorizontalRegularInPortrait
        self.isVerticalRegularInPortrait = isVerticalRegularInPortrait
        self.isHorizontalRegularInLandscape = isHorizontalRegularInLandscape
        self.isVerticalRegularInLandscape = isVerticalRegularInLandscape
        self.expectedPortraitFormFactor = expectedPortraitFormFactor
        self.expectedLandscapeFormFactor = expectedLandscapeFormFactor
    }

    public static let mainstreamCatalog: [CodaIPhoneDeviceSpec] = [
        // iPhone 14 Series
        CodaIPhoneDeviceSpec(
            name: "iPhone 14",
            generation: "14",
            logicalSize: CGSize(width: 390, height: 844),
            scaleFactor: 3,
            portraitSafeArea: CodaEdgeSafeAreas(top: 47, leading: 0, bottom: 34, trailing: 0),
            landscapeSafeArea: CodaEdgeSafeAreas(top: 0, leading: 47, bottom: 21, trailing: 47),
            expectedPortraitFormFactor: .iphonePortrait,
            expectedLandscapeFormFactor: .iphoneLandscape
        ),
        CodaIPhoneDeviceSpec(
            name: "iPhone 14 Plus",
            generation: "14",
            logicalSize: CGSize(width: 428, height: 926),
            scaleFactor: 3,
            portraitSafeArea: CodaEdgeSafeAreas(top: 47, leading: 0, bottom: 34, trailing: 0),
            landscapeSafeArea: CodaEdgeSafeAreas(top: 0, leading: 47, bottom: 21, trailing: 47),
            isHorizontalRegularInLandscape: true,
            expectedPortraitFormFactor: .iphonePortrait,
            expectedLandscapeFormFactor: .iphoneLandscape
        ),
        CodaIPhoneDeviceSpec(
            name: "iPhone 14 Pro",
            generation: "14",
            logicalSize: CGSize(width: 393, height: 852),
            scaleFactor: 3,
            portraitSafeArea: CodaEdgeSafeAreas(top: 59, leading: 0, bottom: 34, trailing: 0),
            landscapeSafeArea: CodaEdgeSafeAreas(top: 0, leading: 59, bottom: 21, trailing: 59),
            expectedPortraitFormFactor: .iphonePortrait,
            expectedLandscapeFormFactor: .iphoneLandscape
        ),
        CodaIPhoneDeviceSpec(
            name: "iPhone 14 Pro Max",
            generation: "14",
            logicalSize: CGSize(width: 430, height: 932),
            scaleFactor: 3,
            portraitSafeArea: CodaEdgeSafeAreas(top: 59, leading: 0, bottom: 34, trailing: 0),
            landscapeSafeArea: CodaEdgeSafeAreas(top: 0, leading: 59, bottom: 21, trailing: 59),
            isHorizontalRegularInLandscape: true,
            expectedPortraitFormFactor: .iphonePortrait,
            expectedLandscapeFormFactor: .iphoneLandscape
        ),
        // iPhone 15 Series
        CodaIPhoneDeviceSpec(
            name: "iPhone 15",
            generation: "15",
            logicalSize: CGSize(width: 393, height: 852),
            scaleFactor: 3,
            portraitSafeArea: CodaEdgeSafeAreas(top: 59, leading: 0, bottom: 34, trailing: 0),
            landscapeSafeArea: CodaEdgeSafeAreas(top: 0, leading: 59, bottom: 21, trailing: 59),
            expectedPortraitFormFactor: .iphonePortrait,
            expectedLandscapeFormFactor: .iphoneLandscape
        ),
        CodaIPhoneDeviceSpec(
            name: "iPhone 15 Plus",
            generation: "15",
            logicalSize: CGSize(width: 430, height: 932),
            scaleFactor: 3,
            portraitSafeArea: CodaEdgeSafeAreas(top: 59, leading: 0, bottom: 34, trailing: 0),
            landscapeSafeArea: CodaEdgeSafeAreas(top: 0, leading: 59, bottom: 21, trailing: 59),
            isHorizontalRegularInLandscape: true,
            expectedPortraitFormFactor: .iphonePortrait,
            expectedLandscapeFormFactor: .iphoneLandscape
        ),
        CodaIPhoneDeviceSpec(
            name: "iPhone 15 Pro",
            generation: "15",
            logicalSize: CGSize(width: 393, height: 852),
            scaleFactor: 3,
            portraitSafeArea: CodaEdgeSafeAreas(top: 59, leading: 0, bottom: 34, trailing: 0),
            landscapeSafeArea: CodaEdgeSafeAreas(top: 0, leading: 59, bottom: 21, trailing: 59),
            expectedPortraitFormFactor: .iphonePortrait,
            expectedLandscapeFormFactor: .iphoneLandscape
        ),
        CodaIPhoneDeviceSpec(
            name: "iPhone 15 Pro Max",
            generation: "15",
            logicalSize: CGSize(width: 430, height: 932),
            scaleFactor: 3,
            portraitSafeArea: CodaEdgeSafeAreas(top: 59, leading: 0, bottom: 34, trailing: 0),
            landscapeSafeArea: CodaEdgeSafeAreas(top: 0, leading: 59, bottom: 21, trailing: 59),
            isHorizontalRegularInLandscape: true,
            expectedPortraitFormFactor: .iphonePortrait,
            expectedLandscapeFormFactor: .iphoneLandscape
        ),
        // iPhone 16 Series
        CodaIPhoneDeviceSpec(
            name: "iPhone 16e",
            generation: "16",
            logicalSize: CGSize(width: 390, height: 844),
            scaleFactor: 3,
            portraitSafeArea: CodaEdgeSafeAreas(top: 59, leading: 0, bottom: 34, trailing: 0),
            landscapeSafeArea: CodaEdgeSafeAreas(top: 0, leading: 59, bottom: 20, trailing: 59),
            expectedPortraitFormFactor: .iphonePortrait,
            expectedLandscapeFormFactor: .iphoneLandscape
        ),
        CodaIPhoneDeviceSpec(
            name: "iPhone 16",
            generation: "16",
            logicalSize: CGSize(width: 393, height: 852),
            scaleFactor: 3,
            portraitSafeArea: CodaEdgeSafeAreas(top: 59, leading: 0, bottom: 34, trailing: 0),
            landscapeSafeArea: CodaEdgeSafeAreas(top: 0, leading: 59, bottom: 20, trailing: 59),
            expectedPortraitFormFactor: .iphonePortrait,
            expectedLandscapeFormFactor: .iphoneLandscape
        ),
        CodaIPhoneDeviceSpec(
            name: "iPhone 16 Plus",
            generation: "16",
            logicalSize: CGSize(width: 430, height: 932),
            scaleFactor: 3,
            portraitSafeArea: CodaEdgeSafeAreas(top: 59, leading: 0, bottom: 34, trailing: 0),
            landscapeSafeArea: CodaEdgeSafeAreas(top: 0, leading: 59, bottom: 20, trailing: 59),
            isHorizontalRegularInLandscape: true,
            expectedPortraitFormFactor: .iphonePortrait,
            expectedLandscapeFormFactor: .iphoneLandscape
        ),
        CodaIPhoneDeviceSpec(
            name: "iPhone 16 Pro",
            generation: "16",
            logicalSize: CGSize(width: 402, height: 874),
            scaleFactor: 3,
            portraitSafeArea: CodaEdgeSafeAreas(top: 62, leading: 0, bottom: 34, trailing: 0),
            landscapeSafeArea: CodaEdgeSafeAreas(top: 0, leading: 62, bottom: 20, trailing: 62),
            expectedPortraitFormFactor: .iphonePortrait,
            expectedLandscapeFormFactor: .iphoneLandscape
        ),
        CodaIPhoneDeviceSpec(
            name: "iPhone 16 Pro Max",
            generation: "16",
            logicalSize: CGSize(width: 440, height: 956),
            scaleFactor: 3,
            portraitSafeArea: CodaEdgeSafeAreas(top: 62, leading: 0, bottom: 34, trailing: 0),
            landscapeSafeArea: CodaEdgeSafeAreas(top: 0, leading: 62, bottom: 20, trailing: 62),
            isHorizontalRegularInLandscape: true,
            expectedPortraitFormFactor: .iphonePortrait,
            expectedLandscapeFormFactor: .iphoneLandscape
        ),
        // iPhone 17 Series
        CodaIPhoneDeviceSpec(
            name: "iPhone 17e",
            generation: "17",
            logicalSize: CGSize(width: 390, height: 844),
            scaleFactor: 3,
            portraitSafeArea: CodaEdgeSafeAreas(top: 59, leading: 0, bottom: 34, trailing: 0),
            landscapeSafeArea: CodaEdgeSafeAreas(top: 0, leading: 59, bottom: 20, trailing: 59),
            expectedPortraitFormFactor: .iphonePortrait,
            expectedLandscapeFormFactor: .iphoneLandscape
        ),
        CodaIPhoneDeviceSpec(
            name: "iPhone 17",
            generation: "17",
            logicalSize: CGSize(width: 402, height: 874),
            scaleFactor: 3,
            portraitSafeArea: CodaEdgeSafeAreas(top: 62, leading: 0, bottom: 34, trailing: 0),
            landscapeSafeArea: CodaEdgeSafeAreas(top: 0, leading: 62, bottom: 20, trailing: 62),
            expectedPortraitFormFactor: .iphonePortrait,
            expectedLandscapeFormFactor: .iphoneLandscape
        ),
        CodaIPhoneDeviceSpec(
            name: "iPhone 17 Air",
            generation: "17",
            logicalSize: CGSize(width: 420, height: 912),
            scaleFactor: 3,
            portraitSafeArea: CodaEdgeSafeAreas(top: 68, leading: 0, bottom: 34, trailing: 0),
            landscapeSafeArea: CodaEdgeSafeAreas(top: 0, leading: 68, bottom: 20, trailing: 68),
            expectedPortraitFormFactor: .iphonePortrait,
            expectedLandscapeFormFactor: .iphoneLandscape
        ),
        CodaIPhoneDeviceSpec(
            name: "iPhone 17 Pro",
            generation: "17",
            logicalSize: CGSize(width: 402, height: 874),
            scaleFactor: 3,
            portraitSafeArea: CodaEdgeSafeAreas(top: 62, leading: 0, bottom: 34, trailing: 0),
            landscapeSafeArea: CodaEdgeSafeAreas(top: 0, leading: 62, bottom: 20, trailing: 62),
            expectedPortraitFormFactor: .iphonePortrait,
            expectedLandscapeFormFactor: .iphoneLandscape
        ),
        CodaIPhoneDeviceSpec(
            name: "iPhone 17 Pro Max",
            generation: "17",
            logicalSize: CGSize(width: 440, height: 956),
            scaleFactor: 3,
            portraitSafeArea: CodaEdgeSafeAreas(top: 62, leading: 0, bottom: 34, trailing: 0),
            landscapeSafeArea: CodaEdgeSafeAreas(top: 0, leading: 62, bottom: 20, trailing: 62),
            isHorizontalRegularInLandscape: true,
            expectedPortraitFormFactor: .iphonePortrait,
            expectedLandscapeFormFactor: .iphoneLandscape
        ),
        // iPhone 18 Series
        CodaIPhoneDeviceSpec(
            name: "iPhone 18",
            generation: "18",
            logicalSize: CGSize(width: 402, height: 874),
            scaleFactor: 3,
            portraitSafeArea: CodaEdgeSafeAreas(top: 62, leading: 0, bottom: 34, trailing: 0),
            landscapeSafeArea: CodaEdgeSafeAreas(top: 0, leading: 62, bottom: 20, trailing: 62),
            expectedPortraitFormFactor: .iphonePortrait,
            expectedLandscapeFormFactor: .iphoneLandscape
        ),
        CodaIPhoneDeviceSpec(
            name: "iPhone 18 Pro",
            generation: "18",
            logicalSize: CGSize(width: 402, height: 874),
            scaleFactor: 3,
            portraitSafeArea: CodaEdgeSafeAreas(top: 62, leading: 0, bottom: 34, trailing: 0),
            landscapeSafeArea: CodaEdgeSafeAreas(top: 0, leading: 62, bottom: 20, trailing: 62),
            expectedPortraitFormFactor: .iphonePortrait,
            expectedLandscapeFormFactor: .iphoneLandscape
        ),
        CodaIPhoneDeviceSpec(
            name: "iPhone 18 Pro Max",
            generation: "18",
            logicalSize: CGSize(width: 440, height: 956),
            scaleFactor: 3,
            portraitSafeArea: CodaEdgeSafeAreas(top: 62, leading: 0, bottom: 34, trailing: 0),
            landscapeSafeArea: CodaEdgeSafeAreas(top: 0, leading: 62, bottom: 20, trailing: 62),
            isHorizontalRegularInLandscape: true,
            expectedPortraitFormFactor: .iphonePortrait,
            expectedLandscapeFormFactor: .iphoneLandscape
        ),
        // iPhone Duo (Foldable 5.4" Cover & 7.6" Inner)
        CodaIPhoneDeviceSpec(
            name: "iPhone Duo Cover (5.4\")",
            generation: "Duo",
            logicalSize: CGSize(width: 466, height: 678),
            scaleFactor: 3,
            portraitSafeArea: CodaEdgeSafeAreas(top: 0, leading: 0, bottom: 34, trailing: 84),
            landscapeSafeArea: CodaEdgeSafeAreas(top: 0, leading: 0, bottom: 20, trailing: 84),
            isHorizontalRegularInPortrait: false,
            isVerticalRegularInPortrait: true,
            isHorizontalRegularInLandscape: false,
            isVerticalRegularInLandscape: false,
            expectedPortraitFormFactor: .iphoneDuoCoverPortrait,
            expectedLandscapeFormFactor: .iphoneDuoCoverLandscape
        ),
        CodaIPhoneDeviceSpec(
            name: "iPhone Duo Inner (7.6\")",
            generation: "Duo",
            logicalSize: CGSize(width: 626, height: 890),
            scaleFactor: 3,
            portraitSafeArea: CodaEdgeSafeAreas(top: 0, leading: 0, bottom: 24, trailing: 84),
            landscapeSafeArea: CodaEdgeSafeAreas(top: 0, leading: 0, bottom: 20, trailing: 84),
            isHorizontalRegularInPortrait: true,
            isVerticalRegularInPortrait: true,
            isHorizontalRegularInLandscape: true,
            isVerticalRegularInLandscape: true,
            expectedPortraitFormFactor: .iphoneDuoInnerPortrait,
            expectedLandscapeFormFactor: .iphoneDuoInnerLandscape
        ),
    ]
}

// MARK: - Path Boundary Helpers

/// Returns true iff `candidate` equals `root` or lies strictly underneath `root/` at a path segment boundary.
public func codaPathWithinRoot(_ candidate: String, root: String) -> Bool {
    let trimmedRoot = root.trimmingCharacters(in: .whitespacesAndNewlines)
    let trimmedCandidate = candidate.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !trimmedRoot.isEmpty, !trimmedCandidate.isEmpty else { return false }
    let normalizedRoot = URL(fileURLWithPath: trimmedRoot).standardized.path
    let normalizedCandidate = URL(fileURLWithPath: trimmedCandidate).standardized.path
    if normalizedCandidate == normalizedRoot {
        return true
    }
    if normalizedRoot == "/" {
        return normalizedCandidate.hasPrefix("/")
    }
    return normalizedCandidate.hasPrefix(normalizedRoot + "/")
}
