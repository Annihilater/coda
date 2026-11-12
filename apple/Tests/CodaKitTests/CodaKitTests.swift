import CodaGhosttyKit
import CodaKit
import Foundation
import XCTest

final class CodaKitTests: XCTestCase {
    func testZeroKnowledgeE2EERoundtripAndReplayRejection() throws {
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

        let commandJSON = Data(#"{"op":"send_terminal_text","req_id":"1","terminal":"t1","text":"cargo test"}"#.utf8)
        let sealed = try clientSess.sealJSON(commandJSON)
        XCTAssertEqual(sealed.seq, 1)

        let opened = try daemonSess.openJSON(sealed)
        XCTAssertEqual(opened, commandJSON)

        // Replay must fail immediately
        XCTAssertThrowsError(try daemonSess.openJSON(sealed))
    }

    func testOSC7501ParsingAndiPhoneDuoUnfoldReflow() {
        var surface = GhosttyReflowEngine(columns: 40, rows: 10)
        // Simulate 5.4" folded cover screen receiving OSC 7501 blocked status + long line
        let stream = "\u{1B}]7501;state=blocked;kind=permission;app=claude;title=RunTests;progress=80\u{07}"
            + "Claude Code modified crates/coda-server/src/relay.rs and awaits approval to run tests\n"
        surface.feed(Data(stream.utf8))

        XCTAssertEqual(surface.latestHerdrStatus?.state, .blocked)
        XCTAssertEqual(surface.latestHerdrStatus?.kind, "permission")
        XCTAssertEqual(surface.latestHerdrStatus?.app, "claude")
        XCTAssertEqual(surface.latestHerdrStatus?.progress, 80)
        XCTAssertGreaterThan(surface.visibleLines.count, 2, "40-col cover screen wraps long line")

        // Unfold iPhone Duo to 7.6" inner display (108 columns) -> instant single-line reflow!
        surface.resizeReflow(columns: 108, rows: 28)
        XCTAssertEqual(surface.visibleLines.count, 1, "108-col unfolded screen reflows cleanly into 1 line")
    }
}
