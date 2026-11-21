#if canImport(XCTest)
import XCTest
#else
open class XCTestCase {}

struct XCTestShimError: Error {
    let message: String
}

func XCTFail(_ message: String = "", file: StaticString = #filePath, line: UInt = #line) {
    fatalError("XCTFail at \(file):\(line): \(message)")
}

func XCTAssertEqual<T: Equatable>(
    _ expression1: @autoclosure () throws -> T,
    _ expression2: @autoclosure () throws -> T,
    _ message: @autoclosure () -> String = "",
    file: StaticString = #filePath,
    line: UInt = #line
) rethrows {
    let lhs = try expression1()
    let rhs = try expression2()
    precondition(lhs == rhs, "XCTAssertEqual failed at \(file):\(line): \(lhs) != \(rhs) \(message())")
}

func XCTAssertGreaterThan<T: Comparable>(
    _ expression1: @autoclosure () throws -> T,
    _ expression2: @autoclosure () throws -> T,
    _ message: @autoclosure () -> String = "",
    file: StaticString = #filePath,
    line: UInt = #line
) rethrows {
    let lhs = try expression1()
    let rhs = try expression2()
    precondition(lhs > rhs, "XCTAssertGreaterThan failed at \(file):\(line): \(lhs) <= \(rhs) \(message())")
}

func XCTAssertTrue(
    _ expression: @autoclosure () throws -> Bool,
    _ message: @autoclosure () -> String = "",
    file: StaticString = #filePath,
    line: UInt = #line
) rethrows {
    let ok = try expression()
    precondition(ok, "XCTAssertTrue failed at \(file):\(line): \(message())")
}

func XCTAssertFalse(
    _ expression: @autoclosure () throws -> Bool,
    _ message: @autoclosure () -> String = "",
    file: StaticString = #filePath,
    line: UInt = #line
) rethrows {
    let flag = try expression()
    precondition(!flag, "XCTAssertFalse failed at \(file):\(line): \(message())")
}

func XCTAssertNil(
    _ expression: @autoclosure () throws -> Any?,
    _ message: @autoclosure () -> String = "",
    file: StaticString = #filePath,
    line: UInt = #line
) rethrows {
    let val = try expression()
    precondition(val == nil, "XCTAssertNil failed at \(file):\(line): \(message())")
}

func XCTAssertThrowsError<T>(
    _ expression: @autoclosure () throws -> T,
    _ message: @autoclosure () -> String = "",
    file: StaticString = #filePath,
    line: UInt = #line
) {
    do {
        _ = try expression()
        preconditionFailure("XCTAssertThrowsError failed at \(file):\(line): expected error \(message())")
    } catch {}
}

func XCTUnwrap<T>(
    _ expression: @autoclosure () throws -> T?,
    _ message: @autoclosure () -> String = "",
    file: StaticString = #filePath,
    line: UInt = #line
) throws -> T {
    guard let value = try expression() else {
        throw XCTestShimError(message: "XCTUnwrap failed at \(file):\(line): \(message())")
    }
    return value
}
#endif
import CodaGhosttyKit
import CodaKit
import Foundation

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

    func testV11PromptSnippetsResetTokenAndInviteURLHelpers() throws {
        XCTAssertEqual(CodaPromptSnippet.defaultStarterSnippets.count, 4)
        let snip = CodaPromptSnippet(id: "s-custom", title: "Ship It", prompt: "Run cargo test && swift build")
        let data = try JSONEncoder().encode(CodaPromptSnippetsEnvelope(snippets: [snip]))
        let decoded = try JSONDecoder().decode(CodaPromptSnippetsEnvelope.self, from: data)
        XCTAssertEqual(decoded.snippets, [snip])

        let tok = "rst_AbCdEfGhIjKlMnOpQrStUvWxYz0123456789_-"
        XCTAssertEqual(extractResetTokenFromInput(tok), tok)
        XCTAssertEqual(extractResetTokenFromInput("https://codahub.dev/reset-password?token=\(tok)"), tok)
        XCTAssertNil(extractResetTokenFromInput("https://codahub.dev/login"))
        XCTAssertNil(extractResetTokenFromInput("rst_"))

        XCTAssertEqual(
            buildInviteRegistrationURL(serverURL: "https://codahub.dev/", code: "CODA-ABCDEF"),
            "https://codahub.dev/register?invite=CODA-ABCDEF"
        )
        XCTAssertEqual(
            buildOneLineInstallCommand(serverURL: "https://codahub.dev/"),
            "curl -fsSL https://codahub.dev/install.sh | sh"
        )
    }
}
