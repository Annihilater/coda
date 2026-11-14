import CoreGraphics
import CryptoKit
import Darwin
import Foundation

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
        if let data = UserDefaults.standard.data(forKey: persistedDefaultsKey),
           let decoded = try? JSONDecoder().decode(CodaDeviceIdentity.self, from: data) {
            return decoded
        }
        let created = generate(name: defaultName)
        savePersisted(created)
        return created
    }

    public static func savePersisted(_ identity: CodaDeviceIdentity) {
        if let data = try? JSONEncoder().encode(identity) {
            UserDefaults.standard.set(data, forKey: persistedDefaultsKey)
        }
    }

    public func sign(message: Data) throws -> String {
        guard let raw = Data(base64Encoded: ed25519SecretB64) else {
            throw CodaCryptoError.invalidKeyEncoding
        }
        let key = try Curve25519.Signing.PrivateKey(rawRepresentation: raw)
        let sig = try key.signature(for: message)
        return sig.base64EncodedString()
    }
}

public enum CodaCryptoError: Error, LocalizedError {
    case invalidKeyEncoding
    case replayDetected(UInt64)
    case invalidCiphertext
    case compressedFrameRequiresRustFFI

    public var errorDescription: String? {
        switch self {
        case .invalidKeyEncoding: return "Invalid Curve25519 key encoding"
        case .replayDetected(let seq): return "Replay or out-of-order E2EE sequence: \(seq)"
        case .invalidCiphertext: return "Invalid ChaCha20-Poly1305 ciphertext"
        case .compressedFrameRequiresRustFFI: return "zstd frame decompression requires libcoda_apple_ffi or swift-* req_id negotiation"
        }
    }
}

public struct CodaEncryptedPayload: Codable, Sendable {
    public let seq: UInt64
    public let compressed: Bool
    public let ciphertextB64: String

    enum CodingKeys: String, CodingKey {
        case seq
        case compressed
        case ciphertextB64 = "ciphertext_b64"
    }
}

/// Pure Apple `CryptoKit` implementation of Coda's directional E2EE session (`X25519 + HKDF-SHA256 + ChaChaPoly`).
/// Matches `crates/coda-protocol/src/lib.rs` byte-for-byte, with dynamic `dlsym` FFI support for `zstd` frames.
public struct CodaE2EESession: Sendable {
    private let txKey: SymmetricKey
    private let rxKey: SymmetricKey
    private let txDirTag: UInt8
    private let rxDirTag: UInt8
    private var nextTxSeq: UInt64 = 1
    private var highestRxSeq: UInt64?

    private typealias ZstdDecompressFn = @convention(c) (UnsafePointer<CChar>?) -> UnsafeMutablePointer<CChar>?
    private typealias StringFreeFn = @convention(c) (UnsafeMutablePointer<CChar>?) -> Void

    public static var hasNativeZstdFFI: Bool {
        let rtldDefault = UnsafeMutableRawPointer(bitPattern: -2)
        return dlsym(rtldDefault, "coda_ffi_zstd_decompress_b64") != nil
    }

    public init(myX25519SecretB64: String, peerX25519PublicB64: String, isClientInitiator: Bool = true) throws {
        guard let myRaw = Data(base64Encoded: myX25519SecretB64),
              let peerRaw = Data(base64Encoded: peerX25519PublicB64) else {
            throw CodaCryptoError.invalidKeyEncoding
        }
        let myPriv = try Curve25519.KeyAgreement.PrivateKey(rawRepresentation: myRaw)
        let peerPub = try Curve25519.KeyAgreement.PublicKey(rawRepresentation: peerRaw)
        let shared = try myPriv.sharedSecretFromKeyAgreement(with: peerPub)

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
    }

    public init(myIdentity: CodaDeviceIdentity, peerX25519PublicB64: String, isInitiator: Bool = true) throws {
        try self.init(
            myX25519SecretB64: myIdentity.x25519SecretB64,
            peerX25519PublicB64: peerX25519PublicB64,
            isClientInitiator: isInitiator
        )
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

    public mutating func sealJSON(_ jsonBytes: Data) throws -> CodaEncryptedPayload {
        let seq = nextTxSeq
        nextTxSeq += 1
        let nonce = try Self.makeNonce(dirTag: txDirTag, seq: seq)
        let aad = Data([0x00])
        let box = try ChaChaPoly.seal(jsonBytes, using: txKey, nonce: nonce, authenticating: aad)
        let combinedCiphertextAndTag = box.ciphertext + box.tag
        return CodaEncryptedPayload(
            seq: seq,
            compressed: false,
            ciphertextB64: combinedCiphertextAndTag.base64EncodedString()
        )
    }

    public mutating func openJSON(_ payload: CodaEncryptedPayload) throws -> Data {
        if let prev = highestRxSeq, payload.seq <= prev {
            throw CodaCryptoError.replayDetected(payload.seq)
        }
        guard let raw = Data(base64Encoded: payload.ciphertextB64), raw.count >= 16 else {
            throw CodaCryptoError.invalidCiphertext
        }
        let ct = raw.prefix(raw.count - 16)
        let tag = raw.suffix(16)
        let nonce = try Self.makeNonce(dirTag: rxDirTag, seq: payload.seq)
        let box = try ChaChaPoly.SealedBox(nonce: nonce, ciphertext: ct, tag: tag)
        let aad = Data([payload.compressed ? 0x01 : 0x00])
        let pt = try ChaChaPoly.open(box, using: rxKey, authenticating: aad)

        if payload.compressed {
            let rtldDefault = UnsafeMutableRawPointer(bitPattern: -2)
            if let zstdSym = dlsym(rtldDefault, "coda_ffi_zstd_decompress_b64"),
               let freeSym = dlsym(rtldDefault, "coda_ffi_string_free") {
                let decompFn = unsafeBitCast(zstdSym, to: ZstdDecompressFn.self)
                let freeFn = unsafeBitCast(freeSym, to: StringFreeFn.self)
                let outPtr = pt.base64EncodedString().withCString { decompFn($0) }
                if let outPtr {
                    defer { freeFn(outPtr) }
                    let utf8 = String(cString: outPtr)
                    highestRxSeq = payload.seq
                    return Data(utf8.utf8)
                }
            }
            if let zlibData = try? (pt as NSData).decompressed(using: .zlib) as Data {
                highestRxSeq = payload.seq
                return zlibData
            }
            throw CodaCryptoError.compressedFrameRequiresRustFFI
        }

        highestRxSeq = payload.seq
        return pt
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

    private static let defaultsKey = "dev.codahub.client.settings.v2"

    public init(
        serverBaseURLString: String = "https://codahub.dev",
        sessionToken: String = "",
        ownerHandle: String = "",
        locale: CodaUILocale = .en,
        pinnedSessionIDs: [String] = [],
        voiceConfig: CodaVoiceModelConfig = CodaVoiceModelConfig()
    ) {
        self.serverBaseURLString = serverBaseURLString
        self.sessionToken = sessionToken
        self.ownerHandle = ownerHandle
        self.locale = locale
        self.pinnedSessionIDs = pinnedSessionIDs
        self.voiceConfig = voiceConfig
    }

    public static func load() -> CodaPersistedSettings {
        guard let data = UserDefaults.standard.data(forKey: defaultsKey),
              let decoded = try? JSONDecoder().decode(CodaPersistedSettings.self, from: data) else {
            return CodaPersistedSettings()
        }
        return decoded
    }

    public func save() {
        if let data = try? JSONEncoder().encode(self) {
            UserDefaults.standard.set(data, forKey: Self.defaultsKey)
        }
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
    public let x25519Pubkey: String
    public let endpoints: [CodaEndpointCandidate]
    public let isOnline: Bool

    public var x25519PublicB64: String { x25519Pubkey }

    enum CodingKeys: String, CodingKey {
        case id
        case name
        case role
        case platform
        case x25519Pubkey = "x25519_pubkey"
        case endpoints = "endpoints_json"
        case isOnline = "is_online"
    }

    public init(
        id: UUID,
        name: String,
        role: String,
        platform: String,
        x25519Pubkey: String,
        endpoints: [CodaEndpointCandidate],
        isOnline: Bool
    ) {
        self.id = id
        self.name = name
        self.role = role
        self.platform = platform
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
        x25519Pubkey = try c.decode(String.self, forKey: .x25519Pubkey)
        endpoints = (try? c.decodeIfPresent([CodaEndpointCandidate].self, forKey: .endpoints)) ?? []
        isOnline = try c.decode(Bool.self, forKey: .isOnline)
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
        case .iphoneLandscape, .iphoneDuoCoverLandscape, .iphoneDuoInnerLandscape:
            return true
        case .iphonePortrait, .iphoneDuoCoverPortrait, .iphoneDuoInnerPortrait, .ipadOrDesktop:
            return false
        }
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
        isVerticalRegular: Bool? = nil
    ) -> CodaFormFactor {
        let w = max(1, width)
        let h = max(1, height)
        let minDim = min(w, h)
        let maxDim = max(w, h)
        let aspect = maxDim / minDim

        // Mainstream slab iPhones (iPhone 14 .. 18 Pro Max, 375..440pt short edge, ~2.16 aspect)
        if (minDim <= 448 && aspect >= 1.70) || (isVerticalRegular == false && minDim <= 448 && maxDim >= 650) {
            return w > h ? .iphoneLandscape : .iphonePortrait
        }

        // iPhone Duo 5.4" Cover Display (466x678 Portrait, 678x466 Landscape, ~1.455 aspect)
        if minDim >= 449 && minDim <= 540 && maxDim >= 600 && maxDim <= 755 && aspect < 1.70
            && (isHorizontalRegular != true || isVerticalRegular == false)
        {
            return w > h ? .iphoneDuoCoverLandscape : .iphoneDuoCoverPortrait
        }

        // iPhone Duo 7.6" Unfolded Inner Display (890x626 / 626x890, Zoomed 951x669 / 669x951)
        if minDim >= 560 && minDim <= 712 && maxDim >= 780 && maxDim <= 975 && aspect >= 1.25 && aspect <= 1.65 {
            return w > h ? .iphoneDuoInnerLandscape : .iphoneDuoInnerPortrait
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
            portraitSafeArea: CodaEdgeSafeAreas(top: 54, leading: 0, bottom: 34, trailing: 0),
            landscapeSafeArea: CodaEdgeSafeAreas(top: 0, leading: 84, bottom: 20, trailing: 54),
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
            portraitSafeArea: CodaEdgeSafeAreas(top: 44, leading: 0, bottom: 34, trailing: 0),
            landscapeSafeArea: CodaEdgeSafeAreas(top: 24, leading: 84, bottom: 20, trailing: 16),
            isHorizontalRegularInPortrait: true,
            isVerticalRegularInPortrait: true,
            isHorizontalRegularInLandscape: true,
            isVerticalRegularInLandscape: true,
            expectedPortraitFormFactor: .iphoneDuoInnerPortrait,
            expectedLandscapeFormFactor: .iphoneDuoInnerLandscape
        ),
    ]
}
