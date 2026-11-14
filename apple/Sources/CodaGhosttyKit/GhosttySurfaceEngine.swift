import CodaKit
import Foundation

// MARK: - `libghostty-vt` C-ABI Surface Protocol & TrueColor / OSC 7501 Reflow Engine

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

public enum TactileChordAction: String, CaseIterable, Sendable, Identifiable {
    case shiftTabPlan = "⇧Tab Plan"
    case escapeInterrupt = "Esc"
    case shiftEnterNewline = "⇧↵ Line"
    case approveYes = "Approve (y↵)"
    case ctrlC = "⌃C"
    case atFilePicker = "@File"
    case slashCommand = "/Cmd"

    public var id: String { rawValue }

    public var encodedBytes: Data {
        switch self {
        case .shiftTabPlan: return Data("\u{1B}[Z".utf8)
        case .escapeInterrupt: return Data([0x1B])
        case .shiftEnterNewline: return Data("\u{1B}[13;2u".utf8)
        case .approveYes: return Data("y\r".utf8)
        case .ctrlC: return Data([0x03])
        case .atFilePicker: return Data("@".utf8)
        case .slashCommand: return Data("/".utf8)
        }
    }
}

// MARK: - Rich ANSI SGR + Git Diff Styled Terminal Lines (24-bit TrueColor / 256 / 16)

public struct GhosttyRGBColor: Sendable, Equatable, Hashable {
    public let r: UInt8
    public let g: UInt8
    public let b: UInt8

    public init(r: UInt8, g: UInt8, b: UInt8) {
        self.r = r
        self.g = g
        self.b = b
    }
}

public struct GhosttyStyledSpan: Sendable, Equatable, Identifiable {
    public let id: Int
    public let text: String
    public let fg: GhosttyRGBColor?
    public let bold: Bool
    public let dim: Bool

    public init(id: Int, text: String, fg: GhosttyRGBColor? = nil, bold: Bool = false, dim: Bool = false) {
        self.id = id
        self.text = text
        self.fg = fg
        self.bold = bold
        self.dim = dim
    }
}

public enum GhosttyLineKind: Sendable, Equatable {
    case normal
    case diffAdded
    case diffRemoved
    case diffHunk
    case promptChoice
}

public struct GhosttyStyledLine: Sendable, Equatable, Identifiable {
    public let id: Int
    public let plainText: String
    public let spans: [GhosttyStyledSpan]
    public let kind: GhosttyLineKind

    public init(id: Int, plainText: String, spans: [GhosttyStyledSpan], kind: GhosttyLineKind) {
        self.id = id
        self.plainText = plainText
        self.spans = spans
        self.kind = kind
    }
}

/// Keys exposed in the Herdr Mobile floating quick-bar (`esc`, `ctrl`, `alt`, `tab`, `⇧tab`, arrows, `^C`).
public enum HerdrQuickKey: String, CaseIterable, Sendable {
    case esc
    case tab
    case shiftTab
    case up
    case down
    case left
    case right
    case enter
    case backspace
    case slash
    case ctrlC
    case ctrlD
    case ctrlBackslash

    public var herdrKeyNames: [String] {
        switch self {
        case .esc: return ["Escape"]
        case .tab: return ["Tab"]
        case .shiftTab: return ["BackTab"]
        case .up: return ["Up"]
        case .down: return ["Down"]
        case .left: return ["Left"]
        case .right: return ["Right"]
        case .enter: return ["Enter"]
        case .backspace: return ["BSpace"]
        case .slash: return ["/"]
        case .ctrlC: return ["Ctrl+c"]
        case .ctrlD: return ["Ctrl+d"]
        case .ctrlBackslash: return ["Ctrl+\\"]
        }
    }
}

/// High-speed Swift VT reference engine with `OSC 7501` interception, 24-bit TrueColor ANSI parsing,
/// Git diff classification, and responsive foldable/mobile column reflow.
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
        let (cleaned, parsedStatus) = Self.extractOSC7501(from: chunk, stripCSI: true)
        if let parsedStatus {
            latestHerdrStatus = parsedStatus
        }
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

    public static func encodeKey(_ key: HerdrQuickKey) -> Data {
        switch key {
        case .esc: return Data([0x1B])
        case .tab: return Data([0x09])
        case .shiftTab: return Data("\u{1B}[Z".utf8)
        case .up: return Data("\u{1B}[A".utf8)
        case .down: return Data("\u{1B}[B".utf8)
        case .right: return Data("\u{1B}[C".utf8)
        case .left: return Data("\u{1B}[D".utf8)
        case .enter: return Data([0x0D])
        case .backspace: return Data([0x7F])
        case .slash: return Data("/".utf8)
        case .ctrlC: return Data([0x03])
        case .ctrlD: return Data([0x04])
        case .ctrlBackslash: return Data([0x1C])
        }
    }

    public static func encodeModifiedText(_ text: String, ctrl: Bool, alt: Bool) -> Data {
        guard !text.isEmpty else { return Data() }
        var out = Data()
        for scalar in text.unicodeScalars {
            if ctrl, let ascii = UInt8(exactly: scalar.value) {
                var ctrlByte: UInt8? = nil
                if ascii >= 0x61 && ascii <= 0x7A { // 'a'..'z'
                    ctrlByte = ascii - 0x60
                } else if ascii >= 0x41 && ascii <= 0x5A { // 'A'..'Z'
                    ctrlByte = ascii - 0x40
                } else if ascii == 0x5B { // '['
                    ctrlByte = 0x1B
                } else if ascii == 0x5C { // '\'
                    ctrlByte = 0x1C
                } else if ascii == 0x5D { // ']'
                    ctrlByte = 0x1D
                }
                if let b = ctrlByte {
                    if alt { out.append(0x1B) }
                    out.append(b)
                    continue
                }
            }
            if alt {
                out.append(0x1B)
            }
            out.append(contentsOf: String(scalar).utf8)
        }
        return out
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

    // MARK: - Rich ANSI / Diff Rendering for `CodaTerminalSessionView`

    public static func renderStyledLines(
        rawOutput: String,
        targetColumns: Int
    ) -> (lines: [GhosttyStyledLine], latestStatus: HerdrOSC7501Status?) {
        let cols = max(24, targetColumns)
        let (cleanOfOSC, status) = extractOSC7501(from: rawOutput, stripCSI: false)
        let normalized = cleanOfOSC.replacingOccurrences(of: "\r\n", with: "\n")
            .replacingOccurrences(of: "\r", with: "\n")

        var result: [GhosttyStyledLine] = []
        var lineID = 0

        let rawLines = normalized.components(separatedBy: "\n")
        for rawLine in rawLines {
            let cells = parseSGRCells(rawLine)
            if cells.isEmpty {
                result.append(GhosttyStyledLine(
                    id: lineID,
                    plainText: "",
                    spans: [],
                    kind: .normal
                ))
                lineID += 1
                continue
            }

            // Wrap cells at `cols` without breaking attribute runs
            var idx = 0
            while idx < cells.count {
                let end = min(idx + cols, cells.count)
                let slice = Array(cells[idx..<end])
                let plain = String(slice.map(\.ch))
                let spans = coalesceSpans(slice)
                let kind = classifyLine(plain)
                result.append(GhosttyStyledLine(
                    id: lineID,
                    plainText: plain,
                    spans: spans,
                    kind: kind
                ))
                lineID += 1
                idx = end
            }
        }

        // Trim trailing blank lines if screen has extra trailing newlines
        while result.count > 1, result.last?.plainText.isEmpty == true {
            result.removeLast()
        }
        return (result, status)
    }

    private struct StyledCell {
        let ch: Character
        let fg: GhosttyRGBColor?
        let bold: Bool
        let dim: Bool
    }

    private static func coalesceSpans(_ cells: [StyledCell]) -> [GhosttyStyledSpan] {
        var spans: [GhosttyStyledSpan] = []
        var buf = ""
        var curFg: GhosttyRGBColor?
        var curBold = false
        var curDim = false
        var spanId = 0

        for cell in cells {
            if !buf.isEmpty && (cell.fg != curFg || cell.bold != curBold || cell.dim != curDim) {
                spans.append(GhosttyStyledSpan(id: spanId, text: buf, fg: curFg, bold: curBold, dim: curDim))
                spanId += 1
                buf = ""
            }
            if buf.isEmpty {
                curFg = cell.fg
                curBold = cell.bold
                curDim = cell.dim
            }
            buf.append(cell.ch)
        }
        if !buf.isEmpty {
            spans.append(GhosttyStyledSpan(id: spanId, text: buf, fg: curFg, bold: curBold, dim: curDim))
        }
        return spans
    }

    private static func parseSGRCells(_ input: String) -> [StyledCell] {
        var cells: [StyledCell] = []
        var curFg: GhosttyRGBColor?
        var curBold = false
        var curDim = false

        var i = input.startIndex
        while i < input.endIndex {
            let ch = input[i]
            if ch == "\u{1B}" {
                let next = input.index(after: i)
                if next < input.endIndex && input[next] == "[" {
                    // CSI sequence: ESC [ <params> <final>
                    var j = input.index(after: next)
                    var params = ""
                    while j < input.endIndex {
                        let cj = input[j]
                        if let scalar = cj.unicodeScalars.first, scalar.value >= 0x40 && scalar.value <= 0x7E {
                            if cj == "m" {
                                applySGR(params: params, fg: &curFg, bold: &curBold, dim: &curDim)
                            }
                            j = input.index(after: j)
                            break
                        }
                        params.append(cj)
                        j = input.index(after: j)
                    }
                    i = j
                    continue
                } else if next < input.endIndex && input[next] == "]" {
                    // Any remaining OSC sequence: skip until BEL or ST
                    var j = input.index(after: next)
                    while j < input.endIndex {
                        if input[j] == "\u{07}" {
                            j = input.index(after: j)
                            break
                        }
                        if input[j] == "\u{1B}" {
                            let afterEsc = input.index(after: j)
                            j = (afterEsc < input.endIndex && input[afterEsc] == "\\")
                                ? input.index(after: afterEsc)
                                : afterEsc
                            break
                        }
                        j = input.index(after: j)
                    }
                    i = j
                    continue
                }
            }

            if let scalar = ch.unicodeScalars.first, scalar.value < 0x20 && ch != "\t" {
                i = input.index(after: i)
                continue
            }
            if ch == "\t" {
                for _ in 0..<4 {
                    cells.append(StyledCell(ch: " ", fg: curFg, bold: curBold, dim: curDim))
                }
            } else {
                cells.append(StyledCell(ch: ch, fg: curFg, bold: curBold, dim: curDim))
            }
            i = input.index(after: i)
        }
        return cells
    }

    private static func applySGR(
        params: String,
        fg: inout GhosttyRGBColor?,
        bold: inout Bool,
        dim: inout Bool
    ) {
        if params.isEmpty {
            fg = nil
            bold = false
            dim = false
            return
        }
        let nums = params.split(separator: ";", omittingEmptySubsequences: false).compactMap { Int($0) }
        var idx = 0
        while idx < nums.count {
            let code = nums[idx]
            switch code {
            case 0:
                fg = nil
                bold = false
                dim = false
            case 1:
                bold = true
            case 2:
                dim = true
            case 22:
                bold = false
                dim = false
            case 39:
                fg = nil
            case 30...37:
                fg = standard16Color(code - 30)
            case 90...97:
                fg = standard16Color(8 + (code - 90))
            case 38:
                if idx + 2 < nums.count && nums[idx + 1] == 5 {
                    fg = xterm256Color(nums[idx + 2])
                    idx += 2
                } else if idx + 4 < nums.count && nums[idx + 1] == 2 {
                    let r = UInt8(clamping: nums[idx + 2])
                    let g = UInt8(clamping: nums[idx + 3])
                    let b = UInt8(clamping: nums[idx + 4])
                    fg = GhosttyRGBColor(r: r, g: g, b: b)
                    idx += 4
                }
            default:
                break
            }
            idx += 1
        }
    }

    private static func standard16Color(_ index: Int) -> GhosttyRGBColor {
        switch index {
        case 0: return GhosttyRGBColor(r: 84, g: 92, b: 106)
        case 1: return GhosttyRGBColor(r: 248, g: 81, b: 73)
        case 2: return GhosttyRGBColor(r: 63, g: 185, b: 80)
        case 3: return GhosttyRGBColor(r: 245, g: 158, b: 11)
        case 4: return GhosttyRGBColor(r: 59, g: 130, b: 246)
        case 5: return GhosttyRGBColor(r: 188, g: 140, b: 255)
        case 6: return GhosttyRGBColor(r: 56, g: 189, b: 248)
        case 7: return GhosttyRGBColor(r: 229, g: 233, b: 240)
        case 8: return GhosttyRGBColor(r: 139, g: 148, b: 158)
        case 9: return GhosttyRGBColor(r: 255, g: 123, b: 114)
        case 10: return GhosttyRGBColor(r: 86, g: 211, b: 100)
        case 11: return GhosttyRGBColor(r: 251, g: 191, b: 36)
        case 12: return GhosttyRGBColor(r: 96, g: 165, b: 250)
        case 13: return GhosttyRGBColor(r: 210, g: 168, b: 255)
        case 14: return GhosttyRGBColor(r: 103, g: 232, b: 249)
        default: return GhosttyRGBColor(r: 245, g: 247, b: 250)
        }
    }

    private static func xterm256Color(_ idx: Int) -> GhosttyRGBColor {
        let n = max(0, min(255, idx))
        if n < 16 { return standard16Color(n) }
        if n >= 232 {
            let v = UInt8(clamping: 8 + (n - 232) * 10)
            return GhosttyRGBColor(r: v, g: v, b: v)
        }
        let c = n - 16
        let levels: [UInt8] = [0, 95, 135, 175, 215, 255]
        return GhosttyRGBColor(r: levels[c / 36], g: levels[(c / 6) % 6], b: levels[c % 6])
    }

    private static func classifyLine(_ line: String) -> GhosttyLineKind {
        let trimmed = line.trimmingCharacters(in: .whitespaces)
        if trimmed.hasPrefix("@@") || trimmed.hasPrefix("diff --git") {
            return .diffHunk
        }
        if (line.hasPrefix("+") && !line.hasPrefix("+++")) || line.range(of: #"^\s*\d+\s+\+"#, options: .regularExpression) != nil {
            return .diffAdded
        }
        if (line.hasPrefix("-") && !line.hasPrefix("---")) || line.range(of: #"^\s*\d+\s+-"#, options: .regularExpression) != nil {
            return .diffRemoved
        }
        if trimmed.range(of: #"^(?:[>❯]\s*)?\d+\.\s+\S+"#, options: .regularExpression) != nil {
            return .promptChoice
        }
        return .normal
    }

    // MARK: - Flexible OSC 7501 Extractor (`;` and `:` prefixes, `msg=` and `task=`)

    private static func extractOSC7501(
        from input: String,
        stripCSI: Bool
    ) -> (cleaned: String, latestStatus: HerdrOSC7501Status?) {
        var working = input
        var lastStatus: HerdrOSC7501Status?

        while let oscRange = working.range(of: "\u{1B}]7501;") ?? working.range(of: "\u{1B}]7501:") {
            let sep = working[working.index(before: oscRange.upperBound)]
            let afterPrefix = working[oscRange.upperBound...]
            guard let terminator = afterPrefix.firstIndex(where: { $0 == "\u{07}" || $0 == "\u{1B}" }) else {
                break
            }
            let body = String(afterPrefix[..<terminator])
            if let parsed = parseOSC7501Payload(body, separator: sep) {
                lastStatus = parsed
            }
            var endIdx = working.index(after: terminator)
            if working[terminator] == "\u{1B}", endIdx < working.endIndex, working[endIdx] == "\\" {
                endIdx = working.index(after: endIdx)
            }
            working.removeSubrange(oscRange.lowerBound..<endIdx)
        }

        if stripCSI {
            let pattern = "\u{1B}\\[[0-9;?]*[A-Za-z]"
            if let regex = try? NSRegularExpression(pattern: pattern) {
                let nsRange = NSRange(working.startIndex..<working.endIndex, in: working)
                working = regex.stringByReplacingMatches(in: working, range: nsRange, withTemplate: "")
            }
        }
        return (working, lastStatus)
    }

    private static func parseOSC7501Payload(_ payload: String, separator: Character) -> HerdrOSC7501Status? {
        if payload.contains("=") {
            var kv: [String: String] = [:]
            for part in payload.split(separator: ";") {
                let pair = part.split(separator: "=", maxSplits: 1)
                if pair.count == 2 {
                    kv[String(pair[0]).trimmingCharacters(in: .whitespaces)] = String(pair[1])
                }
            }
            let state = HerdrLifecycleState.from(rawState: kv["state"], kind: kv["kind"])
            let progress = kv["progress"].flatMap { Int($0) }
            return HerdrOSC7501Status(
                state: state,
                kind: kv["kind"],
                app: kv["app"],
                title: kv["title"],
                message: kv["msg"] ?? kv["task"],
                progress: progress
            )
        } else if separator == ":" {
            // Positional form e.g. `7501:blocked:Approve deploy`
            let parts = payload.split(separator: ":", maxSplits: 1)
            let state = HerdrLifecycleState.from(rawState: parts.first.map(String.init))
            let msg = parts.count > 1 ? String(parts[1]) : nil
            return HerdrOSC7501Status(state: state, message: msg)
        }
        return nil
    }
}
