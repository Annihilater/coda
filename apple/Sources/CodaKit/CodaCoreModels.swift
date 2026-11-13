import CryptoKit
import Foundation

// MARK: - Zero-Knowledge Device Identity & E2EE Session (100% Rust `coda-protocol` Wire Compatible)

public struct CodaDeviceIdentity: Codable, Sendable {
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
        case .compressedFrameRequiresRustFFI: return "zstd frame decompression delegates to libcoda_apple_ffi"
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
/// Matches `crates/coda-protocol/src/lib.rs` byte-for-byte!
public struct CodaE2EESession: Sendable {
    private let txKey: SymmetricKey
    private let rxKey: SymmetricKey
    private let txDirTag: UInt8
    private let rxDirTag: UInt8
    private var nextTxSeq: UInt64 = 1
    private var highestRxSeq: UInt64?

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
        if payload.compressed {
            throw CodaCryptoError.compressedFrameRequiresRustFFI
        }
        guard let raw = Data(base64Encoded: payload.ciphertextB64), raw.count >= 16 else {
            throw CodaCryptoError.invalidCiphertext
        }
        let ct = raw.prefix(raw.count - 16)
        let tag = raw.suffix(16)
        let nonce = try Self.makeNonce(dirTag: rxDirTag, seq: payload.seq)
        let box = try ChaChaPoly.SealedBox(nonce: nonce, ciphertext: ct, tag: tag)
        let aad = Data([0x00])
        let pt = try ChaChaPoly.open(box, using: rxKey, authenticating: aad)
        highestRxSeq = payload.seq
        return pt
    }
}

// MARK: - Domain Models (Multi-Tenant Devices, SmartPath Routes & Herdr Agents)

public enum SmartPathRouteMode: String, Codable, Sendable {
    case lanDirect = "L0 LAN Direct"
    case cnIPv6P2P = "L1 IPv6 P2P"
    case domesticRawTCP = "L2 CN RawTCP"
    case globalCDNWSS = "L2 CDN WSS"
}

public enum HerdrLifecycleState: String, Codable, Sendable, CaseIterable {
    case working
    case blocked
    case idle
    case done
    case error
}

public struct CodaAgentSession: Identifiable, Codable, Sendable, Equatable {
    public let id: String          // e.g. "mac-studio/t3"
    public let address: String     // e.g. "t3"
    public let machineName: String
    public let machinePlatform: String
    public var agentApp: String    // "claude", "grok", "codex", "gemini"
    public var title: String
    public var statusMessage: String
    public var cwd: String
    public var state: HerdrLifecycleState
    public var blockedKind: String? // "permission", "question", "auth"
    public var progress: Int?
    public var diffSummary: String?
    public var screenPreview: String

    public init(
        id: String,
        address: String,
        machineName: String,
        machinePlatform: String,
        agentApp: String,
        title: String,
        statusMessage: String,
        cwd: String,
        state: HerdrLifecycleState,
        blockedKind: String? = nil,
        progress: Int? = nil,
        diffSummary: String? = nil,
        screenPreview: String = ""
    ) {
        self.id = id
        self.address = address
        self.machineName = machineName
        self.machinePlatform = machinePlatform
        self.agentApp = agentApp
        self.title = title
        self.statusMessage = statusMessage
        self.cwd = cwd
        self.state = state
        self.blockedKind = blockedKind
        self.progress = progress
        self.diffSummary = diffSummary
        self.screenPreview = screenPreview
    }
}

// MARK: - Phase 2 & 3 Models: Git Workspaces, Worktrees, Voice Config & Direct P2P Candidates

public struct CodaBranchWorktreeItem: Identifiable, Codable, Sendable, Equatable {
    public var id: String { "\(worktreePath)#\(branch)" }
    public let branch: String
    public let worktreePath: String
    public let diffAdded: UInt32
    public let diffDeleted: UInt32
    public let statusSummary: String

    enum CodingKeys: String, CodingKey {
        case branch
        case worktreePath = "worktree_path"
        case diffAdded = "diff_added"
        case diffDeleted = "diff_deleted"
        case statusSummary = "status_summary"
    }

    public init(
        branch: String,
        worktreePath: String,
        diffAdded: UInt32,
        diffDeleted: UInt32,
        statusSummary: String
    ) {
        self.branch = branch
        self.worktreePath = worktreePath
        self.diffAdded = diffAdded
        self.diffDeleted = diffDeleted
        self.statusSummary = statusSummary
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

public struct CodaEndpointCandidate: Codable, Sendable, Equatable {
    public let kind: String
    public let addr: String

    public init(kind: String, addr: String) {
        self.kind = kind
        self.addr = addr
    }
}

public struct CodaRemoteDevice: Identifiable, Codable, Sendable, Equatable {
    public let id: UUID
    public let name: String
    public let role: String
    public let platform: String
    public let x25519Pubkey: String
    public let endpoints: [CodaEndpointCandidate]
    public let isOnline: Bool

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
