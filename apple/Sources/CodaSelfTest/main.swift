import CodaGhosttyKit
import CodaKit
import Foundation

@main
struct CodaSelfTest {
    static func main() throws {
        // 1. Verify Apple CryptoKit Curve25519 + HKDF-SHA256 + ChaChaPoly E2EE & Anti-Replay
        let iphoneDuo = CodaDeviceIdentity.generate(name: "iPhone Duo")
        let macStudio = CodaDeviceIdentity.generate(name: "Home Mac Studio")

        var clientSess = try CodaE2EESession(
            myX25519SecretB64: iphoneDuo.x25519SecretB64,
            peerX25519PublicB64: macStudio.x25519PublicB64,
            isClientInitiator: true
        )
        var daemonSess = try CodaE2EESession(
            myX25519SecretB64: macStudio.x25519SecretB64,
            peerX25519PublicB64: iphoneDuo.x25519PublicB64,
            isClientInitiator: false
        )

        let msg = Data(#"{"op":"ping","req_id":"apple-1","sent_ms":100}"#.utf8)
        let sealed = try clientSess.sealJSON(msg)
        let opened = try daemonSess.openJSON(sealed)
        precondition(opened == msg, "Decrypted payload mismatch")

        var replayRejected = false
        do {
            _ = try daemonSess.openJSON(sealed)
        } catch {
            replayRejected = true
        }
        precondition(replayRejected, "Replay attack was not rejected")

        // 2. Verify OSC 7501 Semantic Agent Status & iPhone Duo 5.4" -> 7.6" Unfold Reflow
        var surface = GhosttyReflowEngine(columns: 40, rows: 10)
        let stream = "\u{1B}]7501;state=blocked;kind=permission;app=claude;title=RunTests;progress=85\u{07}"
            + "Claude Code modified crates/coda-server/src/relay.rs and awaits approval to run tests\n"
        surface.feed(Data(stream.utf8))
        precondition(surface.latestHerdrStatus?.state == .blocked)
        precondition(surface.latestHerdrStatus?.kind == "permission")
        precondition(surface.latestHerdrStatus?.progress == 85)
        precondition(surface.visibleLines.count > 2)

        surface.resizeReflow(columns: 108, rows: 28)
        precondition(surface.visibleLines.count == 1)

        // 3. Verify Phase 2 Base64 Live PTY Chunk Feeding & Voice Model Config Codable Roundtrip
        let ptyChunkB64 = Data("\u{1B}[32m● PTY Live Stream OK\u{1B}[0m\r\n".utf8).base64EncodedString()
        surface.feedBase64PtyChunk(ptyChunkB64)
        precondition(surface.visibleLines.last?.contains("PTY Live Stream OK") == true)

        let voiceCfg = CodaVoiceModelConfig(
            provider: "openai",
            baseURL: "https://api.openai.com/v1",
            apiKey: "sk-apple-test",
            model: "gpt-4o-mini-transcribe"
        )
        let voiceJSON = try JSONEncoder().encode(voiceCfg)
        let decodedVoice = try JSONDecoder().decode(CodaVoiceModelConfig.self, from: voiceJSON)
        precondition(decodedVoice == voiceCfg)

        // 4. Verify Phase 2 Git Repo & Worktree JSON Wire Compatibility with Rust `coda-protocol`
        let sampleRepoJSON = Data("""
        {
          "repo_name": "coda",
          "root_path": "/Users/vampire/personal/github/coda",
          "machine_name": "Home",
          "branches": [
            {
              "branch": "main",
              "worktree_path": "/Users/vampire/personal/github/coda",
              "diff_added": 1233,
              "diff_deleted": 116,
              "status_summary": "4 个终端运行中"
            }
          ]
        }
        """.utf8)
        let repo = try JSONDecoder().decode(CodaRepoWorkspaceSummary.self, from: sampleRepoJSON)
        precondition(repo.repoName == "coda")
        precondition(repo.branches.first?.diffAdded == 1233)
        precondition(repo.branches.first?.statusSummary == "4 个终端运行中")

        print("✅ CodaSelfTest passed: Apple CryptoKit E2EE + OSC 7501 + PTY Live Stream + Worktrees + iPhone Duo Unfold Reflow verified!")
    }
}
