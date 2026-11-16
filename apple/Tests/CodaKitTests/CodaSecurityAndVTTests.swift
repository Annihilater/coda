#if canImport(XCTest)
import XCTest
#endif
import CryptoKit
import Foundation
@testable import CodaGhosttyKit
@testable import CodaKit

final class CodaSecurityAndVTTests: XCTestCase {
    func testDirectP2Pv2HybridSessionRoundtripAndTranscriptBinding() throws {
        let clientID = UUID()
        let daemonID = UUID()
        let clientIdentity = CodaDeviceIdentity.generate(name: "Client")
        let daemonIdentity = CodaDeviceIdentity.generate(name: "Daemon")
        let clientEph = Curve25519.KeyAgreement.PrivateKey()
        let daemonEph = Curve25519.KeyAgreement.PrivateKey()

        var initiator = try CodaE2EESession(
            directP2Pv2Identity: clientIdentity,
            peerStaticX25519PublicB64: daemonIdentity.x25519PublicB64,
            myEphemeralPrivateKey: clientEph,
            peerEphemeralX25519PublicB64: daemonEph.publicKey.rawRepresentation.base64EncodedString(),
            clientDeviceId: clientID,
            daemonDeviceId: daemonID,
            isInitiator: true
        )
        var responder = try CodaE2EESession(
            directP2Pv2Identity: daemonIdentity,
            peerStaticX25519PublicB64: clientIdentity.x25519PublicB64,
            myEphemeralPrivateKey: daemonEph,
            peerEphemeralX25519PublicB64: clientEph.publicKey.rawRepresentation.base64EncodedString(),
            clientDeviceId: clientID,
            daemonDeviceId: daemonID,
            isInitiator: false
        )

        let sealed1 = try initiator.sealJSON(Data("{\"op\":\"ping\",\"seq\":1}".utf8))
        let opened1 = try responder.openJSON(sealed1)
        XCTAssertEqual(String(decoding: opened1, as: UTF8.self), "{\"op\":\"ping\",\"seq\":1}")

        let sealed2 = try responder.sealJSON(Data("{\"op\":\"pong\",\"ok\":true}".utf8))
        let opened2 = try initiator.openJSON(sealed2)
        XCTAssertEqual(String(decoding: opened2, as: UTF8.self), "{\"op\":\"pong\",\"ok\":true}")

        // Mismatched daemon ID (transcript binding) must fail authentication
        var wrongIDResponder = try CodaE2EESession(
            directP2Pv2Identity: daemonIdentity,
            peerStaticX25519PublicB64: clientIdentity.x25519PublicB64,
            myEphemeralPrivateKey: daemonEph,
            peerEphemeralX25519PublicB64: clientEph.publicKey.rawRepresentation.base64EncodedString(),
            clientDeviceId: clientID,
            daemonDeviceId: UUID(),
            isInitiator: false
        )
        let sealed3 = try initiator.sealJSON(Data("{}".utf8))
        XCTAssertThrowsError(try wrongIDResponder.openJSON(sealed3))
    }

    func testRelayV2SaltedSessionAndRustFFIInterop() throws {
        let clientID = UUID()
        let daemonID = UUID()
        let clientIdentity = CodaDeviceIdentity.generate(name: "Apple-Relay")
        let daemonIdentity = CodaDeviceIdentity.generate(name: "Rust-Daemon")
        let nonceB64 = CodaE2EESession.randomSessionNonceB64()

        var swiftClient = try CodaE2EESession(
            relayV2Identity: clientIdentity,
            peerStaticX25519PublicB64: daemonIdentity.x25519PublicB64,
            sessionNonceB64: nonceB64,
            clientDeviceId: clientID,
            daemonDeviceId: daemonID,
            isInitiator: true
        )

        guard let rustDaemon = CodaRustFFISessionBridge(
            identity: daemonIdentity,
            peerX25519PublicB64: clientIdentity.x25519PublicB64,
            sessionNonceB64: nonceB64,
            clientDeviceId: clientID,
            daemonDeviceId: daemonID,
            isInitiator: false,
            peerSupportsZstd: true
        ) else {
            XCTFail("Expected libcoda_apple_ffi.dylib Relay v2 bridge to load")
            return
        }

        XCTAssertTrue(CodaE2EESession.hasNativeZstdFFI)
        XCTAssertTrue(CodaE2EESession.hasRelayV2FFIBridge)

        // Swift -> Rust FFI
        let clientPayload = try swiftClient.sealJSON(Data("{\"op\":\"ping\",\"req_id\":\"r-1\",\"sent_ms\":1234}".utf8))
        let clientEncJSON = String(decoding: try JSONEncoder().encode(clientPayload), as: UTF8.self)
        let rustOpened = try XCTUnwrap(rustDaemon.openEncryptedPayloadJSON(clientEncJSON))
        XCTAssertTrue(rustOpened.contains("\"req_id\":\"r-1\""))

        // Rust FFI -> Swift (including compression path cleanly when large payload)
        let bigOutput = String(repeating: "coda-zstd-interop-line\n", count: 80)
        let rustMsgJSON = try JSONSerialization.data(withJSONObject: [
            "op": "pong",
            "req_id": "r-1",
            "sent_ms": 12345,
            "daemon_hostname": bigOutput,
            "herdr_version": "0.1.20",
        ])
        let rustSealedJSON = try XCTUnwrap(
            rustDaemon.sealPeerMessageJSON(String(decoding: rustMsgJSON, as: UTF8.self))
        )
        let rustEnc = try JSONDecoder().decode(CodaEncryptedPayload.self, from: Data(rustSealedJSON.utf8))
        let swiftOpened = try swiftClient.openJSON(rustEnc)
        let parsed = try XCTUnwrap(JSONSerialization.jsonObject(with: swiftOpened) as? [String: Any])
        XCTAssertEqual(parsed["daemon_hostname"] as? String, bigOutput)
    }

    func testPostAEADReplayWindowRejectsForgedHighSequenceAndTracksLegitimatePackets() throws {
        let clientID = UUID()
        let daemonID = UUID()
        let alice = CodaDeviceIdentity.generate(name: "Alice")
        let bob = CodaDeviceIdentity.generate(name: "Bob")
        let nonce = CodaE2EESession.randomSessionNonceB64()

        var sender = try CodaE2EESession(
            relayV2Identity: alice,
            peerStaticX25519PublicB64: bob.x25519PublicB64,
            sessionNonceB64: nonce,
            clientDeviceId: clientID,
            daemonDeviceId: daemonID,
            isInitiator: true
        )
        var receiver = try CodaE2EESession(
            relayV2Identity: bob,
            peerStaticX25519PublicB64: alice.x25519PublicB64,
            sessionNonceB64: nonce,
            clientDeviceId: clientID,
            daemonDeviceId: daemonID,
            isInitiator: false
        )

        let p1 = try sender.sealJSON(Data("{\"n\":1}".utf8))
        let p2 = try sender.sealJSON(Data("{\"n\":2}".utf8))
        _ = try receiver.openJSON(p1)

        // Inject forged packet claiming high sequence 10_000 -> must NOT poison replay window!
        let forged = CodaEncryptedPayload(
            seq: 10_000,
            compressed: false,
            ciphertextB64: Data("forged-ciphertext".utf8).base64EncodedString(),
            sessionNonceB64: nonce
        )
        XCTAssertThrowsError(try receiver.openJSON(forged))

        // Legitimate packet seq=2 must still succeed!
        let opened2 = try receiver.openJSON(p2)
        XCTAssertEqual(String(decoding: opened2, as: UTF8.self), "{\"n\":2}")

        // Replaying seq=2 must fail
        XCTAssertThrowsError(try receiver.openJSON(p2))
    }

    func testSecretStorePermissionsAndUserDefaultsStripping() throws {
        let tmpDir = FileManager.default.temporaryDirectory
            .appendingPathComponent("coda-apple-secrets-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: tmpDir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tmpDir) }

        setenv("CODA_APPLE_SECRETS_DIR", tmpDir.path, 1)
        defer { unsetenv("CODA_APPLE_SECRETS_DIR") }

        let account = "test.secret.\(UUID().uuidString)"
        let secret = "sk-secret-token-value-12345"
        CodaSecretStore.saveString(secret, account: account)
        XCTAssertEqual(CodaSecretStore.readString(account: account), secret)

        let files = try FileManager.default.contentsOfDirectory(at: tmpDir, includingPropertiesForKeys: nil)
        XCTAssertEqual(files.count, 1)
        let attrs = try FileManager.default.attributesOfItem(atPath: files[0].path)
        let perms = (attrs[.posixPermissions] as? NSNumber)?.intValue
        XCTAssertEqual(perms, 0o600)

        CodaSecretStore.delete(account: account)
        XCTAssertNil(CodaSecretStore.readString(account: account))
    }

    func testGhosttyStatefulVTUTF8ChunksCJKAltScreenAndReflow() throws {
        var engine = GhosttyReflowEngine(columns: 20, rows: 6)

        // Split 3-byte UTF-8 '你' + 4-byte '🚀' across chunk boundary
        let utf8Data = Data("Hi 你🚀\r\n".utf8)
        let splitAt = 5
        engine.feed(utf8Data.subdata(in: 0..<splitAt))
        engine.feed(utf8Data.subdata(in: splitAt..<utf8Data.count))
        XCTAssertEqual(engine.visibleLines.first, "Hi 你🚀")

        // Carriage return overwrite + backspace
        engine.feed(Data("progress 10%\rprogress 99%!\u{08} \r\n".utf8))
        XCTAssertEqual(engine.visibleLines.count, 2)
        XCTAssertEqual(engine.visibleLines[1], "progress 99%")

        // Alt-screen should preserve primary screen when exited
        engine.feed(Data("\u{1B}[?1049hALT SCREEN CONTENT\u{1B}[?1049l".utf8))
        XCTAssertEqual(engine.visibleLines[0], "Hi 你🚀")
        XCTAssertEqual(engine.visibleLines[1], "progress 99%")

        // Split OSC 7501 across chunks must not leak escape chars
        let osc = Data("\u{1B}]7501;state=blocked;kind=permission;msg=Allow?\u{07}".utf8)
        engine.feed(osc.subdata(in: 0..<12))
        engine.feed(osc.subdata(in: 12..<osc.count))
        XCTAssertEqual(engine.latestHerdrStatus?.state, .needsYou)
        XCTAssertEqual(engine.latestHerdrStatus?.message, "Allow?")
        XCTAssertFalse(engine.plainVisibleText.contains("7501"))

        // Reflow narrower/wider
        var reflowEngine = GhosttyReflowEngine(columns: 40, rows: 8)
        reflowEngine.feed(Data("123456789012345678901234567890".utf8))
        reflowEngine.resizeReflow(columns: 20, rows: 8)
        XCTAssertEqual(reflowEngine.visibleLines.count, 2)
    }

    func testPathWithinRootBoundary() {
        XCTAssertTrue(codaPathWithinRoot("/Users/dev/coda", root: "/Users/dev/coda"))
        XCTAssertTrue(codaPathWithinRoot("/Users/dev/coda/crates/coda-server", root: "/Users/dev/coda"))
        XCTAssertFalse(codaPathWithinRoot("/Users/dev/coda-other", root: "/Users/dev/coda"))
        XCTAssertFalse(codaPathWithinRoot("/Users/dev/coda2/src", root: "/Users/dev/coda"))
        XCTAssertFalse(codaPathWithinRoot("/Users/dev/coda/../coda-other", root: "/Users/dev/coda"))
    }
}
