import CodaKit
import Foundation

// MARK: - `libghostty-vt` C-ABI Surface Protocol & Instant Foldable Reflow Engine

/// Abstracts the terminal state machine so `libghostty-vt.xcframework` (Metal 120Hz renderer)
/// slots in directly via its stable C ABI (`ghostty_terminal_*` / `ghostty_key_encode`),
/// while providing a pure-Swift VT + `OSC 7501` engine out-of-the-box for unit tests & previews.
public protocol GhosttyVTSurface: Sendable {
    var columns: Int { get }
    var rows: Int { get }
    var visibleLines: [String] { get }
    var latestHerdrStatus: HerdrOSC7501Status? { get }
    mutating func feed(_ utf8Bytes: Data)
    mutating func resizeReflow(columns: Int, rows: Int)
    func encodeKeyChord(_ chord: TactileChordAction) -> Data
}

/// Parsed `OSC 7501` semantic agent lifecycle signal emitted by `herdr-core` & AI Coding CLIs.
public struct HerdrOSC7501Status: Equatable, Sendable {
    public let state: HerdrLifecycleState
    public let kind: String?     // "permission" | "question" | "auth"
    public let app: String?      // "claude" | "grok" | "codex" | "gemini"
    public let title: String?
    public let message: String?
    public let progress: Int?

    public init(
        state: HerdrLifecycleState,
        kind: String? = nil,
        app: String? = nil,
        title: String? = nil,
        message: String? = nil,
        progress: Int? = nil
    ) {
        self.state = state
        self.kind = kind
        self.app = app
        self.title = title
        self.message = message
        self.progress = progress
    }
}

/// Tactile Chord Bar actions tailored for mobile control of Claude Code, Codex, Grok Build, and Gemini CLI.
public enum TactileChordAction: String, CaseIterable, Sendable, Identifiable {
    case shiftTabPlan = "⇧Tab Plan"
    case escapeInterrupt = "Esc"
    case shiftEnterNewline = "⇧↵ Line"
    case approveYes = "Approve (y↵)"
    case ctrlC = "⌃C"
    case atFilePicker = "@File"
    case slashCommand = "/Cmd"

    public var id: String { rawValue }

    /// Kitty Keyboard Protocol (`CSI u`) & standard VT sequences matching `libghostty-vt` `KeyEncoder`.
    public var encodedBytes: Data {
        switch self {
        case .shiftTabPlan:
            return Data("\u{1B}[Z".utf8)           // Backtab (switches Plan/Auto mode in Claude Code & Grok Build)
        case .escapeInterrupt:
            return Data([0x1B])                    // ESC
        case .shiftEnterNewline:
            return Data("\u{1B}[13;2u".utf8)       // Kitty Keyboard Protocol Shift+Enter
        case .approveYes:
            return Data("y\r".utf8)
        case .ctrlC:
            return Data([0x03])
        case .atFilePicker:
            return Data("@".utf8)
        case .slashCommand:
            return Data("/".utf8)
        }
    }
}

/// High-speed Swift reference engine with `OSC 7501` interception and non-destructive paragraph reflow
/// when unfolding **iPhone Duo** between its 5.4" Cover Display (~52 cols) and 7.6" Inner Display (~108 cols).
public struct GhosttyReflowEngine: GhosttyVTSurface {
    public private(set) var columns: Int
    public private(set) var rows: Int
    private var logicalParagraphs: [String] = []
    public private(set) var visibleLines: [String] = []
    public private(set) var latestHerdrStatus: HerdrOSC7501Status?

    public init(columns: Int = 52, rows: Int = 24) {
        self.columns = max(20, columns)
        self.rows = max(6, rows)
    }

    public mutating func feed(_ utf8Bytes: Data) {
        guard let chunk = String(data: utf8Bytes, encoding: .utf8) else { return }
        let cleaned = extractOSC7501AndStripANSI(chunk)
        let incoming = cleaned.components(separatedBy: "\n")
        for line in incoming {
            let trimmed = line.replacingOccurrences(of: "\r", with: "").trimmingCharacters(in: .controlCharacters)
            if !trimmed.isEmpty {
                logicalParagraphs.append(trimmed)
            }
        }
        if logicalParagraphs.count > 500 {
            logicalParagraphs.removeFirst(logicalParagraphs.count - 500)
        }
        rebuildReflowedGrid()
    }

    /// Feeds a base64-encoded raw ANSI/VT chunk emitted by `PeerMessage::PtyOutputChunk` (`hc attach`).
    public mutating func feedBase64PtyChunk(_ dataB64: String) {
        guard let raw = Data(base64Encoded: dataB64) else { return }
        feed(raw)
    }

    public mutating func resizeReflow(columns: Int, rows: Int) {
        self.columns = max(20, columns)
        self.rows = max(6, rows)
        rebuildReflowedGrid()
    }

    public func encodeKeyChord(_ chord: TactileChordAction) -> Data {
        chord.encodedBytes
    }

    private mutating func rebuildReflowedGrid() {
        var wrapped: [String] = []
        for p in logicalParagraphs {
            if p.count <= columns {
                wrapped.append(p)
            } else {
                var idx = p.startIndex
                while idx < p.endIndex {
                    let end = p.index(idx, offsetBy: columns, limitedBy: p.endIndex) ?? p.endIndex
                    wrapped.append(String(p[idx..<end]))
                    idx = end
                }
            }
        }
        if wrapped.count > rows {
            visibleLines = Array(wrapped.suffix(rows))
        } else {
            visibleLines = wrapped
        }
    }

    /// Intercepts `ESC ] 7501 ; state=blocked;kind=permission;app=claude;msg=... BEL/ST`
    private mutating func extractOSC7501AndStripANSI(_ input: String) -> String {
        var working = input
        while let oscRange = working.range(of: "\u{1B}]7501;") {
            let afterPrefix = working[oscRange.upperBound...]
            if let terminator = afterPrefix.firstIndex(where: { $0 == "\u{07}" || $0 == "\u{1B}" }) {
                let body = String(afterPrefix[..<terminator])
                parseOSC7501Payload(body)
                var endIdx = working.index(after: terminator)
                if working[terminator] == "\u{1B}", endIdx < working.endIndex, working[endIdx] == "\\" {
                    endIdx = working.index(after: endIdx)
                }
                working.removeSubrange(oscRange.lowerBound..<endIdx)
            } else {
                break
            }
        }

        // Strip standard CSI escape sequences for clean fallback text rendering
        let pattern = "\u{1B}\\[[0-9;?]*[A-Za-z]"
        if let regex = try? NSRegularExpression(pattern: pattern) {
            let nsRange = NSRange(working.startIndex..<working.endIndex, in: working)
            working = regex.stringByReplacingMatches(in: working, range: nsRange, withTemplate: "")
        }
        return working
    }

    private mutating func parseOSC7501Payload(_ payload: String) {
        var kv: [String: String] = [:]
        for part in payload.split(separator: ";") {
            let pair = part.split(separator: "=", maxSplits: 1)
            if pair.count == 2 {
                kv[String(pair[0])] = String(pair[1])
            }
        }
        let stateRaw = kv["state"] ?? "idle"
        let state = HerdrLifecycleState(rawValue: stateRaw) ?? .idle
        let progress = kv["progress"].flatMap { Int($0) }
        latestHerdrStatus = HerdrOSC7501Status(
            state: state,
            kind: kv["kind"],
            app: kv["app"],
            title: kv["title"],
            message: kv["msg"],
            progress: progress
        )
    }
}
