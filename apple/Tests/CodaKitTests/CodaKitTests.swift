#if canImport(XCTest)
import CodaGhosttyKit
import CodaKit
import Foundation
import XCTest

final class CodaKitTests: XCTestCase {
    func testZeroKnowledgeE2EERoundtripAndReplayRejection() throws {
        let iphone = CodaDeviceIdentity.generate(name: "iPhone Client")
        let host = CodaDeviceIdentity.generate(name: "Workstation Host")

        var clientSess = try CodaE2EESession(
            myX25519SecretB64: iphone.x25519SecretB64,
            peerX25519PublicB64: host.x25519PublicB64,
            isClientInitiator: true
        )
        var daemonSess = try CodaE2EESession(
            myX25519SecretB64: host.x25519SecretB64,
            peerX25519PublicB64: iphone.x25519PublicB64,
            isClientInitiator: false
        )

        let commandJSON = Data(#"{"op":"send_terminal_text","req_id":"swift-1","terminal":"t1","text":"cargo test"}"#.utf8)
        let sealed = try clientSess.sealJSON(commandJSON)
        XCTAssertEqual(sealed.seq, 1)

        let opened = try daemonSess.openJSON(sealed)
        XCTAssertEqual(opened, commandJSON)

        XCTAssertThrowsError(try daemonSess.openJSON(sealed))
    }

    func testOSC7501ParsingTrueColorDiffAndReflow() {
        var surface = GhosttyReflowEngine(columns: 40, rows: 10)
        let stream = "\u{1B}]7501;state=blocked;kind=permission;app=claude;title=RunTests;progress=80\u{07}"
            + "Claude Code modified crates/coda-server/src/relay.rs and awaits approval to run tests\n"
        surface.feed(Data(stream.utf8))

        XCTAssertEqual(surface.latestHerdrStatus?.state, .needsYou)
        XCTAssertEqual(surface.latestHerdrStatus?.kind, "permission")
        XCTAssertEqual(surface.latestHerdrStatus?.app, "claude")
        XCTAssertEqual(surface.latestHerdrStatus?.progress, 80)
        XCTAssertGreaterThan(surface.visibleLines.count, 2)

        surface.resizeReflow(columns: 108, rows: 28)
        XCTAssertEqual(surface.visibleLines.count, 1)

        let styled = GhosttyReflowEngine.renderStyledLines(
            rawOutput: "\u{1B}[38;2;56;189;248m+added diff line\u{1B}[0m\n-removed line\n1. Yes, allow",
            targetColumns: 80
        )
        XCTAssertEqual(styled.lines.count, 3)
        XCTAssertEqual(styled.lines[0].kind, .diffAdded)
        XCTAssertEqual(styled.lines[0].spans.first?.fg, GhosttyRGBColor(r: 56, g: 189, b: 248))
        XCTAssertEqual(styled.lines[1].kind, .diffRemoved)
        XCTAssertEqual(styled.lines[2].kind, .promptChoice)
    }

    func testInboxPriorityOrdering() {
        let now = Date()
        let idle = CodaAgentSession(id: "s-idle", terminalAddress: "t1", workspaceName: "app", hostName: "studio", agentName: "zsh", taskTitle: "Shell", state: .idle, updatedAt: now)
        let working = CodaAgentSession(id: "s-work", terminalAddress: "t2", workspaceName: "app", hostName: "studio", agentName: "codex", taskTitle: "Refactor", state: .working, updatedAt: now.addingTimeInterval(-30))
        let doneUnread = CodaAgentSession(id: "s-done", terminalAddress: "t3", workspaceName: "app", hostName: "studio", agentName: "claude", taskTitle: "Build OK", state: .done, unreadCompletion: true, updatedAt: now.addingTimeInterval(-60))
        let needsYou = CodaAgentSession(id: "s-needs", terminalAddress: "t4", workspaceName: "app", hostName: "studio", agentName: "claude", taskTitle: "Approve push", state: .needsYou, updatedAt: now.addingTimeInterval(-90))

        let ordered = [idle, working, doneUnread, needsYou].sorted(by: CodaAgentSession.inboxOrder)
        XCTAssertEqual(ordered.map(\.id), ["s-needs", "s-done", "s-work", "s-idle"])
    }
}
#endif
